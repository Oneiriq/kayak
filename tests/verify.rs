//! Plan verification against a real embedded engine.
//!
//! Static validation proves an index exists for every claim; these
//! tests prove the OTHER half of the promise: that `verify` catches
//! the planner declining to use one. They run the in-process `mem://`
//! engine (the `surrealdb` dev-dependency feature-unifies `kv-mem`
//! into the client), so they always run under `cargo test
//! --all-features` and in CI without a server.
#![cfg(feature = "verify")]

use std::sync::atomic::{AtomicU64, Ordering};

use kayak::verify::{probes, verify_contract};
use kayak::{Contract, FieldExposure, Query, Resource, SearchBacking, SearchKind, SubResource};
use surql::connection::ConnectionConfig;
use surql::DatabaseClient;

static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn memory_client() -> DatabaseClient {
    let seq = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("kayak_verify_{seq}");
    let cfg = ConnectionConfig::builder()
        .url("mem://")
        .namespace(name.clone())
        .database(name)
        .build()
        .expect("valid mem config");
    let client = DatabaseClient::new(cfg).expect("client constructs");
    client.connect().await.expect("connect to embedded engine");
    client
}

/// File-service listing tables: `file` seekable through its listing
/// index, `file_version` seekable through the parent key, and `note`
/// carrying no index at all, so its claims have nothing to plan on.
const DDL: &str = "
DEFINE TABLE file SCHEMAFULL;
DEFINE FIELD tenant_id ON file TYPE string;
DEFINE FIELD state ON file TYPE string;
DEFINE FIELD path ON file TYPE string;
DEFINE FIELD created_at ON file TYPE datetime;
DEFINE INDEX idx_listing ON file FIELDS tenant_id, state, created_at;
DEFINE TABLE file_version SCHEMAFULL;
DEFINE FIELD file ON file_version TYPE string;
DEFINE FIELD tenant_id ON file_version TYPE string;
DEFINE FIELD number ON file_version TYPE int;
DEFINE FIELD created_at ON file_version TYPE datetime;
DEFINE INDEX idx_versions ON file_version FIELDS file, tenant_id, created_at;
DEFINE TABLE note SCHEMAFULL;
DEFINE FIELD tenant_id ON note TYPE string;
DEFINE FIELD title ON note TYPE string;
DEFINE FIELD body ON note TYPE string;
CREATE file SET tenant_id = 't1', state = 'ready', path = '/a', created_at = time::now();
CREATE file_version SET file = 'f1', tenant_id = 't1', number = 1, created_at = time::now();
CREATE note SET tenant_id = 't1', title = 'a', body = 'b';
";

fn resource(name: &str, table: &str) -> Resource {
    Resource {
        name: name.into(),
        table: table.into(),
        identity: Default::default(),
        fields: vec![FieldExposure::column("created_at")],
        pinned: vec!["tenant_id".into()],
        pinned_either: vec![],
        filterable: vec![],
        filter_options: Default::default(),
        faces: Default::default(),
        sortable: vec![],
        max_page_size: 100,
        actions: vec![],
        content: None,
        sub_resources: vec![],
        rate_class: None,
        reads_require: vec![],
        watchable: false,
        graphql: None,
    }
}

fn contract(resources: Vec<Resource>) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        rate_classes: vec![],
        limits: None,
        auth: Default::default(),
        resources,
        queries: vec![],
    }
}

/// A file service's search tables, both search indexes in place: the analyzer,
/// BM25 over the passage text, HNSW over its embedding. Dimension 3
/// because the fixture controls it and three is enough to seek.
const SEARCH_DDL: &str = "
DEFINE ANALYZER chunk_text TOKENIZERS class FILTERS lowercase, ascii, snowball(english);
DEFINE TABLE text_chunk SCHEMAFULL;
DEFINE FIELD tenant_id ON text_chunk TYPE string;
DEFINE FIELD body ON text_chunk TYPE string;
DEFINE FIELD embedding ON text_chunk TYPE option<array<float>>;
DEFINE INDEX idx_chunk_body ON text_chunk FIELDS body FULLTEXT ANALYZER chunk_text BM25;
DEFINE INDEX idx_chunk_embedding ON text_chunk FIELDS embedding HNSW DIMENSION 3 DIST COSINE TYPE F32;
CREATE text_chunk SET tenant_id = 't1', body = 'the quick brown fox', embedding = [0.1, 0.2, 0.3];
";

/// The drift scenario: the same table after a schema change dropped
/// both search indexes, while the contract still promises search.
const DRIFTED_DDL: &str = "
DEFINE TABLE text_chunk SCHEMAFULL;
DEFINE FIELD tenant_id ON text_chunk TYPE string;
DEFINE FIELD body ON text_chunk TYPE string;
DEFINE FIELD embedding ON text_chunk TYPE option<array<float>>;
CREATE text_chunk SET tenant_id = 't1', body = 'the quick brown fox', embedding = [0.1, 0.2, 0.3];
";

/// A contract whose one query declares a file service's two backings.
fn searching_contract() -> Contract {
    let mut searching = contract(vec![]);
    searching.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: vec![],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![SearchKind::Lexical, SearchKind::Vector],
        backing: vec![
            SearchBacking {
                table: "text_chunk".into(),
                column: "body".into(),
                index: "idx_chunk_body".into(),
                kind: SearchKind::Lexical,
                dimension: None,
                optional: false,
            },
            SearchBacking {
                table: "text_chunk".into(),
                column: "embedding".into(),
                index: "idx_chunk_embedding".into(),
                kind: SearchKind::Vector,
                dimension: Some(3),
                optional: false,
            },
        ],
    }];
    searching
}

/// The plan vocabulary this crate matches against, pinned to the
/// engine. `verify`'s worst failure mode is the vocabulary drifting
/// under it: a renamed operator would turn every verification
/// silently green. So the probe that discovered the strings stays on
/// as a test, and an engine upgrade that respells its plans fails
/// HERE, naming the operator, instead of nowhere.
///
/// Probed on SurrealDB 3.x over `mem://` (abridged to the attributes
/// the matcher reads):
///
/// ```text
/// SELECT * FROM file WHERE tenant_id = 't1' AND state = 'ready'
///     LIMIT 5 EXPLAIN
/// -> {"operator": "SelectProject", "children": [
///      {"operator": "IndexScan", "attributes": {
///        "index": "idx_listing", "access": "['t1', 'ready']",
///        "direction": "Forward", "limit": "5"}}]}
///
/// SELECT * FROM note ORDER BY title LIMIT 5 EXPLAIN
/// -> {"operator": "SelectProject", "children": [
///      {"operator": "Limit", "children": [
///        {"operator": "SortTopKByKey", "children": [
///          {"operator": "TableScan", "attributes": {
///            "table": "note", "direction": "Forward",
///            "topk_pushdown": "yes"}}]}]}]}
/// ```
///
/// And the search operators, probed the same way. Served, each names
/// its index in one leaf; unserved, `@@` degrades to a `TableScan`
/// carrying the predicate as an attribute and `<|k,EF|>` to a bare
/// one. The wrong-dimension literal still resolves the index, which
/// is what lets a probe carry `[0]` without knowing the embedding
/// width. MTREE is gone from this engine entirely: the `DEFINE` no
/// longer parses, and `<|k|>` errors with "no longer supported".
///
/// ```text
/// SELECT * FROM text_chunk WHERE body @@ 'quick' EXPLAIN
/// -> {"operator": "SelectProject", "children": [
///      {"operator": "FullTextScan", "attributes": {
///        "index": "idx_chunk_body", "query": "quick"}}]}
///
/// SELECT * FROM text_chunk WHERE embedding <|4,40|> [0.1, 0.2, 0.3] EXPLAIN
/// -> {"operator": "SelectProject", "children": [
///      {"operator": "KnnScan", "attributes": {
///        "index": "idx_chunk_embedding", "dimension": "3",
///        "ef": "40", "k": "4"}}]}
///
/// SELECT * FROM text_chunk WHERE embedding <|4,COSINE|> [0.1, 0.2, 0.3] EXPLAIN
/// -> {"operator": "SelectProject", "children": [
///      {"operator": "KnnTopK", "attributes": {"dimension": "3",
///        "distance": "Cosine", "field": "embedding", "k": "4"},
///       "children": [
///        {"operator": "TableScan", "attributes": {
///          "table": "text_chunk", "direction": "Forward"}}]}]}
/// ```
#[tokio::test]
async fn the_probed_plan_vocabulary_still_holds() {
    let client = memory_client().await;
    client.query(DDL).await.expect("schema applies");

    let seeking = client
        .query("SELECT * FROM file WHERE tenant_id = 't1' AND state = 'ready' LIMIT 5 EXPLAIN")
        .await
        .expect("explain answers");
    let text = seeking.to_string();
    assert!(text.contains("\"IndexScan\""), "{text}");
    assert!(text.contains("idx_listing"), "{text}");
    assert!(!text.contains("\"TableScan\""), "{text}");

    let walking = client
        .query("SELECT * FROM note ORDER BY title LIMIT 5 EXPLAIN")
        .await
        .expect("explain answers");
    let text = walking.to_string();
    assert!(text.contains("\"TableScan\""), "{text}");
    assert!(text.contains("\"operator\""), "{text}");
    assert!(text.contains("\"children\""), "{text}");

    // The search half of the vocabulary, against the search tables.
    let client = memory_client().await;
    client.query(SEARCH_DDL).await.expect("schema applies");

    let matching = client
        .query("SELECT * FROM text_chunk WHERE body @@ 'quick' EXPLAIN")
        .await
        .expect("explain answers");
    let text = matching.to_string();
    assert!(text.contains("\"FullTextScan\""), "{text}");
    assert!(text.contains("idx_chunk_body"), "{text}");
    assert!(!text.contains("\"TableScan\""), "{text}");

    let neighboring = client
        .query("SELECT * FROM text_chunk WHERE embedding <|1,64|> [0] EXPLAIN")
        .await
        .expect("explain answers");
    let text = neighboring.to_string();
    assert!(text.contains("\"KnnScan\""), "{text}");
    assert!(text.contains("idx_chunk_embedding"), "{text}");
    assert!(!text.contains("\"TableScan\""), "{text}");

    // The metric KNN form ignores the index even where one exists,
    // which is why the probe composes `<|k,EF|>` the way a real search does.
    let brute = client
        .query("SELECT * FROM text_chunk WHERE embedding <|1,COSINE|> [0] EXPLAIN")
        .await
        .expect("explain answers");
    let text = brute.to_string();
    assert!(text.contains("\"KnnTopK\""), "{text}");
    assert!(text.contains("\"TableScan\""), "{text}");
}

/// Claims the schema really serves come back clean, sub-resources
/// included: the files listing seeks `idx_listing` for its filter and
/// its sort, and the versions listing seeks `idx_versions` through
/// the parent key the server always binds.
#[tokio::test]
async fn a_backed_contract_verifies_clean() {
    let client = memory_client().await;
    client.query(DDL).await.expect("schema applies");

    let mut files = resource("files", "file");
    files.filterable = vec!["state".into()];
    files.filter_options = [("state".to_owned(), vec!["ready".to_owned()])]
        .into_iter()
        .collect();
    files.sortable = vec!["created_at".into()];
    files.sub_resources = vec![SubResource {
        name: "versions".into(),
        table: "file_version".into(),
        parent_key: "file".into(),
        identity: Default::default(),
        fields: vec![FieldExposure::column("number")],
        pinned: vec!["tenant_id".into()],
        filterable: vec![],
        sortable: vec!["created_at".into()],
        max_page_size: 50,
        description: None,
        graphql: None,
    }];
    let contract = contract(vec![files]);

    let violations = verify_contract(&client, &contract).await.unwrap();
    assert_eq!(violations, vec![], "every claim plans on an index");
    // And the run actually asked something: three claims, three probes.
    assert_eq!(probes(&contract).len(), 3);
}

/// A claim nothing serves fails naming the claim, which is the whole
/// deliverable: "the planner walks `note` for `sort title`" is
/// actionable in a way a slow endpoint six weeks later is not.
#[tokio::test]
async fn an_unserved_claim_names_itself() {
    let client = memory_client().await;
    client.query(DDL).await.expect("schema applies");

    // `note` has no index: the sort claim has nothing to walk in
    // order, and the filter claim (pins bound first, like a real
    // listing) has nothing to seek.
    let mut notes = resource("notes", "note");
    notes.pinned = vec![];
    notes.sortable = vec!["title".into()];
    notes.filterable = vec!["body".into()];
    let violations = verify_contract(&client, &contract(vec![notes]))
        .await
        .unwrap();

    let rendered: Vec<String> = violations.iter().map(ToString::to_string).collect();
    assert_eq!(violations.len(), 2, "{rendered:?}");
    assert!(
        rendered
            .iter()
            .any(|v| v.starts_with("notes: filter body plans as \"TableScan over note\"")),
        "{rendered:?}",
    );
    assert!(
        rendered
            .iter()
            .any(|v| v.starts_with("notes: sort title plans as \"TableScan over note\"")),
        "{rendered:?}",
    );
}

/// A file service's search surface, backed the way its schema really is:
/// both backings reach their named index, so the contract's promise
/// and the planner's answer agree.
#[tokio::test]
async fn a_backed_search_verifies_clean() {
    let client = memory_client().await;
    client.query(SEARCH_DDL).await.expect("schema applies");

    let contract = searching_contract();
    let violations = verify_contract(&client, &contract).await.unwrap();
    assert_eq!(violations, vec![], "both backings reach their index");
    assert_eq!(probes(&contract).len(), 2, "one probe per backing");
}

/// The motivating drift: a schema change dropped the search indexes
/// while the contract still promises search. Static validation against
/// the CHECKED-IN schema would still pass -- the live database is where
/// the divergence lives, so the live planner is what convicts it,
/// naming each backing.
#[tokio::test]
async fn a_dropped_search_index_convicts_the_backing_by_name() {
    let client = memory_client().await;
    client.query(DRIFTED_DDL).await.expect("schema applies");

    let violations = verify_contract(&client, &searching_contract())
        .await
        .unwrap();
    let rendered: Vec<String> = violations.iter().map(ToString::to_string).collect();
    assert_eq!(violations.len(), 2, "{rendered:?}");
    assert!(
        rendered.iter().any(|v| v.starts_with(
            "query search: lexical backing text_chunk.body plans as \"TableScan over text_chunk\""
        )),
        "{rendered:?}",
    );
    assert!(
        rendered.iter().any(|v| v.starts_with(
            "query search: vector backing text_chunk.embedding plans as \
             \"TableScan over text_chunk\""
        )),
        "{rendered:?}",
    );
}

/// An optional backing is excused on a database that never had the
/// index, and convicted on one that has it and does not use it.
///
/// Both halves matter, and the second is the one that makes the flag
/// worth having. "This deployment may not have configured it" is a
/// true statement about a real embedding index and a tempting cover
/// for anything at all, so the excuse is granted on exactly one fact:
/// the index is not defined here. Define it, and the claim answers to
/// the planner like every other.
#[tokio::test]
async fn an_optional_backing_is_excused_only_by_an_absent_index() {
    let mut optional = searching_contract();
    optional.queries[0].searches = vec![SearchKind::Vector];
    optional.queries[0]
        .backing
        .retain(|b| b.kind == SearchKind::Vector);
    optional.queries[0].backing[0].optional = true;

    // No index: nothing to answer for.
    let bare = memory_client().await;
    bare.query(DRIFTED_DDL).await.expect("schema applies");
    assert_eq!(
        verify_contract(&bare, &optional).await.unwrap(),
        vec![],
        "an index the deployment never configured is not a broken promise",
    );

    // The index exists: the plan has to reach it, and here it does.
    let configured = memory_client().await;
    configured.query(SEARCH_DDL).await.expect("schema applies");
    assert_eq!(
        verify_contract(&configured, &optional).await.unwrap(),
        vec![]
    );

    // The index exists and the query cannot reach it -- the metric
    // form of the KNN operator, which plans as KnnTopK over a
    // TableScan even with the index in place. Optional does not
    // excuse it.
    let mut wrong_column = optional.clone();
    wrong_column.queries[0].backing[0].column = "tenant_id".into();
    let violations = verify_contract(&configured, &wrong_column).await.unwrap();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].claim, "vector backing text_chunk.tenant_id");
}

/// The planner, not the catalog, is the judge: `state` IS indexed on
/// `file`, but a listing that binds no pins hands the planner a
/// predicate on the index's second column, which it cannot seek -- the
/// static gate's prefix rule said so, and the plan agrees. This is
/// the divergence class `verify` exists for.
#[tokio::test]
async fn an_index_the_listing_cannot_seek_is_still_a_walk() {
    let client = memory_client().await;
    client.query(DDL).await.expect("schema applies");

    let mut files = resource("files", "file");
    files.pinned = vec![];
    files.filterable = vec!["state".into()];
    let violations = verify_contract(&client, &contract(vec![files]))
        .await
        .unwrap();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].claim, "filter state");
    assert_eq!(violations[0].operation, "TableScan over file");
}

/// An either-of pin is probed once per branch. The listing binds `a`
/// or `b`, and the engine answers each branch with its own seek, so a
/// branch no index serves is a walk even when the other branch seeks.
/// Verification used to bind neither column and probe a listing no
/// caller ever sends.
#[tokio::test]
async fn each_branch_of_an_either_pin_is_verified() {
    let client = memory_client().await;
    client
        .query(
            "
DEFINE TABLE friendship SCHEMAFULL;
DEFINE FIELD a ON friendship TYPE string;
DEFINE FIELD b ON friendship TYPE string;
DEFINE FIELD state ON friendship TYPE string;
DEFINE INDEX idx_by_a ON friendship FIELDS a, state;
CREATE friendship SET a = 'x', b = 'y', state = 'ready';
",
        )
        .await
        .expect("schema applies");

    let mut friends = resource("friends", "friendship");
    friends.pinned = vec![];
    friends.pinned_either = vec!["a".into(), "b".into()];
    friends.filterable = vec!["state".into()];
    let contract = contract(vec![friends]);

    let composed = probes(&contract);
    assert_eq!(composed.len(), 2, "{composed:?}");
    assert_eq!(composed[0].scope, "friends (pinned on a)");
    assert!(composed[0].surql.contains("a = 'kayak-probe' AND state"));
    assert_eq!(composed[1].scope, "friends (pinned on b)");
    assert!(composed[1].surql.contains("b = 'kayak-probe' AND state"));

    let violations = verify_contract(&client, &contract).await.unwrap();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].scope, "friends (pinned on b)");
    assert_eq!(violations[0].claim, "filter state");
    assert_eq!(violations[0].operation, "TableScan over friendship");
}

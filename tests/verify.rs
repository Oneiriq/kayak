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

use janus::verify::{probes, verify_contract};
use janus::{Contract, FieldExposure, Resource, SubResource};
use surql::connection::ConnectionConfig;
use surql::DatabaseClient;

static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn memory_client() -> DatabaseClient {
    let seq = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("janus_verify_{seq}");
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

/// Copal-shaped listing tables: `file` seekable through its listing
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
        fields: vec![FieldExposure::column("created_at")],
        pinned: vec!["tenant_id".into()],
        filterable: vec![],
        filter_options: Default::default(),
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
        rate_classes: vec![],
        limits: None,
        resources,
        queries: vec![],
    }
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

/// The planner, not the catalog, is the judge: `state` IS indexed on
/// `file`, but a listing that binds no pins hands the planner a
/// predicate on the index's second column, which it cannot seek — the
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

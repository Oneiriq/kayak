//! The gate and the generator, driven by a schema shaped like Copal's
//! `file` table (built inline: Janus takes schema definitions as input
//! and depends on no consumer).

use janus::{
    generate_openapi, validate, ActionField, Contract, FieldExposure, Query, Resource,
    SearchBacking, SearchKind, TypeRef, Violation,
};
use surql::schema::{
    array_field, bm25_index, datetime_field, hnsw_index, index, int_field, mtree_index,
    string_field, table_schema, unique_index, HnswDistanceType, IndexType, MTreeDistanceType,
    MTreeVectorType, TableDefinition, TableMode,
};

fn file_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("path")),
            built(string_field("state")),
            built(string_field("content_type")),
            built(int_field("size_bytes").nullable(true)),
            built(string_field("digest").nullable(true)),
            built(datetime_field("created_at")),
        ])
        .with_indexes([
            unique_index("uniq_live_path", ["tenant_id", "path", "live_marker"]),
            // The REAL copal shape: created_at leads no index. The sort
            // is reachable only through this prefix, which is exactly
            // what the prefix rule exists to credit.
            index("idx_listing", ["tenant_id", "state", "created_at"]),
        ])
}

/// Copal's `text_chunk` shape, with the index set left open because
/// what varies between these cases is only which index covers what.
fn chunk_table(
    indexes: impl IntoIterator<Item = surql::schema::IndexDefinition>,
) -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("text_chunk")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("file")),
            built(int_field("ordinal")),
            built(string_field("body")),
            built(array_field("embedding")),
            built(array_field("locator")),
        ])
        .with_indexes(indexes)
}

/// The real thing: plainly indexed, and none of it in a way that
/// answers a filter or a sort on the columns anyone wants to claim.
fn text_chunk_table() -> TableDefinition {
    chunk_table([
        unique_index("uniq_chunk_position", ["file", "ordinal"]),
        // The real table's tenant listing index, which is also what
        // seats the pinned tenant_id: without it the resource under
        // test would additionally be an unreachable listing, and these
        // tests are about the claims.
        index("idx_chunk_tenant", ["tenant_id", "created_at"]),
        bm25_index("idx_chunk_body", ["body"], "copal_text"),
        hnsw_index(
            "idx_chunk_embedding",
            "embedding",
            768,
            HnswDistanceType::Cosine,
            MTreeVectorType::F32,
            None,
            None,
        ),
        mtree_index(
            "idx_chunk_locator",
            "locator",
            3,
            MTreeDistanceType::Euclidean,
            MTreeVectorType::F64,
        ),
    ])
}

fn files_resource() -> Resource {
    Resource {
        name: "files".into(),
        table: "file".into(),
        filter_options: Default::default(),
        fields: vec![
            FieldExposure::column("path"),
            FieldExposure::column("state"),
            FieldExposure::column("content_type"),
            FieldExposure::renamed("size_bytes", "size"),
            FieldExposure::column("digest"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["state".into()],
        sortable: vec!["created_at".into()],
        max_page_size: 100,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![],
        actions: vec![],
        content: None,
    }
}

fn contract(resources: Vec<Resource>) -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        limits: None,
        rate_classes: vec![],
        resources,
        queries: vec![],
    }
}

#[test]
fn valid_contract_passes_the_gate() {
    assert_eq!(
        validate(&contract(vec![files_resource()]), &[file_table()]),
        vec![]
    );
}

#[test]
fn unindexed_sort_fails_generation_by_name() {
    let mut resource = files_resource();
    // `digest` exists but leads no index.
    resource.sortable.push("digest".into());
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    assert_eq!(violations.len(), 1);
    assert!(matches!(
        &violations[0],
        Violation::UnindexedSort { column, .. } if column == "digest"
    ));
    // The generator refuses, and the message names the field.
    let err = generate_openapi(
        &contract(vec![{
            let mut r = files_resource();
            r.sortable.push("digest".into());
            r
        }]),
        &[file_table()],
    )
    .unwrap_err();
    assert!(err.to_string().contains("digest"), "{err}");
}

#[test]
fn prefix_rule_credits_pinned_and_filterable_columns() {
    // `state` sits behind only the pinned tenant_id: sortable.
    let mut resource = files_resource();
    resource.sortable.push("state".into());
    assert_eq!(validate(&contract(vec![resource]), &[file_table()]), vec![]);

    // But drop `state` from filterable and created_at (behind
    // tenant_id, state) loses its path to the index: refused.
    let mut resource = files_resource();
    resource.filterable.clear();
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    assert!(matches!(
        &violations[0],
        Violation::UnindexedSort { column, .. } if column == "created_at"
    ));

    // And without the pin nothing behind tenant_id is reachable.
    let mut resource = files_resource();
    resource.pinned.clear();
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    assert!(matches!(
        &violations[0],
        Violation::UnindexedSort { column, .. } if column == "created_at"
    ));
}

/// A column can be indexed and still be neither filterable nor
/// sortable, and that is the case most likely to be got wrong.
///
/// The shape is copal's `text_chunk`: `body` carries a BM25 index for
/// lexical recall, `embedding` an HNSW index for vector recall,
/// `locator` an MTREE one. None of the three has a b-tree behind it, so
/// an equality filter on `body` scans the table and an ORDER BY down
/// `embedding` is not a thing the engine will do. The gate matched on
/// column membership alone and accepted every one of them, which is
/// worse than accepting a claim against no index at all: the author
/// looked, found an index, and was told they were right.
#[test]
fn a_claim_resting_on_a_search_or_vector_index_is_refused() {
    let resource = Resource {
        table: "text_chunk".into(),
        fields: vec![
            FieldExposure::column("body"),
            FieldExposure::column("embedding"),
            FieldExposure::column("locator"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["body".into(), "embedding".into(), "locator".into()],
        sortable: vec!["body".into(), "embedding".into(), "locator".into()],
        ..files_resource()
    };
    let violations = validate(&contract(vec![resource]), &[text_chunk_table()]);

    for (column, index, kind) in [
        ("body", "idx_chunk_body", IndexType::Search),
        ("embedding", "idx_chunk_embedding", IndexType::Hnsw),
        ("locator", "idx_chunk_locator", IndexType::Mtree),
    ] {
        for claim in ["filterable", "sortable"] {
            assert!(
                violations.iter().any(|v| matches!(
                    v,
                    Violation::WrongIndexType {
                        column: c, claim: k, index: i, index_type: t, ..
                    } if c == column && k == claim && i == index && *t == kind
                )),
                "{claim} {column} rests on {index} alone and must be refused: {violations:?}",
            );
        }
    }
    assert_eq!(violations.len(), 6, "{violations:?}");

    // Refusing is half the job. An author who reads "not covered by any
    // index" against a table that visibly has one goes looking for the
    // bug in janus, so the message names the index they were looking at
    // and what it turned out to be.
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("the FULLTEXT index idx_chunk_body"), "{text}");
    assert!(
        text.contains("the HNSW index idx_chunk_embedding"),
        "{text}"
    );
    assert!(text.contains("the MTREE index idx_chunk_locator"), "{text}");

    // And generation refuses rather than shipping the scan.
    let err = generate_openapi(
        &contract(vec![Resource {
            table: "text_chunk".into(),
            fields: vec![FieldExposure::column("body")],
            pinned: vec!["tenant_id".into()],
            filterable: vec!["body".into()],
            sortable: vec![],
            ..files_resource()
        }]),
        &[text_chunk_table()],
    )
    .unwrap_err();
    assert!(err.to_string().contains("idx_chunk_body"), "{err}");
}

/// The same column under both kinds of index is a different answer.
///
/// A BM25 index beside a standard one is the ordinary way to make a
/// column both searchable and filterable, and refusing that would push
/// authors to drop the search index to satisfy a gate. The rule is
/// about what covers the column, not about what else exists.
#[test]
fn an_ordering_index_beside_a_search_one_still_carries_the_claim() {
    let table = chunk_table([
        index("idx_chunk_tenant", ["tenant_id", "body"]),
        bm25_index("idx_chunk_body", ["body"], "copal_text"),
    ]);
    let resource = Resource {
        table: "text_chunk".into(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["body".into()],
        sortable: vec!["body".into()],
        ..files_resource()
    };
    assert_eq!(validate(&contract(vec![resource]), &[table]), vec![]);
}

/// A sort a standard index does hold, but behind an unbound prefix, is
/// still a prefix problem.
///
/// Naming the search index there would send the author to define an
/// index they already have, so the older violation keeps the case.
#[test]
fn an_unbound_prefix_is_reported_as_a_prefix_and_not_as_an_index_type() {
    let table = chunk_table([
        index("idx_chunk_body", ["file", "body"]),
        bm25_index("idx_chunk_search", ["body"], "copal_text"),
    ]);
    let resource = Resource {
        table: "text_chunk".into(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec![],
        filterable: vec![],
        sortable: vec!["body".into()],
        ..files_resource()
    };
    let violations = validate(&contract(vec![resource]), &[table]);
    assert!(
        matches!(
            &violations[0],
            Violation::UnindexedSort { column, .. } if column == "body"
        ),
        "{violations:?}",
    );
}

/// One backing, for the tests that vary a member at a time.
fn backing(column: &str, index: &str, kind: SearchKind) -> SearchBacking {
    SearchBacking {
        table: "text_chunk".into(),
        column: column.into(),
        index: index.into(),
        kind,
    }
}

/// A query holding `backing`, on a contract with no resources: the
/// backing rules do not care what else the contract exposes.
fn searching(backing: Vec<SearchBacking>) -> Contract {
    let mut contract = contract(vec![]);
    contract.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: vec![ActionField {
            name: "q".into(),
            kind: TypeRef::String,
            required: true,
            multiple: false,
            description: None,
            options: Vec::new(),
        }],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        backing,
    }];
    contract
}

/// Copal's real search surface, declared.
///
/// The `search` query in `crates/copal-server/src/contract/search.rs`
/// is served by machinery the contract never named: BM25 over
/// `text_chunk.body` through `idx_chunk_body` and HNSW over
/// `text_chunk.embedding` through `idx_chunk_embedding` (the fused
/// implementation in `crates/copal-store/src/repo/text.rs`, the
/// indexes in `crates/copal-store/src/schema/text.rs`), fused in the
/// resolver — so nothing stopped a schema change from dropping
/// `idx_chunk_body` while the contract went on promising search.
/// Declared as two backings, the same surface validates clean against
/// the copal-shaped table, and `file_text` beside it shows a backing
/// is optional: plain queries stay legal. This fixture is what makes
/// the copal adoption a three-line contract edit.
#[test]
fn copal_search_declared_with_its_backing_validates_clean() {
    let mut contract = searching(vec![
        backing("body", "idx_chunk_body", SearchKind::Lexical),
        backing("embedding", "idx_chunk_embedding", SearchKind::Vector),
    ]);
    contract.queries.push(Query {
        name: "file_text".into(),
        path: "/v1/files/{id}/text".into(),
        input: vec![ActionField {
            name: "id".into(),
            kind: TypeRef::String,
            required: true,
            multiple: false,
            description: None,
            options: Vec::new(),
        }],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        backing: vec![],
    });
    assert_eq!(validate(&contract, &[text_chunk_table()]), vec![]);

    // MTREE is the other vector machinery the schema layer can spell,
    // and a vector backing accepts it the way the WrongIndexType rule
    // groups it with HNSW.
    let mtree = searching(vec![backing(
        "locator",
        "idx_chunk_locator",
        SearchKind::Vector,
    )]);
    assert_eq!(validate(&mtree, &[text_chunk_table()]), vec![]);
}

/// The mirror image of `a_claim_resting_on_a_search_or_vector_index_is_refused`:
/// there a filter rested on search machinery that cannot narrow, here
/// a search rests on machinery that cannot search, and the refusal
/// teaches the same way — name the index the author was looking at,
/// say what it turned out to be, say what the claim needs.
#[test]
fn a_backing_resting_on_the_wrong_index_kind_is_refused_by_name() {
    for (column, index, kind, expected_type) in [
        // A b-tree under a lexical backing: the plain listing index.
        (
            "tenant_id",
            "idx_chunk_tenant",
            SearchKind::Lexical,
            surql::schema::IndexType::Standard,
        ),
        // A unique index under a vector backing.
        (
            "file",
            "uniq_chunk_position",
            SearchKind::Vector,
            surql::schema::IndexType::Unique,
        ),
        // The two search machineries crossed.
        (
            "body",
            "idx_chunk_body",
            SearchKind::Vector,
            surql::schema::IndexType::Search,
        ),
        (
            "embedding",
            "idx_chunk_embedding",
            SearchKind::Lexical,
            surql::schema::IndexType::Hnsw,
        ),
    ] {
        let contract = searching(vec![backing(column, index, kind)]);
        let violations = validate(&contract, &[text_chunk_table()]);
        assert_eq!(violations.len(), 1, "{column}/{index}: {violations:?}");
        assert!(
            matches!(
                &violations[0],
                Violation::WrongBackingIndexType {
                    column: c, index: i, kind: k, index_type: t, ..
                } if c == column && i == index && *k == kind && *t == expected_type
            ),
            "{column}/{index}: {violations:?}",
        );
    }

    // The message does the teaching: the index, what it is, what each
    // kind needs.
    let contract = searching(vec![backing(
        "tenant_id",
        "idx_chunk_tenant",
        SearchKind::Lexical,
    )]);
    let text = validate(&contract, &[text_chunk_table()])[0].to_string();
    assert!(text.contains("the plain index idx_chunk_tenant"), "{text}");
    assert!(
        text.contains("a lexical backing needs a FULLTEXT index"),
        "{text}"
    );
    assert!(
        text.contains("a vector backing needs an HNSW or MTREE one"),
        "{text}"
    );
    let crossed = searching(vec![backing("body", "idx_chunk_body", SearchKind::Vector)]);
    let text = validate(&crossed, &[text_chunk_table()])[0].to_string();
    assert!(text.contains("the FULLTEXT index idx_chunk_body"), "{text}");
}

/// One expected refusal, as a predicate over the violation.
type Refusal = Box<dyn Fn(&Violation) -> bool>;

/// Every name a backing carries resolves or is refused by name, and
/// an index that resolves but holds a different column is its own
/// refusal rather than a type complaint about the wrong thing.
#[test]
fn a_backing_that_resolves_nothing_is_named() {
    let cases: Vec<(Contract, Refusal)> = vec![
        (
            {
                let mut wrong =
                    searching(vec![backing("body", "idx_chunk_body", SearchKind::Lexical)]);
                wrong.queries[0].backing[0].table = "no_such_table".into();
                wrong
            },
            Box::new(
                |v| matches!(v, Violation::UnknownBackingTable { table, .. } if table == "no_such_table"),
            ),
        ),
        (
            searching(vec![backing(
                "no_such_column",
                "idx_chunk_body",
                SearchKind::Lexical,
            )]),
            Box::new(
                |v| matches!(v, Violation::UnknownBackingColumn { column, .. } if column == "no_such_column"),
            ),
        ),
        (
            searching(vec![backing("body", "no_such_index", SearchKind::Lexical)]),
            Box::new(
                |v| matches!(v, Violation::UnknownBackingIndex { index, .. } if index == "no_such_index"),
            ),
        ),
        (
            // The index exists and is even the right kind, but it
            // holds `body`, not `ordinal`.
            searching(vec![backing(
                "ordinal",
                "idx_chunk_body",
                SearchKind::Lexical,
            )]),
            Box::new(|v| {
                matches!(
                    v,
                    Violation::BackingIndexElsewhere { column, index, .. }
                        if column == "ordinal" && index == "idx_chunk_body"
                )
            }),
        ),
    ];
    for (contract, matches_case) in cases {
        let violations = validate(&contract, &[text_chunk_table()]);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(matches_case(&violations[0]), "{violations:?}");
    }

    // Declaring the same backing twice promises one thing twice and
    // makes any later diff of it ambiguous.
    let doubled = searching(vec![
        backing("body", "idx_chunk_body", SearchKind::Lexical),
        backing("body", "idx_chunk_body", SearchKind::Lexical),
    ]);
    let violations = validate(&doubled, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].to_string().contains("same backing twice"),
        "{violations:?}",
    );
}

#[test]
fn pinned_columns_must_exist() {
    let mut resource = files_resource();
    resource.pinned.push("no_such_pin".into());
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    assert!(matches!(
        &violations[0],
        Violation::UnknownColumn { column, .. } if column == "no_such_pin"
    ));
}

/// Copal's `webhook_delivery` shape: tenant-scoped rows whose indexes
/// serve the dispatcher and the parent endpoint, never the tenant.
/// Exposed as a top-level resource this scans; reached through the
/// endpoint it is the sub-collection copal actually ships.
fn delivery_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("webhook_delivery")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("endpoint")),
            built(string_field("state")),
            built(datetime_field("created_at")),
        ])
        .with_indexes([
            index("idx_delivery_due", ["state", "created_at"]),
            index("idx_delivery_endpoint", ["endpoint", "created_at"]),
        ])
}

/// Pins are bound on every read, so the bound set must reach an index
/// or the plain listing scans. The claims rules never see this: the
/// resource below claims nothing a caller could send wrong, and it
/// still cannot be listed well.
#[test]
fn pins_no_index_leads_with_are_refused() {
    let resource = Resource {
        name: "deliveries".into(),
        table: "webhook_delivery".into(),
        filter_options: Default::default(),
        fields: vec![
            FieldExposure::column("endpoint"),
            FieldExposure::column("state"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["state".into()],
        sortable: vec![],
        max_page_size: 100,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![],
        actions: vec![],
        content: None,
    };
    let violations = validate(&contract(vec![resource]), &[delivery_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(
            &violations[0],
            Violation::UnreachableListing { table, bound, .. }
                if table == "webhook_delivery" && bound == &vec!["tenant_id".to_owned()]
        ),
        "{violations:?}",
    );
    let message = violations[0].to_string();
    assert!(message.contains("tenant_id"), "{message}");
    assert!(message.contains("no index leads"), "{message}");
}

/// The same table with the same unindexed pin is fine as a
/// sub-collection, because the parent key is bound on every read too
/// and an index leads with it: the seek narrows to one endpoint's rows
/// and the pin rides as a residual check. This is copal's real
/// contract, kept passing on purpose.
#[test]
fn a_parent_key_leading_an_index_carries_the_pins() {
    let mut parent = files_resource();
    parent.sub_resources = vec![janus::SubResource {
        name: "deliveries".into(),
        table: "webhook_delivery".into(),
        parent_key: "endpoint".into(),
        fields: vec![
            FieldExposure::column("state"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["state".into()],
        sortable: vec![],
        max_page_size: 100,
        description: None,
        graphql: None,
    }];
    assert_eq!(
        validate(&contract(vec![parent]), &[file_table(), delivery_table()]),
        vec![],
    );
}

/// A FULLTEXT index leading with the pinned column is the perverse
/// case: only the index-type predicate stands between it and being
/// credited as a seek.
#[test]
fn a_search_index_leading_with_the_pin_is_not_a_seek() {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    let table = table_schema("note")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("body")),
        ])
        .with_indexes([bm25_index("idx_tenant_text", ["tenant_id"], "copal_text")]);
    let resource = Resource {
        name: "notes".into(),
        table: "note".into(),
        filter_options: Default::default(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec!["tenant_id".into()],
        filterable: vec![],
        sortable: vec![],
        max_page_size: 100,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![],
        actions: vec![],
        content: None,
    };
    let violations = validate(&contract(vec![resource]), &[table]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(matches!(
        &violations[0],
        Violation::UnreachableListing { .. }
    ));
}

/// A pin the table does not have is an unknown column and nothing
/// more: the reachability question waits for a name that resolves.
#[test]
fn an_unknown_pin_is_reported_once_not_twice() {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    let table = table_schema("note")
        .with_mode(TableMode::Schemafull)
        .with_fields([built(string_field("body"))]);
    let resource = Resource {
        name: "notes".into(),
        table: "note".into(),
        filter_options: Default::default(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec!["tenant".into()],
        filterable: vec![],
        sortable: vec![],
        max_page_size: 100,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![],
        actions: vec![],
        content: None,
    };
    let violations = validate(&contract(vec![resource]), &[table]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(matches!(
        &violations[0],
        Violation::UnknownColumn { column, .. } if column == "tenant"
    ));
}

#[test]
fn unknown_names_and_collisions_are_each_reported() {
    let resource = Resource {
        name: "files".into(),
        table: "file".into(),
        filter_options: Default::default(),
        fields: vec![
            FieldExposure::column("no_such_column"),
            FieldExposure::renamed("path", "state"),
            FieldExposure::column("state"),
        ],
        pinned: vec![],
        filterable: vec!["also_missing".into()],
        sortable: vec![],
        max_page_size: 10,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![],
        actions: vec![],
        content: None,
    };
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("no_such_column"), "{text}");
    assert!(text.contains("also_missing"), "{text}");
    assert!(text.contains("rename collision"), "{text}");

    let missing_table = Resource {
        table: "nonexistent".into(),
        ..files_resource()
    };
    let violations = validate(&contract(vec![missing_table]), &[file_table()]);
    assert!(
        matches!(&violations[0], Violation::UnknownTable { table, .. } if table == "nonexistent")
    );
}

#[test]
fn generated_openapi_matches_the_golden_document() {
    let doc = generate_openapi(&contract(vec![files_resource()]), &[file_table()]).unwrap();
    let rendered = serde_json::to_string_pretty(&doc).unwrap();

    let golden_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/copal_files.json");
    if std::env::var("JANUS_BLESS").is_ok() {
        std::fs::write(golden_path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(golden_path)
        .expect("golden file missing; run with JANUS_BLESS=1 to create");
    assert_eq!(
        rendered.trim(),
        golden.trim(),
        "generated OpenAPI drifted from golden; JANUS_BLESS=1 to re-bless deliberately",
    );
}

#[test]
fn nullable_columns_become_type_unions_and_leave_required() {
    let doc = generate_openapi(&contract(vec![files_resource()]), &[file_table()]).unwrap();
    let file_schema = &doc["components"]["schemas"]["File"];
    // option<int> -> ["integer", "null"] and absent from required.
    assert_eq!(
        file_schema["properties"]["size"]["type"],
        serde_json::json!(["integer", "null"])
    );
    let required: Vec<_> = file_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!required.contains(&"size"));
    assert!(required.contains(&"path"));
    // The rename is the API name everywhere.
    assert!(file_schema["properties"].get("size_bytes").is_none());
}

#[test]
fn chosen_names_are_gated_against_surrealdb_reserved_words() {
    // A rename onto a reserved word refuses.
    let mut resource = files_resource();
    resource.fields[4] = FieldExposure::renamed("digest", "value");
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("\"value\"") && text.contains("reserved"),
        "{text}",
    );

    // GraphQL overrides are gated for grammar, __, root names, and
    // reserved words.
    let mut resource = files_resource();
    resource.graphql = Some(janus::GraphqlNames {
        type_name: Some("Query".into()),
        list_field: Some("__files".into()),
        get_field: Some("select".into()),
        watch_field: None,
    });
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("root type name"), "{text}");
    assert!(text.contains("__ prefix"), "{text}");
    assert!(text.contains("reserved"), "{text}");

    // Two resources landing on one effective GraphQL type name refuse.
    let mut second = files_resource();
    second.name = "file".into();
    let violations = validate(&contract(vec![files_resource(), second]), &[file_table()]);
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("collides with the GraphQL type name"),
        "{text}"
    );

    // The reserved gate is exported for schema layers to reuse.
    assert!(janus::is_reserved("SELECT"));
    assert!(janus::is_reserved("$auth"));
    assert!(!janus::is_reserved("tenant_id"));
}

/// Two resources cannot share a name.
///
/// Both would claim the same REST prefix and the same GraphQL field,
/// and whichever the generator wrote second would win silently. The
/// contract already refuses this for queries, rate classes, actions
/// and sub-collections; resources were the one level that let it
/// through, which a contract assembled from several files makes easy
/// to do by accident.
#[test]
fn two_resources_cannot_share_a_name() {
    let violations = validate(
        &contract(vec![files_resource(), files_resource()]),
        &[file_table()],
    );
    assert!(
        violations.iter().any(|v| matches!(
            v,
            Violation::InvalidName { name, problem, .. }
                if name == "files" && problem.contains("duplicate")
        )),
        "the collision is named: {violations:?}",
    );
}

/// A stored string cannot panic the page that renders it.
///
/// `shorten_timestamp` checked bytes 4, 7, 10 and 13 looked like a
/// timestamp and then sliced to byte 19. Nothing constrained bytes 14
/// through 19, so a value that passed the shape check and carried a
/// multi-byte character across byte 19 was sliced through the middle
/// of it. Field values are caller-controlled: a file path or a
/// metadata string of that shape took down the listing that showed
/// it.
#[test]
fn a_crafted_field_value_does_not_panic_the_cell() {
    // Passes every shape check, and '€' occupies bytes 17, 18 and 19.
    let crafted = "2026-08-05T19:00x\u{20ac}Z";
    assert!(crafted.len() >= 20);
    for value in [
        crafted.to_owned(),
        "2026-08-05T19:29:23Z".to_owned(),
        "2026-08-05T19:29:2\u{20ac}Z".to_owned(),
        "\u{20ac}\u{20ac}\u{20ac}\u{20ac}\u{20ac}\u{20ac}\u{20ac}Z".to_owned(),
        "not a timestamp at all".to_owned(),
    ] {
        let json = serde_json::Value::String(value.clone());
        // Rendering at all is the assertion.
        let _ = janus::runtime::cell("created_at", Some(&json));
    }

    // The ordinary value still shortens the way it did.
    let ordinary = serde_json::Value::String("2026-08-05T19:29:23.394140700Z".to_owned());
    let rendered = janus::runtime::cell("created_at", Some(&ordinary)).into_string();
    assert!(
        rendered.contains("2026-08-05 19:29:23"),
        "the shortened form survives: {rendered}",
    );
}

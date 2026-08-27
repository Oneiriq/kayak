//! What a FULLTEXT or vector index can and cannot answer, both ways.
//!
//! A filter or a sort resting on search machinery, and a search
//! resting on a b-tree, are one question asked from two sides, so the
//! cases sit together: the same `text_chunk` fixture answers both, and
//! a rule that drifts shows up here as a pair of tests disagreeing
//! rather than as one passing somewhere else.
//!
//! Split out of `contract_gate.rs` when the declared-search rules
//! carried it past a thousand lines. The fixtures are local because a
//! test binary is its own crate; nothing here is shared, and nothing
//! here needs to be.

use kayak::{
    generate_openapi, validate, ActionField, Contract, FieldExposure, Query, Resource,
    SearchBacking, SearchKind, TypeRef, Violation,
};
use surql::schema::{
    array_field, bm25_index, hnsw_index, index, int_field, mtree_index, string_field, table_schema,
    unique_index, HnswDistanceType, IndexType, MTreeDistanceType, MTreeVectorType, TableDefinition,
    TableMode,
};

/// The listing template the index-type cases vary one claim at a time
/// from: enough of a resource to be well-formed, so the only thing a
/// case changes is the thing under test.
fn chunk_resource() -> Resource {
    Resource {
        name: "chunks".into(),
        table: "text_chunk".into(),
        identity: Default::default(),
        filter_options: Default::default(),
        faces: Default::default(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec!["tenant_id".into()],
        pinned_either: vec![],
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
    }
}

fn contract(resources: Vec<Resource>) -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: Default::default(),
        resources,
        queries: vec![],
    }
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
        identity: Default::default(),
        fields: vec![
            FieldExposure::column("body"),
            FieldExposure::column("embedding"),
            FieldExposure::column("locator"),
        ],
        pinned: vec!["tenant_id".into()],
        pinned_either: vec![],
        filterable: vec!["body".into(), "embedding".into(), "locator".into()],
        sortable: vec!["body".into(), "embedding".into(), "locator".into()],
        ..chunk_resource()
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
    // bug in kayak, so the message names the index they were looking at
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
            identity: Default::default(),
            fields: vec![FieldExposure::column("body")],
            pinned: vec!["tenant_id".into()],
            pinned_either: vec![],
            filterable: vec!["body".into()],
            sortable: vec![],
            ..chunk_resource()
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
        identity: Default::default(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec!["tenant_id".into()],
        pinned_either: vec![],
        filterable: vec!["body".into()],
        sortable: vec!["body".into()],
        ..chunk_resource()
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
        identity: Default::default(),
        fields: vec![FieldExposure::column("body")],
        pinned: vec![],
        pinned_either: vec![],
        filterable: vec![],
        sortable: vec!["body".into()],
        ..chunk_resource()
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
        dimension: None,
        optional: false,
    }
}

/// A query holding `backing`, on a contract with no resources: the
/// backing rules do not care what else the contract exposes.
///
/// The declared searches follow the backings, because these cases are
/// about whether a backing holds up and a query that declares a
/// search it has no backing for is refused before any of them is
/// reached. The cases that ARE about the declaration write it
/// themselves.
fn searching(backing: Vec<SearchBacking>) -> Contract {
    let searches = backing
        .iter()
        .map(|b: &SearchBacking| b.kind)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
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
        searches,
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
/// resolver -- so nothing stopped a schema change from dropping
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
        searches: vec![],
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
/// teaches the same way -- name the index the author was looking at,
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

    // A DISKANN index is the third machinery that answers a vector
    // backing, new with surql 0.33; a Lexical claim on one is still
    // refused. The table gains the index only inside this test so the
    // other cases keep exercising the HNSW shape copal actually ships.
    let mut with_diskann = text_chunk_table();
    with_diskann.indexes.push(surql::schema::diskann_index(
        "idx_chunk_diskann",
        "embedding",
        768,
        surql::schema::DiskAnnDistanceType::Cosine,
        surql::schema::MTreeVectorType::F16,
    ));
    let vector = searching(vec![backing(
        "embedding",
        "idx_chunk_diskann",
        SearchKind::Vector,
    )]);
    assert_eq!(
        validate(&vector, &[with_diskann.clone()]),
        vec![],
        "a vector backing rests on DISKANN the way it rests on HNSW",
    );
    let lexical = searching(vec![backing(
        "embedding",
        "idx_chunk_diskann",
        SearchKind::Lexical,
    )]);
    let violations = validate(&lexical, &[with_diskann]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(
            &violations[0],
            Violation::WrongBackingIndexType { index, .. } if index == "idx_chunk_diskann"
        ),
        "{violations:?}",
    );

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
        text.contains("a vector backing needs an HNSW, MTREE, or DISKANN one"),
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

/// The gate: a search the contract says it performs must have
/// something behind it.
///
/// Every other rule here catches a claim held against the schema and
/// found wanting. This one catches an absence, and an absence is what
/// unindexed search looks like from the outside: driftnet's chunk
/// search and antumbra's recall paths both ran for months over
/// columns nothing could answer a neighbor query on, and no contract
/// anywhere could have said so, because saying nothing about search
/// and needing nothing were the same declaration. They stop being the
/// same here.
#[test]
fn a_declared_search_with_nothing_behind_it_is_refused() {
    let mut promised = searching(vec![]);
    promised.queries[0].searches = vec![SearchKind::Vector];
    let violations = validate(&promised, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(
            &violations[0],
            Violation::UnbackedSearch { query, kind }
                if query == "search" && *kind == SearchKind::Vector
        ),
        "{violations:?}",
    );
    // The message names the repair, both halves of it.
    let text = violations[0].to_string();
    assert!(text.contains("performs a vector search"), "{text}");
    assert!(text.contains("names no vector backing"), "{text}");

    // A lexical backing does not answer a vector search: the kinds
    // are matched, not counted.
    let mut crossed = searching(vec![backing("body", "idx_chunk_body", SearchKind::Lexical)]);
    crossed.queries[0].searches = vec![SearchKind::Vector];
    let violations = validate(&crossed, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(&violations[0], Violation::UnbackedSearch { kind, .. } if *kind == SearchKind::Vector),
        "{violations:?}",
    );

    // And the same declaration over the machinery that answers it is
    // clean, which is the whole repair: name the index.
    let mut backed = searching(vec![backing(
        "embedding",
        "idx_chunk_embedding",
        SearchKind::Vector,
    )]);
    backed.queries[0].searches = vec![SearchKind::Vector];
    assert_eq!(validate(&backed, &[text_chunk_table()]), vec![]);

    // Declaring the same search twice says one thing twice.
    let mut doubled = backed.clone();
    doubled.queries[0].searches = vec![SearchKind::Vector, SearchKind::Vector];
    let violations = validate(&doubled, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].to_string().contains("same search twice"),
        "{violations:?}",
    );
}

/// A vector search sends vectors of one width, and the index answers
/// at one width, and they have to be the same width.
///
/// This is the check that survives a model swap. Changing the
/// embedding model changes the width, the schema has to follow, and
/// where the contract pins what the search sends, a schema that did
/// not follow is a generation failure rather than a quiet change in
/// what comes back.
#[test]
fn a_vector_backing_answers_for_its_width() {
    let pinned = |dimension: Option<u32>| {
        let mut contract = searching(vec![backing(
            "embedding",
            "idx_chunk_embedding",
            SearchKind::Vector,
        )]);
        contract.queries[0].backing[0].dimension = dimension;
        contract
    };
    // The fixture's HNSW index is 768 wide.
    assert_eq!(validate(&pinned(Some(768)), &[text_chunk_table()]), vec![]);
    assert_eq!(validate(&pinned(None), &[text_chunk_table()]), vec![]);

    let violations = validate(&pinned(Some(1536)), &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(
            &violations[0],
            Violation::BackingWidthMismatch { declared, actual, index, .. }
                if *declared == 1536 && *actual == Some(768) && index == "idx_chunk_embedding"
        ),
        "{violations:?}",
    );
    let text = violations[0].to_string();
    assert!(text.contains("searches at 1536 dimensions"), "{text}");
    assert!(text.contains("defined over 768"), "{text}");

    // A width on a lexical backing is not a wrong number, it is a
    // number about nothing, and it is refused as one rather than
    // reported as a mismatch against an index that states no width.
    let mut lexical = searching(vec![backing("body", "idx_chunk_body", SearchKind::Lexical)]);
    lexical.queries[0].backing[0].dimension = Some(768);
    let violations = validate(&lexical, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0]
            .to_string()
            .contains("a lexical backing has no vector width"),
        "{violations:?}",
    );
}

/// Machinery a deployment configures can be declared without the
/// contract lying about deployments that did not.
///
/// Copal's HNSW index over `text_chunk.embedding` exists only where an
/// embedding model is configured, applied at startup at that model's
/// width. Declared outright it would be false everywhere else, so it
/// was declared nowhere -- and a search nothing in the contract
/// mentions is exactly what this rulebook exists to end. Optional is
/// the third answer: the index may be absent, and an index that is
/// there answers for its column, its kind, and its width like any
/// other. May be absent, never may be wrong.
#[test]
fn an_optional_backing_may_be_absent_but_not_wrong() {
    let optional = |column: &str, index: &str| {
        let mut contract = searching(vec![backing(column, index, SearchKind::Vector)]);
        contract.queries[0].backing[0].optional = true;
        contract
    };

    // Absent from the schema entirely: nothing to refuse.
    assert_eq!(
        validate(
            &optional("embedding", "idx_not_configured"),
            &[text_chunk_table()]
        ),
        vec![],
    );
    // The same backing required is the refusal it always was, so the
    // flag is doing the work and not the name.
    let violations = validate(
        &searching(vec![backing(
            "embedding",
            "idx_not_configured",
            SearchKind::Vector,
        )]),
        &[text_chunk_table()],
    );
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(&violations[0], Violation::UnknownBackingIndex { index, .. } if index == "idx_not_configured"),
        "{violations:?}",
    );

    // Present, and the wrong kind: optional buys nothing here.
    let violations = validate(&optional("body", "idx_chunk_body"), &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(&violations[0], Violation::WrongBackingIndexType { index, .. } if index == "idx_chunk_body"),
        "{violations:?}",
    );

    // Present, right kind, wrong width: still refused.
    let mut mismatched = optional("embedding", "idx_chunk_embedding");
    mismatched.queries[0].backing[0].dimension = Some(1536);
    let violations = validate(&mismatched, &[text_chunk_table()]);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        matches!(&violations[0], Violation::BackingWidthMismatch { .. }),
        "{violations:?}",
    );

    // And an optional backing answers a declared search: the
    // declaration is about what the query does, not about what every
    // deployment has.
    let mut declared = optional("embedding", "idx_not_configured");
    declared.queries[0].searches = vec![SearchKind::Vector];
    assert_eq!(validate(&declared, &[text_chunk_table()]), vec![]);
}

//! The gate and the generator, driven by a schema shaped like Copal's
//! `file` table (built inline: Janus takes schema definitions as input
//! and depends on no consumer).
//!
//! What an index can ANSWER — a filter or a sort resting on search
//! machinery, a search resting on a b-tree, a declared search resting
//! on nothing — lives next door in `search_gate.rs`.

use janus::{generate_openapi, validate, Contract, FieldExposure, Resource, Violation};
use surql::schema::{
    bm25_index, datetime_field, index, int_field, string_field, table_schema, unique_index,
    TableDefinition, TableMode,
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

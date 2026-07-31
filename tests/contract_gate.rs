//! The gate and the generator, driven by a schema shaped like Copal's
//! `file` table (built inline: Janus takes schema definitions as input
//! and depends on no consumer).

use janus::{generate_openapi, validate, Contract, FieldExposure, Resource, Violation};
use surql::schema::{
    datetime_field, index, int_field, string_field, table_schema, unique_index, TableDefinition,
    TableMode,
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
        actions: vec![],
    }
}

fn contract(resources: Vec<Resource>) -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        resources,
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

#[test]
fn unknown_names_and_collisions_are_each_reported() {
    let resource = Resource {
        name: "files".into(),
        table: "file".into(),
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
        actions: vec![],
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

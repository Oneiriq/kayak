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
            index("idx_listing", ["tenant_id", "state", "created_at"]),
            index("idx_created", ["created_at"]),
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
        filterable: vec!["state".into()],
        sortable: vec!["created_at".into()],
        max_page_size: 100,
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
fn non_leading_index_membership_filters_but_does_not_sort() {
    let mut resource = files_resource();
    // `state` is in idx_listing but not its leading column: filterable
    // yes, sortable no.
    resource.sortable.push("state".into());
    let violations = validate(&contract(vec![resource]), &[file_table()]);
    assert!(matches!(&violations[0], Violation::UnindexedSort { column, .. } if column == "state"));
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
        filterable: vec!["also_missing".into()],
        sortable: vec![],
        max_page_size: 10,
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
        .expect("golden file missing — run with JANUS_BLESS=1 to create");
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

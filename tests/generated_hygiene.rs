//! Generated code has to build clean in someone else's crate.
//!
//! A consumer cannot edit a generated file to silence a warning, and
//! a workspace with `-D warnings` cannot build at all. So an import
//! the file does not use is not cosmetic -- it is the generator
//! handing over something that fails a build the consumer controls.

use janus::clients::{generate_client_rs, generate_client_rs_blocking};
use janus::{Contract, FieldExposure, Resource};
use surql::schema::{object_field, string_field, table_schema, TableDefinition, TableMode};

fn table(open: bool) -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    let mut fields = vec![built(string_field("path"))];
    if open {
        fields.push(built(object_field("metadata")));
    }
    table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields(fields)
}

fn contract(open: bool) -> Contract {
    let mut fields = vec![FieldExposure::column("path")];
    if open {
        fields.push(FieldExposure::column("metadata"));
    }
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: janus::AuthScheme::None,
        resources: vec![Resource {
            name: "files".into(),
            table: "file".into(),
            fields,
            pinned: vec![],
            filterable: vec![],
            sortable: vec![],
            max_page_size: 100,
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
            faces: Default::default(),
            actions: vec![],
            content: None,
            filter_options: Default::default(),
        }],
        queries: vec![],
    }
}

#[test]
fn the_rust_client_imports_value_only_when_it_names_one() {
    // Nothing here is open JSON: no queries, no actions, every column
    // a scalar. Importing Value anyway is an unused-import warning in
    // the consumer's build.
    let plain = contract(false);
    let plain_schema = vec![table(false)];
    for client in [
        generate_client_rs(&plain, &plain_schema).unwrap(),
        generate_client_rs_blocking(&plain, &plain_schema).unwrap(),
    ] {
        assert!(
            !client.contains("use serde_json::Value;"),
            "nothing in this contract names Value:\n{client}",
        );
        // And the header does not tell the caller to add a dependency
        // the file never reaches for.
        assert!(!client.contains(", serde_json."), "{client}");
    }

    // An object column is enough to need it back.
    let open = contract(true);
    let open_schema = vec![table(true)];
    for client in [
        generate_client_rs(&open, &open_schema).unwrap(),
        generate_client_rs_blocking(&open, &open_schema).unwrap(),
    ] {
        assert!(client.contains("use serde_json::Value;"), "{client}");
        assert!(client.contains(", serde_json."), "{client}");
        assert!(client.contains("pub metadata: Value,"), "{client}");
    }
}

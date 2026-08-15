//! Names a contract can choose that the generated clients cannot spell.
//!
//! Three of these produced source that does not compile, silently, from
//! contracts the validator accepted: a column named for a Rust keyword,
//! a column named `id` beside the one every resource already carries,
//! and an action deriving a method name a listing or getter had taken.
//!
//! Two different answers, chosen by who owns the name. A column named
//! `type` is ordinary in a schema and ordinary on the wire, and the
//! other three clients spell it plainly -- so Rust escapes it rather
//! than the contract being refused over a limitation of one language.
//! A duplicate method or a second `id` is a contract that cannot be
//! served by any client, so it is refused.

use janus::clients::{
    generate_client_go, generate_client_py, generate_client_rs, generate_client_rs_blocking,
    generate_client_ts,
};
use janus::validate::{validate, Violation};
use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, TypeRef};
use surql::schema::{string_field, table_schema, TableDefinition, TableMode};

fn table(columns: &[&str]) -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields(columns.iter().map(|c| built(string_field(*c))))
}

fn contract_exposing(columns: &[&str]) -> Contract {
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
            fields: columns.iter().map(|c| FieldExposure::column(*c)).collect(),
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
fn a_column_named_for_a_rust_keyword_is_escaped_not_refused() {
    let columns = ["type", "match", "move", "path"];
    let contract = contract_exposing(&columns);
    let schema = vec![table(&columns)];
    assert_eq!(
        validate(&contract, &schema),
        vec![],
        "nothing is wrong here"
    );

    for client in [
        generate_client_rs(&contract, &schema).unwrap(),
        generate_client_rs_blocking(&contract, &schema).unwrap(),
    ] {
        for keyword in ["type", "match", "move"] {
            assert!(
                client.contains(&format!("pub r#{keyword}: String,")),
                "expected a raw identifier for {keyword} in:\n{client}",
            );
            assert!(
                !client.contains(&format!("pub {keyword}: String,")),
                "and never the bare keyword, which does not parse",
            );
        }
        assert!(
            client.contains("pub path: String,"),
            "ordinary names are untouched"
        );
    }
}

#[test]
fn a_keyword_no_raw_identifier_can_rescue_is_renamed() {
    // `r#self` is rejected by the compiler, so the field takes a suffix
    // and serde is told the name it answers to on the wire.
    let columns = ["self", "path"];
    let contract = contract_exposing(&columns);
    let schema = vec![table(&columns)];
    assert_eq!(validate(&contract, &schema), vec![]);

    let client = generate_client_rs(&contract, &schema).unwrap();
    assert!(client.contains("#[serde(rename = \"self\")]"), "{client}");
    assert!(client.contains("pub self_: String,"), "{client}");
    assert!(!client.contains("r#self"), "which the compiler rejects");
}

#[test]
fn the_other_three_clients_spell_a_keyword_column_plainly() {
    // The escaping is Rust's problem alone. A contract does not get a
    // different wire name in Python because Rust cannot say `type`.
    let columns = ["type", "path"];
    let contract = contract_exposing(&columns);
    let schema = vec![table(&columns)];

    let ts = generate_client_ts(&contract, &schema).unwrap();
    let py = generate_client_py(&contract, &schema).unwrap();
    let go = generate_client_go(&contract, &schema).unwrap();
    assert!(ts.contains("type:"), "{ts}");
    assert!(py.contains("type"), "{py}");
    // Go exports as Type and keeps the wire name in the tag.
    assert!(go.contains("`json:\"type\"`"), "{go}");
    for client in [&ts, &py, &go] {
        assert!(!client.contains("r#type"), "no Rust escaping leaked out");
    }
}

#[test]
fn a_column_named_id_is_refused() {
    // Every generated resource type carries an `id` the contract never
    // declares, so this one would be the second field of that name.
    let columns = ["id", "path"];
    let contract = contract_exposing(&columns);
    let schema = vec![table(&columns)];

    let violations = validate(&contract, &schema);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::ShadowsId { .. })),
        "{violations:?}",
    );
}

#[test]
fn an_action_that_takes_a_generated_method_name_is_refused() {
    let columns = ["path"];
    let mut contract = contract_exposing(&columns);
    // `get` on `files` derives `get_file`, which the generated getter
    // already holds. Four clients would emit the method twice.
    contract.resources[0].actions = vec![Action {
        name: "get".into(),
        method: "POST".into(),
        path: "/{id}/get".into(),
        input: vec![],
        output: ActionOutput::Json,
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    }];
    let schema = vec![table(&columns)];

    let violations = validate(&contract, &schema);
    let collision = violations
        .iter()
        .find(|v| matches!(v, Violation::DuplicateMethod { .. }))
        .unwrap_or_else(|| panic!("{violations:?}"));
    let rendered = collision.to_string();
    assert!(rendered.contains("get_file"), "{rendered}");
    // Both sides are named, because knowing only one does not tell the
    // author which of the two to rename.
    assert!(rendered.contains("getter"), "{rendered}");
    assert!(rendered.contains("action get"), "{rendered}");
}

#[test]
fn a_query_colliding_with_a_resource_method_is_refused() {
    // Queries land on the same Client as everything else.
    let columns = ["path"];
    let mut contract = contract_exposing(&columns);
    contract.queries = vec![Query {
        name: "list_files".into(),
        path: "/v1/list-files".into(),
        input: vec![],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![],
        backing: vec![],
    }];
    let schema = vec![table(&columns)];

    let violations = validate(&contract, &schema);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v, Violation::DuplicateMethod { .. })),
        "{violations:?}",
    );
}

#[test]
fn a_query_parameter_cannot_shadow_the_methods_own_locals() {
    // The generated body binds `url` and `request`. A parameter of
    // either name used to shadow them, so the request was built from
    // the caller's string and the query was attached to nothing.
    let columns = ["path"];
    let mut contract = contract_exposing(&columns);
    contract.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: ["url", "request", "type", "q"]
            .iter()
            .map(|name| ActionField {
                name: (*name).into(),
                kind: TypeRef::String,
                required: true,
                multiple: false,
                description: None,
                options: Vec::new(),
            })
            .collect(),
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![],
        backing: vec![],
    }];
    let schema = vec![table(&columns)];
    assert_eq!(validate(&contract, &schema), vec![]);

    let client = generate_client_rs(&contract, &schema).unwrap();
    // The wire names are untouched...
    for wire in ["url", "request", "type", "q"] {
        assert!(
            client.contains(&format!("(\"{wire}\", ")),
            "wire name {wire} missing from:\n{client}",
        );
    }
    // ...while the bindings step aside.
    assert!(client.contains("url_: &str"), "{client}");
    assert!(client.contains("request_: &str"), "{client}");
    assert!(client.contains("r#type: &str"), "{client}");
    assert!(
        client.contains("q: &str"),
        "an uncontested name is left alone"
    );
    assert!(
        client.contains("let url = format!"),
        "the generator's own local keeps its name",
    );
}

/// Writes the keyword fixture to disk so the generated Rust can be
/// compiled by a real toolchain out of band. Ignored by default.
#[test]
#[ignore]
fn dump_keyword_fixture() {
    let columns = ["type", "match", "move", "self", "path"];
    let contract = contract_exposing(&columns);
    let schema = vec![table(&columns)];
    let dir = std::env::var("JANUS_DUMP_DIR").expect("set JANUS_DUMP_DIR");
    std::fs::write(
        format!("{dir}/schema.json"),
        serde_json::to_string_pretty(&schema).unwrap(),
    )
    .unwrap();
    std::fs::write(
        format!("{dir}/contract.json"),
        serde_json::to_string_pretty(&contract).unwrap(),
    )
    .unwrap();
}

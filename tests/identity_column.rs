//! A resource that does not name itself `id`.
//!
//! Every emitter used to write `id` unconditionally: the OpenAPI schema
//! and its required list, the Rust struct, the GraphQL type, the MCP
//! get-tool. That is right for a table with an `id` column and wrong for
//! one keyed by something else, and wrong in a specific way -- the
//! generated client declares a field the service never sends, so
//! deserializing a SUCCESSFUL response fails.
//!
//! It shipped. polyconsole's social service keys presence one row per
//! account and names it `user`; three of six generated structs could not
//! parse a 200 from the routes they described, in every artifact at
//! once, while the byte-comparison gate stayed green because the
//! generator was deterministically wrong.
//!
//! These tests hold the two halves: a resource that says nothing still
//! gets `id`, and a resource that names its identity gets that name
//! everywhere.

use janus::generate::generate_all;
use janus::validate::validate;
use janus::{AuthScheme, Contract, FieldExposure, Resource};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

/// A table keyed by `user` rather than `id`: one presence row per
/// account, which is exactly the shape that exposed the bug.
fn schema() -> Vec<TableDefinition> {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    vec![table_schema("presence")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("user")),
            built(string_field("status")),
            built(string_field("realm")),
        ])
        .with_indexes([
            index("presence_user_idx", ["user"]),
            index("presence_realm_idx", ["realm"]),
        ])]
}

fn contract(identity: Option<&str>) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: AuthScheme::None,
        resources: vec![Resource {
            name: "presences".into(),
            table: "presence".into(),
            identity: identity.map(str::to_owned),
            fields: vec![FieldExposure::column("status")],
            pinned: vec!["realm".into()],
            pinned_either: vec![],
            filterable: vec![],
            sortable: vec![],
            max_page_size: 50,
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
            actions: vec![],
            filter_options: Default::default(),
            faces: Default::default(),
            content: None,
        }],
        queries: vec![],
    }
}

/// Saying nothing keeps `id`, which is what every contract written
/// before this option existed said by saying nothing.
#[test]
fn a_resource_that_names_no_identity_still_carries_id() {
    let schema = schema();
    let contract = contract(None);
    assert!(validate(&contract, &schema).is_empty());

    let artifacts = generate_all(&contract, &schema, janus::generate::TARGETS).expect("generates");
    let openapi = artifacts
        .get("openapi.json")
        .expect("an OpenAPI document is generated");
    assert!(
        openapi.contains("\"id\""),
        "a resource with no declared identity should still describe an id",
    );
}

/// Naming the identity changes it in EVERY artifact, which is the point:
/// the bug was not that one emitter was wrong, it was that all of them
/// asked the same question and got the same wrong answer.
#[test]
fn a_named_identity_reaches_every_artifact() {
    let schema = schema();
    let contract = contract(Some("user"));
    assert!(
        validate(&contract, &schema).is_empty(),
        "{:?}",
        validate(&contract, &schema),
    );

    let artifacts = generate_all(&contract, &schema, janus::generate::TARGETS).expect("generates");
    for (name, body) in &artifacts {
        // The query surfaces and the engine policy do not describe a
        // resource's fields, so they have no identity to carry.
        if !body.contains("Presence") && !body.contains("presence") {
            continue;
        }
        assert!(
            body.contains("user"),
            "{name} does not mention the declared identity column",
        );
    }

    let openapi = artifacts.get("openapi.json").expect("an OpenAPI document");
    let doc: serde_json::Value = serde_json::from_str(openapi).expect("valid JSON");
    let presence = &doc["components"]["schemas"]["Presence"];
    assert!(
        presence["properties"].get("user").is_some(),
        "the schema should declare `user`: {presence}",
    );
    assert!(
        presence["properties"].get("id").is_none(),
        "and should NOT declare the `id` the table does not have: {presence}",
    );
    let required = presence["required"]
        .as_array()
        .expect("a required list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(
        required.contains(&"user") && !required.contains(&"id"),
        "required should name the real identity, got {required:?}",
    );

    // The PATH names the identity too, and the parameter declaration
    // matches the template -- a document whose path says one name and
    // whose parameter list says another is invalid OpenAPI.
    let get = &doc["paths"]["/v1/presences/{user}"];
    assert!(
        get.get("get").is_some(),
        "the by-instance path should be /v1/presences/{{user}}; paths: {:?}",
        doc["paths"]
            .as_object()
            .map(|m| m.keys().collect::<Vec<_>>()),
    );
    assert_eq!(
        get["get"]["parameters"][0]["name"], "user",
        "the path parameter declaration must match the template",
    );

    // And the GraphQL get query argument, which a caller names at the
    // call site.
    let sdl = artifacts.get("schema.graphql").expect("an SDL");
    assert!(
        sdl.contains("presence(user: ID!)"),
        "the SDL get field should take `user`, got:\n{sdl}",
    );
}

/// An identity that is not a column would emit a field the service can
/// never send -- in every artifact at once, which is the failure the
/// whole option exists to prevent. It is refused at validation rather
/// than discovered by a client that cannot parse a 200.
#[test]
fn an_identity_that_is_not_a_column_is_refused() {
    let schema = schema();
    let contract = contract(Some("nonesuch"));
    let violations = validate(&contract, &schema);
    assert!(
        violations.iter().any(|v| {
            let rendered = v.to_string();
            rendered.contains("nonesuch") && rendered.contains("presence")
        }),
        "expected a violation naming the missing column, got {violations:?}",
    );
}

//! How a resource's rows name themselves: `id`, another column, or
//! not at all.
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
//! Naming a column was the first fix, and it could not describe the
//! third shape: rows with no identity at all. An invite listing is
//! `{account, token_hash, expires_at}` -- nothing on that wire addresses
//! one row, and a contract forced to pick a column would be lying the
//! same way `id` was. These tests hold all three states: silence keeps
//! `id`, a named column reaches every artifact, and named nothing
//! synthesises nothing while refusing every face that needs to address
//! an instance.

use kayak::generate::generate_all;
use kayak::validate::validate;
use kayak::{AuthScheme, Contract, FieldExposure, Identity, Resource, ResourceFaces};
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

fn contract(identity: Identity) -> Contract {
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
            identity,
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
    let contract = contract(Identity::Id);
    assert!(validate(&contract, &schema).is_empty());

    let artifacts = generate_all(&contract, &schema, kayak::generate::TARGETS).expect("generates");
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
    let contract = contract(Identity::Column("user".into()));
    assert!(
        validate(&contract, &schema).is_empty(),
        "{:?}",
        validate(&contract, &schema),
    );

    let artifacts = generate_all(&contract, &schema, kayak::generate::TARGETS).expect("generates");
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
    let contract = contract(Identity::Column("nonesuch".into()));
    let violations = validate(&contract, &schema);
    assert!(
        violations.iter().any(|v| {
            let rendered = v.to_string();
            rendered.contains("nonesuch") && rendered.contains("presence")
        }),
        "expected a violation naming the missing column, got {violations:?}",
    );
}

/// Rows that carry no identity get no synthesised field, in any
/// artifact -- the same phantom-`id` failure as a misnamed identity,
/// prevented the same way.
#[test]
fn rows_that_carry_no_identity_get_no_synthesised_field() {
    let schema = schema();
    let mut contract = contract(Identity::Absent);
    contract.resources[0].faces = ResourceFaces::LIST_ONLY;
    let violations = validate(&contract, &schema);
    assert!(violations.is_empty(), "{violations:?}");

    let artifacts = generate_all(&contract, &schema, kayak::generate::TARGETS).expect("generates");

    let openapi = artifacts.get("openapi.json").expect("an OpenAPI document");
    let doc: serde_json::Value = serde_json::from_str(openapi).expect("valid JSON");
    let presence = &doc["components"]["schemas"]["Presence"];
    assert!(
        presence["properties"].get("id").is_none(),
        "no id should be described: {presence}",
    );
    let required = presence["required"]
        .as_array()
        .expect("a required list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(
        !required.contains(&"id"),
        "required should not demand a field the service never sends, got {required:?}",
    );

    let rust = artifacts.get("client.rs").expect("a Rust client");
    assert!(
        !rust.contains("pub id:"),
        "the Rust struct should carry no id field:\n{rust}",
    );
    let sdl = artifacts.get("schema.graphql").expect("an SDL");
    assert!(
        !sdl.contains("id: ID!"),
        "the GraphQL type should carry no id field:\n{sdl}",
    );
    let mcp = artifacts.get("mcp-tools.json").expect("an MCP manifest");
    assert!(
        mcp.contains("presences_list") && !mcp.contains("presence_get"),
        "only the listing should become a tool:\n{mcp}",
    );
}

/// Every face that addresses one instance is refused when the rows
/// carry nothing to address them by -- at validation, where the
/// contradiction is visible, not in a generator that would have to
/// invent a path parameter out of nothing.
#[test]
fn addressing_rows_that_carry_no_identity_is_refused() {
    let schema = schema();
    let contract = contract(Identity::Absent);
    // The default faces include get: addressing one presence by...
    // nothing. There is no path template, no parameter name, no wire
    // field for a client to read the answer's identity from.
    let violations = validate(&contract, &schema);
    assert!(
        violations.iter().any(|v| {
            let rendered = v.to_string();
            rendered.contains("presences") && rendered.contains("no identity")
        }),
        "expected the get face refused for want of an identity, got {violations:?}",
    );
}

/// The three states on the contract document: silence, a string, and
/// an explicit null are all distinct, and each survives the round trip.
#[test]
fn the_identity_states_round_trip_through_the_contract_document() {
    let cases = [
        (Identity::Id, None),
        (
            Identity::Column("user".into()),
            Some(serde_json::json!("user")),
        ),
        (Identity::Absent, Some(serde_json::Value::Null)),
    ];
    for (identity, rendered) in cases {
        let document = serde_json::to_value(contract(identity.clone())).expect("serializes");
        assert_eq!(
            document["resources"][0].get("identity").cloned(),
            rendered,
            "{identity:?} should render as {rendered:?}",
        );
        let parsed: Contract = serde_json::from_value(document).expect("parses");
        assert_eq!(
            parsed.resources[0].identity, identity,
            "{identity:?} should survive the round trip",
        );
    }

    // And the spelled-out default reads back as the default, so a
    // contract that says `"identity": "id"` out loud is not a third
    // state pretending to be a fourth.
    let mut document = serde_json::to_value(contract(Identity::Id)).expect("serializes");
    document["resources"][0]["identity"] = serde_json::json!("id");
    let parsed: Contract = serde_json::from_value(document).expect("parses");
    assert_eq!(parsed.resources[0].identity, Identity::Id);
}

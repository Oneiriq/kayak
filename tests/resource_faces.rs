//! Which collection faces a resource exposes.
//!
//! Both, before this existed, which misdescribes a resource whose
//! collection is deliberately not browsable. polyconsole-social serves
//! `GET /accounts/{id}` and must never serve `GET /accounts`, because
//! enumerating every user is the thing a social service is careful not
//! to do -- and it hangs a keys sub-resource off that same resource, so
//! leaving `accounts` undeclared costs the sub-resource too.
//!
//! The other shape is a domain that is all verbs: `friends` is eight
//! two-account RPCs over a table whose rows are unordered pairs. There
//! is no listing and no getter to declare, and a resource with neither
//! is still where those actions belong.

use janus::diff::{diff, Change};
use janus::generate::generate_all;
use janus::validate::validate;
use janus::{Action, ActionOutput, Contract, FieldExposure, Resource, ResourceFaces, SubResource};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

fn schema() -> Vec<TableDefinition> {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    vec![
        table_schema("account")
            .with_mode(TableMode::Schemafull)
            .with_fields([built(string_field("handle")), built(string_field("realm"))])
            .with_indexes([index("account_realm_idx", ["realm"])]),
        table_schema("account_key")
            .with_mode(TableMode::Schemafull)
            .with_fields([built(string_field("user")), built(string_field("pubkey"))])
            .with_indexes([index("account_key_user_idx", ["user"])]),
    ]
}

fn accounts(faces: ResourceFaces) -> Resource {
    Resource {
        name: "accounts".into(),
        table: "account".into(),
        fields: vec![FieldExposure::column("handle")],
        pinned: vec![],
        // Only meaningful with a listing, so the callers below that
        // turn the listing off clear these too.
        filterable: if faces.list {
            vec!["realm".into()]
        } else {
            vec![]
        },
        sortable: vec![],
        max_page_size: 50,
        graphql: None,
        watchable: false,
        reads_require: vec![],
        rate_class: None,
        sub_resources: vec![SubResource {
            name: "keys".into(),
            table: "account_key".into(),
            parent_key: "user".into(),
            fields: vec![FieldExposure::column("pubkey")],
            pinned: vec![],
            filterable: vec![],
            sortable: vec![],
            max_page_size: 50,
            description: None,
            graphql: None,
        }],
        actions: vec![Action {
            name: "suspend".into(),
            method: "POST".into(),
            path: "/{id}/suspend".into(),
            input: vec![],
            output: ActionOutput::None,
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        }],
        content: None,
        filter_options: Default::default(),
        faces,
    }
}

fn contract(faces: ResourceFaces) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: String::new(),
        limits: None,
        rate_classes: vec![],
        auth: janus::AuthScheme::None,
        resources: vec![accounts(faces)],
        queries: vec![],
    }
}

fn artifacts(faces: ResourceFaces) -> std::collections::BTreeMap<String, String> {
    generate_all(
        &contract(faces),
        &schema(),
        &[
            "openapi",
            "sdl",
            "mcp",
            "client-rs",
            "client-ts",
            "client-py",
            "client-go",
        ],
    )
    .expect("the contract generates")
}

#[test]
fn a_resource_that_says_nothing_exposes_both_faces() {
    assert_eq!(ResourceFaces::default(), ResourceFaces::ALL);
    let generated = artifacts(ResourceFaces::default());
    assert!(generated["openapi.json"].contains("\"/accounts\""));
    assert!(generated["openapi.json"].contains("\"/accounts/{id}\""));
    assert!(generated["client.rs"].contains("fn list_accounts"));
    assert!(generated["client.rs"].contains("fn get_account"));
}

#[test]
fn a_get_only_resource_is_reachable_by_id_and_never_enumerable() {
    let generated = artifacts(ResourceFaces::GET_ONLY);
    assert_eq!(
        validate(&contract(ResourceFaces::GET_ONLY), &schema()),
        vec![]
    );

    // The collection path is gone from every face, and the by-id path
    // survives in every face. A gate that reached six generators and
    // missed the seventh would leave one artifact promising the
    // enumeration the contract just refused.
    let document: serde_json::Value = serde_json::from_str(&generated["openapi.json"]).unwrap();
    let paths = document["paths"].as_object().unwrap();
    assert!(!paths.contains_key("/accounts"), "{:?}", paths.keys());
    assert!(paths.contains_key("/accounts/{id}"));

    assert!(!generated["client.rs"].contains("fn list_accounts"));
    assert!(generated["client.rs"].contains("fn get_account"));
    assert!(!generated["client.ts"].contains("listAccounts"));
    assert!(generated["client.ts"].contains("getAccount"));
    assert!(!generated["client.py"].contains("def list_accounts"));
    assert!(generated["client.py"].contains("def get_account"));
    assert!(!generated["client.go"].contains("func (c *Client) ListAccounts"));
    assert!(generated["client.go"].contains("func (c *Client) GetAccount"));

    // GraphQL and the MCP manifest too.
    assert!(!generated["schema.graphql"].contains("  accounts("));
    assert!(generated["schema.graphql"].contains("  account(id: ID!)"));
    let tools: serde_json::Value = serde_json::from_str(&generated["mcp-tools.json"]).unwrap();
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // MCP names a tool subject-first.
    assert!(!names.contains(&"account_list"), "{names:?}");
    assert!(names.contains(&"account_get"), "{names:?}");
}

#[test]
fn a_faceless_resource_still_carries_its_actions_and_sub_resources() {
    let generated = artifacts(ResourceFaces::NONE);
    assert_eq!(validate(&contract(ResourceFaces::NONE), &schema()), vec![]);

    let document: serde_json::Value = serde_json::from_str(&generated["openapi.json"]).unwrap();
    let paths = document["paths"].as_object().unwrap();
    assert!(!paths.contains_key("/accounts"));
    assert!(!paths.contains_key("/accounts/{id}"));
    // The two things that do not depend on a collection face.
    assert!(
        paths.contains_key("/accounts/{id}/suspend"),
        "{:?}",
        paths.keys()
    );
    assert!(
        paths.contains_key("/accounts/{id}/keys"),
        "{:?}",
        paths.keys()
    );

    assert!(generated["client.rs"].contains("fn suspend_account"));
    assert!(generated["client.rs"].contains("fn list_keys_accounts"));
}

#[test]
fn a_listing_claim_on_a_resource_with_no_listing_is_refused() {
    // These describe an endpoint that does not exist. Left to stand,
    // they would reach the artifacts and read as promises.
    let mut resource = accounts(ResourceFaces::GET_ONLY);
    resource.filterable = vec!["realm".into()];
    let mut broken = contract(ResourceFaces::GET_ONLY);
    broken.resources = vec![resource];
    let violations = validate(&broken, &schema());
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("filterable") && v.to_string().contains("no listing")),
        "{violations:?}",
    );

    let mut watching = contract(ResourceFaces::GET_ONLY);
    watching.resources[0].watchable = true;
    let violations = validate(&watching, &schema());
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("watchable")),
        "{violations:?}",
    );
}

#[test]
fn a_resource_that_exposes_nothing_at_all_is_refused() {
    let mut empty = contract(ResourceFaces::NONE);
    empty.resources[0].actions = vec![];
    empty.resources[0].sub_resources = vec![];
    let violations = validate(&empty, &schema());
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("exposes no listing")),
        "{violations:?}",
    );
}

#[test]
fn a_missing_face_frees_the_method_name_for_an_action() {
    // `get` on `accounts` derives `get_account`, which collides with
    // the getter -- unless the resource has no getter, in which case
    // the name is genuinely free.
    let mut taken = contract(ResourceFaces::ALL);
    taken.resources[0].actions = vec![Action {
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
    assert!(
        !validate(&taken, &schema()).is_empty(),
        "the getter still holds get_account",
    );

    let mut freed = taken.clone();
    freed.resources[0].faces = ResourceFaces::LIST_ONLY;
    assert_eq!(
        validate(&freed, &schema()),
        vec![],
        "with no getter, get_account belongs to the action",
    );
}

#[test]
fn withdrawing_a_face_is_breaking_and_restoring_one_is_not() {
    let breaking = diff(
        &contract(ResourceFaces::ALL),
        &contract(ResourceFaces::GET_ONLY),
    );
    assert!(
        breaking
            .iter()
            .any(|c| matches!(c, Change::Breaking(m) if m.contains("listing removed"))),
        "{breaking:?}",
    );

    let restored = diff(
        &contract(ResourceFaces::GET_ONLY),
        &contract(ResourceFaces::ALL),
    );
    assert!(
        restored
            .iter()
            .any(|c| matches!(c, Change::Compatible(m) if m.contains("listing added"))),
        "{restored:?}",
    );
    assert!(
        !restored.iter().any(Change::is_breaking),
        "adding a face takes nothing away: {restored:?}",
    );
}

#[test]
fn the_default_faces_stay_out_of_a_serialized_contract() {
    let rendered = serde_json::to_string(&contract(ResourceFaces::ALL)).unwrap();
    assert!(!rendered.contains("faces"), "{rendered}");

    let narrowed = serde_json::to_string(&contract(ResourceFaces::GET_ONLY)).unwrap();
    assert!(narrowed.contains("\"faces\""), "{narrowed}");
    let parsed: Contract = serde_json::from_str(&narrowed).unwrap();
    assert_eq!(parsed.resources[0].faces, ResourceFaces::GET_ONLY);

    // A contract written before the field reads as both faces.
    let parsed: Contract = serde_json::from_str(&rendered).unwrap();
    assert_eq!(parsed.resources[0].faces, ResourceFaces::ALL);
}

#[test]
fn a_faceless_resource_needs_no_fields_to_expose() {
    // An RPC-only domain has rows nobody receives, so requiring an
    // exposure would be requiring a projection that is never built.
    // Found writing polyconsole-social's contract, where `friends` is
    // five verbs over a table of unordered pairs and `me` is three
    // writes that name no id.
    let mut verbs_only = contract(ResourceFaces::NONE);
    verbs_only.resources[0].fields = vec![];
    assert_eq!(validate(&verbs_only, &schema()), vec![]);

    // A face still demands one: something has to be in the row.
    for faces in [
        ResourceFaces::ALL,
        ResourceFaces::GET_ONLY,
        ResourceFaces::LIST_ONLY,
    ] {
        let mut bare = contract(faces);
        bare.resources[0].fields = vec![];
        bare.resources[0].filterable = vec![];
        let violations = validate(&bare, &schema());
        assert!(
            violations
                .iter()
                .any(|v| v.to_string().contains("exposes no fields")),
            "{faces:?} projects rows and must expose some: {violations:?}",
        );
    }
}

#[test]
fn an_action_answering_with_the_resource_still_needs_fields() {
    // No collection face, but the type is rendered anyway because an
    // action returns it -- so an empty projection would emit a struct
    // with nothing but an id.
    let mut answering = contract(ResourceFaces::NONE);
    answering.resources[0].fields = vec![];
    answering.resources[0].actions = vec![Action {
        name: "adopt".into(),
        method: "POST".into(),
        path: "/{id}/adopt".into(),
        input: vec![],
        output: ActionOutput::Resource,
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    }];
    let violations = validate(&answering, &schema());
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("exposes no fields")),
        "{violations:?}",
    );
}

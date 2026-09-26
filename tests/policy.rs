//! The engine-policy face, proven against the deployment it replaces.
//!
//! A file service derived its SurrealDB `PERMISSIONS` clauses from the
//! contract by hand in its server, and this module moved that
//! derivation into kayak. The tests here hold the derived clause
//! STRINGS byte-identical to what the service's own code produced for
//! the same inputs, so its switch to `kayak::derive_policy` reviews as
//! pure deletion: same tables, same columns, same clauses, in the same
//! order.

use kayak::generate::{generate_all, TARGETS};
use kayak::{
    derive_policy, ClaimVocabulary, EnginePolicy, FieldExposure, GenerateError, Resource,
    SubResource,
};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

/// The tables the contract below validates against: every column it
/// names, and an index leading with the columns each listing binds.
/// The derivation reads none of this, and the target validates against
/// it first.
fn schema() -> Vec<TableDefinition> {
    let table = |name: &str, columns: &[&str], indexed: &[&str]| {
        table_schema(name)
            .with_mode(TableMode::Schemafull)
            .with_fields(
                columns
                    .iter()
                    .map(|column| string_field(*column).build_unchecked().unwrap()),
            )
            .with_indexes([index(format!("idx_{name}"), indexed.iter().copied())])
    };
    vec![
        table(
            "file",
            &["tenant_id", "created_at", "digest"],
            &["tenant_id"],
        ),
        table(
            "file_version",
            &["file", "tenant_id", "number", "created_by", "created_at"],
            &["file", "tenant_id"],
        ),
        table(
            "webhook_endpoint",
            &["tenant_id", "created_at"],
            &["tenant_id"],
        ),
        table(
            "webhook_delivery",
            &["endpoint", "tenant_id", "state", "created_at"],
            &["endpoint", "tenant_id", "state"],
        ),
        table("file_event", &["tenant_id", "created_at"], &["tenant_id"]),
        table("workflow_run", &["tenant_id", "created_at"], &["tenant_id"]),
    ]
}

/// A resource carrying only what the policy face reads: table, field
/// exposures, read scopes, sub-resources. Everything else defaulted,
/// because the derivation must not depend on it.
fn resource(name: &str, table: &str) -> Resource {
    Resource {
        name: name.into(),
        table: table.into(),
        identity: Default::default(),
        fields: vec![FieldExposure::column("created_at")],
        pinned: vec!["tenant_id".into()],
        pinned_either: vec![],
        filterable: vec![],
        filter_options: Default::default(),
        faces: Default::default(),
        sortable: vec![],
        max_page_size: 100,
        actions: vec![],
        content: None,
        sub_resources: vec![],
        rate_class: None,
        reads_require: vec![],
        watchable: false,
        graphql: None,
    }
}

/// The shape of the reference deployment's real contract: four
/// resources, all
/// requiring the `read` scope, two carrying a sub-resource, and one
/// guarded field -- `file_version.created_by` under `owner_or_admin`.
fn reference_shaped() -> kayak::Contract {
    let mut files = resource("files", "file");
    files.reads_require = vec!["read".into()];
    files.sub_resources = vec![SubResource {
        name: "versions".into(),
        table: "file_version".into(),
        parent_key: "file".into(),
        identity: Default::default(),
        fields: vec![
            FieldExposure::column("number"),
            FieldExposure::column("created_by").with_guard("owner_or_admin"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec![],
        sortable: vec![],
        max_page_size: 100,
        description: None,
        graphql: None,
    }];
    let mut webhooks = resource("webhooks", "webhook_endpoint");
    webhooks.reads_require = vec!["read".into()];
    webhooks.sub_resources = vec![SubResource {
        name: "deliveries".into(),
        table: "webhook_delivery".into(),
        parent_key: "endpoint".into(),
        identity: Default::default(),
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
    let mut events = resource("events", "file_event");
    events.reads_require = vec!["read".into()];
    let mut runs = resource("runs", "workflow_run");
    runs.reads_require = vec!["read".into()];

    kayak::Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        rate_classes: vec![],
        limits: None,
        auth: Default::default(),
        resources: vec![files, webhooks, events, runs],
        queries: vec![],
    }
}

/// Byte-for-byte what the reference deployment's hand derivation
/// produced. The forms it matches:
///
/// - Guard clauses: `"owner_or_admin"` renders as
///   `"$token.adm = true OR created_by = $token.pr"` and
///   `"admin_only"` as `"$token.adm = true"` -- the default
///   [`ClaimVocabulary`] carries both verbatim.
/// - Field guards walk resources in contract order, each resource's
///   own fields before its sub-resources' fields, pushing
///   `(table, column, clause)`.
/// - Each scope in `reads_require` renders as
///   `$token.sc CONTAINS '{scope}'`, the scopes join with `" AND "`,
///   and the conjunct lands on the resource's table and then on every
///   sub-resource table, in contract order.
///
/// Not reproduced, on purpose: the `file_version` delete conjunct
/// (retention the contract cannot declare) and the store's mechanical
/// tenancy floor (derived from the schema so an omitted table cannot
/// dodge it). Both stay with the service.
#[test]
fn the_derivation_is_a_no_op_for_the_reference_deployment() {
    let policy = derive_policy(&reference_shaped(), &ClaimVocabulary::default()).unwrap();
    assert_eq!(
        policy,
        EnginePolicy {
            field_guards: vec![(
                "file_version".to_owned(),
                "created_by".to_owned(),
                "$token.adm = true OR created_by = $token.pr".to_owned(),
            )],
            select_conjuncts: vec![
                ("file".to_owned(), "$token.sc CONTAINS 'read'".to_owned()),
                (
                    "file_version".to_owned(),
                    "$token.sc CONTAINS 'read'".to_owned(),
                ),
                (
                    "webhook_endpoint".to_owned(),
                    "$token.sc CONTAINS 'read'".to_owned(),
                ),
                (
                    "webhook_delivery".to_owned(),
                    "$token.sc CONTAINS 'read'".to_owned(),
                ),
                (
                    "file_event".to_owned(),
                    "$token.sc CONTAINS 'read'".to_owned(),
                ),
                (
                    "workflow_run".to_owned(),
                    "$token.sc CONTAINS 'read'".to_owned(),
                ),
            ],
        },
    );
}

/// The other guard the default vocabulary names, and the multi-scope
/// join, held to the same clause forms.
#[test]
fn the_default_vocabulary_matches_the_other_forms() {
    let mut audited = resource("audits", "audit_log");
    audited.reads_require = vec!["read".into(), "audit".into()];
    audited.fields = vec![FieldExposure::column("actor").with_guard("admin_only")];
    let contract = kayak::Contract {
        resources: vec![audited],
        ..reference_shaped()
    };
    let policy = derive_policy(&contract, &ClaimVocabulary::default()).unwrap();
    assert_eq!(
        policy.field_guards,
        vec![(
            "audit_log".to_owned(),
            "actor".to_owned(),
            "$token.adm = true".to_owned(),
        )],
    );
    assert_eq!(
        policy.select_conjuncts,
        vec![(
            "audit_log".to_owned(),
            "$token.sc CONTAINS 'read' AND $token.sc CONTAINS 'audit'".to_owned(),
        )],
    );
}

/// A guard without a clause refuses the whole derivation, the way
/// the reference deployment's boot refuses: shipping it would
/// silently drop the engine layer for that column while the
/// application layer kept enforcing.
#[test]
fn an_unknown_guard_refuses_rather_than_ships_half_a_policy() {
    let mut contract = reference_shaped();
    contract.resources[0].fields = vec![FieldExposure::column("digest").with_guard("finance_only")];
    let error = derive_policy(&contract, &ClaimVocabulary::default()).unwrap_err();
    assert!(
        error.to_string().contains("finance_only"),
        "the refusal names the guard: {error}",
    );
}

/// The opt-in `engine-policy` target renders the derivation as
/// `policy.json`, review-visible beside the other artifacts. The
/// expectation is inline rather than a golden because the whole
/// artifact fits in one look, and its bytes ARE the point: what
/// changes here in a commit is what changes in the engine.
#[test]
fn the_engine_policy_target_renders_the_artifact() {
    let artifacts = generate_all(&reference_shaped(), &schema(), &["engine-policy"]).unwrap();
    assert_eq!(
        artifacts["policy.json"],
        r#"{
  "field_guards": [
    [
      "file_version",
      "created_by",
      "$token.adm = true OR created_by = $token.pr"
    ]
  ],
  "select_conjuncts": [
    [
      "file",
      "$token.sc CONTAINS 'read'"
    ],
    [
      "file_version",
      "$token.sc CONTAINS 'read'"
    ],
    [
      "webhook_endpoint",
      "$token.sc CONTAINS 'read'"
    ],
    [
      "webhook_delivery",
      "$token.sc CONTAINS 'read'"
    ],
    [
      "file_event",
      "$token.sc CONTAINS 'read'"
    ],
    [
      "workflow_run",
      "$token.sc CONTAINS 'read'"
    ]
  ]
}
"#,
    );
}

/// Through the CLI orchestrator, a guard outside the default
/// vocabulary refuses the run and names the guard -- the reason the
/// target is opt-in rather than a default: the vocabulary belongs to
/// the deployment, and the CLI only holds the default conventions.
#[test]
fn the_target_refuses_a_guard_outside_the_default_vocabulary() {
    let mut contract = reference_shaped();
    contract.resources[0].fields = vec![FieldExposure::column("digest").with_guard("finance_only")];
    let error = generate_all(&contract, &schema(), &["engine-policy"]).unwrap_err();
    assert!(
        error.to_string().contains("finance_only"),
        "the refusal names the guard: {error}",
    );
}

/// Every target validates first, the two that read only the contract
/// included. The MCP manifest and the engine policy used to render
/// whatever they were handed, so a contract over a table the schema
/// lacks shipped a manifest and a policy for it.
#[test]
fn every_target_refuses_an_invalid_contract() {
    let mut contract = reference_shaped();
    contract.resources[0].table = "nonesuch".into();
    let opt_in = ["engine-policy", "client-rs-blocking"];
    for target in TARGETS.iter().chain(opt_in.iter()) {
        let refused = generate_all(&contract, &schema(), &[target]);
        assert!(
            matches!(refused, Err(GenerateError::Invalid(_))),
            "{target} rendered an invalid contract: {refused:?}",
        );
    }
    assert!(generate_all(&reference_shaped(), &schema(), &["mcp", "engine-policy"]).is_ok());
}

//! Where a contract's resource routes live.
//!
//! `/v1` was hardcoded in four client generators and the OpenAPI
//! emitter, which meant janus could only generate clients for services
//! that had already chosen its version prefix. A service serving
//! `/accounts` could not adopt a generated client, and moving its
//! routes to suit the generator breaks whatever is shipped against
//! them -- the wrong direction for a tool whose job is catching breaks.

use janus::diff::{diff, Change};
use janus::generate::generate_all;
use janus::validate::validate;
use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, TypeRef};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

fn schema() -> Vec<TableDefinition> {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    vec![table_schema("account")
        .with_mode(TableMode::Schemafull)
        .with_fields([built(string_field("handle")), built(string_field("realm"))])
        .with_indexes([index("account_realm_idx", ["realm"])])]
}

fn contract(prefix: &str) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: prefix.into(),
        limits: None,
        rate_classes: vec![],
        auth: janus::AuthScheme::None,
        resources: vec![Resource {
            name: "accounts".into(),
            table: "account".into(),
            identity: Default::default(),
            fields: vec![FieldExposure::column("handle")],
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
            faces: Default::default(),
        }],
        // A query declares an absolute path, so it should be untouched
        // by the prefix in every case below.
        queries: vec![Query {
            name: "whoami".into(),
            path: "/me".into(),
            input: vec![ActionField {
                name: "verbose".into(),
                kind: TypeRef::Bool,
                required: false,
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
        }],
    }
}

fn artifacts(prefix: &str) -> std::collections::BTreeMap<String, String> {
    generate_all(
        &contract(prefix),
        &schema(),
        &[
            "openapi",
            "client-rs",
            "client-ts",
            "client-py",
            "client-go",
        ],
    )
    .expect("the contract generates")
}

#[test]
fn a_contract_that_says_nothing_still_gets_v1() {
    // The whole existing corpus depends on this, which is why the
    // goldens do not move when the prefix becomes declarable.
    assert_eq!(Contract::default_api_prefix(), "/v1");
    let generated = artifacts("/v1");
    assert!(generated["openapi.json"].contains("\"/v1/accounts\""));
    assert!(generated["client.rs"].contains("/v1/accounts"));
}

#[test]
fn an_empty_prefix_puts_the_resources_at_the_root() {
    let generated = artifacts("");
    assert_eq!(validate(&contract(""), &schema()), vec![]);

    // Every face, because a prefix that reached three generators and
    // missed the fourth is worse than one that reached none.
    assert!(
        generated["openapi.json"].contains("\"/accounts\""),
        "{}",
        &generated["openapi.json"][..400],
    );
    assert!(generated["openapi.json"].contains("\"/accounts/{id}\""));
    assert!(generated["client.rs"].contains("format!(\"{}/accounts\""));
    assert!(generated["client.ts"].contains("`/accounts${suffix}`"));
    assert!(generated["client.py"].contains("f'/accounts{suffix}'"));
    assert!(generated["client.go"].contains("path := \"/accounts\""));

    // And nothing anywhere still says /v1.
    for (filename, body) in &generated {
        assert!(!body.contains("/v1/"), "{filename} still carries /v1");
    }
}

#[test]
fn a_custom_prefix_reaches_every_face_including_actions() {
    let generated = artifacts("/api/v2");
    assert_eq!(validate(&contract("/api/v2"), &schema()), vec![]);

    assert!(generated["openapi.json"].contains("\"/api/v2/accounts\""));
    // The action path hangs off the same prefix, which is the site the
    // list and get endpoints do not cover.
    assert!(generated["openapi.json"].contains("\"/api/v2/accounts/{id}/suspend\""));
    assert!(generated["client.rs"].contains("/api/v2/accounts/{id}/suspend"));
    assert!(generated["client.go"].contains("\"/api/v2/accounts\""));
    assert!(generated["client.py"].contains("/api/v2/accounts"));
    assert!(generated["client.ts"].contains("/api/v2/accounts"));
}

#[test]
fn a_query_keeps_its_absolute_path_whatever_the_prefix() {
    // This is what made queries able to describe a real service's
    // routes before resources could, and it must stay true.
    for prefix in ["", "/v1", "/api/v2"] {
        let generated = artifacts(prefix);
        assert!(
            generated["openapi.json"].contains("\"/me\""),
            "prefix {prefix:?} moved a query path",
        );
        assert!(generated["client.rs"].contains("format!(\"{}/me\""));
    }
}

#[test]
fn a_trailing_slash_is_not_a_second_slash_in_every_path() {
    let generated = artifacts("/v1/");
    assert!(generated["openapi.json"].contains("\"/v1/accounts\""));
    assert!(!generated["openapi.json"].contains("//accounts"));
    // A lone slash means the root, not an empty first segment.
    let root = artifacts("/");
    assert!(root["openapi.json"].contains("\"/accounts\""));
    assert!(!root["openapi.json"].contains("\"//accounts\""));
}

#[test]
fn a_malformed_prefix_is_refused() {
    for bad in ["v1", "/v1//x", "/v1 /x"] {
        let violations = validate(&contract(bad), &schema());
        assert!(
            violations
                .iter()
                .any(|v| v.to_string().contains("api_prefix")),
            "{bad:?} was accepted: {violations:?}",
        );
    }
}

#[test]
fn moving_the_prefix_is_breaking() {
    // Every resource route moves at once, so this is the broadest
    // break a contract can express.
    let changes = diff(&contract("/v1"), &contract(""));
    let breaking: Vec<_> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Breaking(message) => Some(message),
            _ => None,
        })
        .collect();
    assert!(breaking.iter().any(|m| m.contains("prefix")), "{changes:?}",);
    // And in the other direction too: adding a prefix to a rootless
    // API moves every route just as far.
    let back = diff(&contract(""), &contract("/v1"));
    assert!(back
        .iter()
        .any(|c| matches!(c, Change::Breaking(m) if m.contains("prefix"))));
}

#[test]
fn the_default_prefix_stays_out_of_a_serialized_contract() {
    // So contracts written before the field existed round-trip byte
    // for byte, and only a service that chose something says so.
    let rendered = serde_json::to_string(&contract("/v1")).unwrap();
    assert!(!rendered.contains("api_prefix"), "{rendered}");

    let custom = serde_json::to_string(&contract("")).unwrap();
    assert!(custom.contains("\"api_prefix\":\"\""), "{custom}");
    let parsed: Contract = serde_json::from_str(&custom).unwrap();
    assert_eq!(parsed.api_prefix, "");

    // And a document with no field at all still reads as /v1.
    let without = serde_json::to_string(&contract("/v1")).unwrap();
    let parsed: Contract = serde_json::from_str(&without).unwrap();
    assert_eq!(parsed.api_prefix, "/v1");
}

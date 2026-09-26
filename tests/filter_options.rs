//! A filter's closed set, held on the wire.
//!
//! `filter_options` says which values a filterable column accepts. The
//! console offers them as a menu and verification probes with them.
//! These tests hold the documents and the runtime to the same set.

use kayak::{Contract, FieldExposure, Resource};

fn contract() -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: Default::default(),
        resources: vec![Resource {
            name: "files".into(),
            table: "file".into(),
            identity: Default::default(),
            fields: vec![FieldExposure::column("state")],
            pinned: vec![],
            pinned_either: vec![],
            filterable: vec!["state".into()],
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
            filter_options: [(
                "state".to_owned(),
                vec!["ready".to_owned(), "uploading".to_owned()],
            )]
            .into(),
        }],
        queries: vec![],
    }
}

#[cfg(feature = "runtime")]
fn filtered(state: &str) -> kayak::runtime::ListArgs {
    kayak::runtime::ListArgs {
        limit: 10,
        cursor: None,
        filters: [("state".to_owned(), serde_json::json!(state))].into(),
        sort: None,
    }
}

#[cfg(feature = "runtime")]
#[tokio::test]
async fn a_filter_value_outside_the_declared_set_is_refused() {
    use std::sync::Arc;

    use kayak::runtime::{Dispatcher, KayakContext, KayakError, ListArgs, ListOutput, Resolvers};

    let resolvers = Resolvers::new()
        .list("files", |_ctx, _args: ListArgs| async move {
            Ok(ListOutput::default())
        })
        .get("files", |_ctx, _args| async move { Ok(None) });
    let dispatcher = Dispatcher::new(Arc::new(contract()), resolvers, vec![]).unwrap();

    let listed = dispatcher
        .list("files", KayakContext::new(), filtered("ready"))
        .await;
    assert!(listed.is_ok(), "a declared value passes: {listed:?}");

    let refused = dispatcher
        .list("files", KayakContext::new(), filtered("deleted"))
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, KayakError::BadRequest(message) if message.contains("ready, uploading")),
        "the refusal names the set: {refused}",
    );
}

/// The documents publish the set the runtime holds callers to, the way
/// they publish an input's `options`.
#[test]
fn openapi_and_mcp_publish_the_declared_set() {
    use surql::schema::{index, string_field, table_schema, TableMode};

    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    let schema = vec![table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields([built(string_field("state"))])
        .with_indexes([index("idx_state", ["state"])])];
    let contract = contract();

    let openapi = kayak::generate_openapi(&contract, &schema).unwrap();
    let parameters = openapi["paths"]["/v1/files"]["get"]["parameters"]
        .as_array()
        .unwrap();
    let state = parameters.iter().find(|p| p["name"] == "state").unwrap();
    assert_eq!(
        state["schema"]["enum"],
        serde_json::json!(["ready", "uploading"])
    );

    let tools = kayak::generate_mcp_tools(&contract).unwrap();
    let list = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "files_list")
        .unwrap();
    assert_eq!(
        list["inputSchema"]["properties"]["state"]["enum"],
        serde_json::json!(["ready", "uploading"]),
    );
}

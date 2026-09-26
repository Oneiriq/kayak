//! A filter's closed set, held on the wire.
//!
//! `filter_options` says which values a filterable column accepts. The
//! console offers them as a menu and verification probes with them.
//! These tests hold the runtime to the same set.
#![cfg(feature = "runtime")]

use std::collections::BTreeMap;
use std::sync::Arc;

use kayak::runtime::{Dispatcher, KayakContext, KayakError, ListArgs, ListOutput, Resolvers};
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

fn filtered(state: &str) -> ListArgs {
    ListArgs {
        limit: 10,
        cursor: None,
        filters: BTreeMap::from([("state".to_owned(), serde_json::json!(state))]),
        sort: None,
    }
}

#[tokio::test]
async fn a_filter_value_outside_the_declared_set_is_refused() {
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

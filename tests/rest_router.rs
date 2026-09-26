//! The REST router serves what the contract declares, where the
//! contract declares it.
//!
//! The generated clients and the OpenAPI document take every path from
//! the contract, so a router that assumes anything the contract did not
//! say answers those clients with a 404 or a refusal.
#![cfg(feature = "runtime")]

use std::sync::{Arc, Mutex};

use kayak::runtime::{Dispatcher, KayakContext, ListArgs, ListOutput, Resolvers, RestRouter};
use kayak::{Contract, FieldExposure, Resource};

fn resource() -> Resource {
    Resource {
        name: "files".into(),
        table: "file".into(),
        identity: Default::default(),
        fields: vec![FieldExposure::column("path")],
        pinned: vec![],
        pinned_either: vec![],
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
    }
}

fn contract(api_prefix: &str) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: api_prefix.into(),
        limits: None,
        rate_classes: vec![],
        auth: Default::default(),
        resources: vec![resource()],
        queries: vec![],
    }
}

/// A dispatcher over `contract`, and the list arguments its resolver
/// saw.
fn dispatcher(contract: Contract) -> (Arc<Dispatcher>, Arc<Mutex<Option<ListArgs>>>) {
    let seen = Arc::new(Mutex::new(None));
    let capture = seen.clone();
    let resolvers = Resolvers::new()
        .list("files", move |_ctx, args: ListArgs| {
            let capture = capture.clone();
            async move {
                *capture.lock().unwrap() = Some(args);
                Ok(ListOutput::default())
            }
        })
        .get("files", |_ctx, args| async move {
            Ok(Some(serde_json::json!({ "id": args.id, "path": "a.txt" })))
        });
    let dispatcher = Dispatcher::new(Arc::new(contract), resolvers, vec![]).unwrap();
    (Arc::new(dispatcher), seen)
}

fn router(contract: Contract) -> (RestRouter, Arc<Mutex<Option<ListArgs>>>) {
    let (dispatcher, seen) = dispatcher(contract);
    (RestRouter::new(dispatcher), seen)
}

async fn status(router: &RestRouter, path: &str) -> u16 {
    router
        .handle("GET", path, "", None, KayakContext::new())
        .await
        .status
}

#[tokio::test]
async fn resources_serve_under_the_contracts_own_prefix() {
    let (under_api, _) = router(contract("/api"));
    assert_eq!(status(&under_api, "/api/files").await, 200);
    assert_eq!(status(&under_api, "/api/files/01A").await, 200);
    assert_eq!(
        status(&under_api, "/v1/files").await,
        404,
        "the default prefix is not served when the contract names another",
    );

    let (at_root, _) = router(contract(""));
    assert_eq!(status(&at_root, "/files").await, 200);
    assert_eq!(status(&at_root, "/files/01A").await, 200);

    let (by_default, _) = router(contract("/v1"));
    assert_eq!(status(&by_default, "/v1/files").await, 200);
}

/// The console's reference shows the path the router serves, so a
/// request copied from it reaches the service.
#[cfg(feature = "console")]
#[tokio::test]
async fn the_console_reference_names_the_served_prefix() {
    use kayak::runtime::{ConsoleConfig, ConsoleRouter};

    let (dispatcher, _) = dispatcher(contract("/api"));
    let console = ConsoleRouter::new(
        dispatcher,
        ConsoleConfig {
            base: "/console".to_owned(),
            title: "test".to_owned(),
        },
    );
    let page = console.page("/reference", "", KayakContext::new()).await;
    assert!(page.html.contains("GET /api/files"), "{}", page.html);
    assert!(!page.html.contains("/v1/files"), "{}", page.html);
}

/// A query whose path names one of its inputs receives it from the
/// path. The OpenAPI document declares it `in: path` and every client
/// sends it only there.
#[tokio::test]
async fn a_query_takes_the_input_its_path_carries() {
    let mut contract = contract("/v1");
    contract.queries = vec![kayak::Query {
        name: "file_text".into(),
        path: "/v1/files/{id}/text".into(),
        input: vec![
            kayak::ActionField {
                name: "id".into(),
                kind: kayak::TypeRef::String,
                required: true,
                multiple: false,
                description: None,
                options: vec![],
            },
            kayak::ActionField {
                name: "page".into(),
                kind: kayak::TypeRef::Int,
                required: false,
                multiple: false,
                description: None,
                options: vec![],
            },
        ],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![],
        backing: vec![],
    }];
    let resolvers = Resolvers::new()
        .list("files", |_ctx, _args: ListArgs| async move {
            Ok(ListOutput::default())
        })
        .get("files", |_ctx, _args| async move { Ok(None) })
        .query("file_text", |_ctx, args| async move {
            Ok(serde_json::Value::Object(args.input))
        });
    let dispatcher = Dispatcher::new(Arc::new(contract), resolvers, vec![]).unwrap();
    let router = RestRouter::new(Arc::new(dispatcher));

    let answered = router
        .handle(
            "GET",
            "/v1/files/01A/text",
            "page=2",
            None,
            KayakContext::new(),
        )
        .await;
    assert_eq!(answered.status, 200, "{:?}", answered.body);
    assert_eq!(
        answered.body,
        serde_json::json!({ "id": "01A", "page": 2 }),
        "the path's id and the query string's page both arrive",
    );
}

/// A listing without a limit gets the page the OpenAPI document
/// promises: `max_page_size`, the same default GraphQL applies.
#[tokio::test]
async fn a_listing_without_a_limit_gets_the_declared_page() {
    let mut narrow = contract("/v1");
    narrow.resources[0].max_page_size = 20;
    for (contract, expected) in [(contract("/v1"), 100), (narrow, 20)] {
        let (router, seen) = router(contract);
        let answered = router
            .handle("GET", "/v1/files", "", None, KayakContext::new())
            .await;
        assert_eq!(answered.status, 200, "{:?}", answered.body);
        let limit = seen.lock().unwrap().as_ref().unwrap().limit;
        assert_eq!(limit, expected);
    }
}

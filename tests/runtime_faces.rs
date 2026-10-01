//! The runtime serves the faces a resource declares, and only those.
//!
//! The SDL, the OpenAPI document, the MCP manifest, and the clients all
//! leave out a face the resource withholds. A runtime that served it
//! anyway would enumerate the collection the contract says must never
//! be enumerated.
#![cfg(feature = "graphql")]

use std::sync::Arc;

use kayak::runtime::graphql::build_schema;
use kayak::runtime::{
    Dispatcher, GetArgs, KayakContext, ListArgs, ListOutput, Resolvers, RestRouter,
};
use kayak::{Contract, FieldExposure, Resource, ResourceFaces};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

fn table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("account")
        .with_mode(TableMode::Schemafull)
        .with_fields([built(string_field("handle"))])
        .with_indexes([index("idx_handle", ["handle"])])
}

fn contract(faces: ResourceFaces) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: Default::default(),
        resources: vec![Resource {
            name: "accounts".into(),
            table: "account".into(),
            identity: Default::default(),
            fields: vec![FieldExposure::column("handle")],
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
            faces,
            actions: vec![],
            content: None,
            filter_options: Default::default(),
        }],
        queries: vec![],
    }
}

fn getter() -> Resolvers {
    Resolvers::new().get("accounts", |_ctx, args: GetArgs| async move {
        Ok(Some(serde_json::json!({ "id": args.id, "handle": "ada" })))
    })
}

fn lister() -> Resolvers {
    Resolvers::new().list("accounts", |_ctx, _args: ListArgs| async move {
        Ok(ListOutput {
            items: vec![serde_json::json!({ "id": "01A", "handle": "ada" })],
            next_cursor: None,
        })
    })
}

fn listing() -> ListArgs {
    ListArgs {
        limit: 10,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_resource_that_withholds_its_listing_is_never_listed() {
    let faces = ResourceFaces {
        list: false,
        get: true,
    };
    // No list resolver: the contract declares no listing to serve.
    let dispatcher = Dispatcher::new(Arc::new(contract(faces)), getter(), vec![])
        .expect("a get-only resource needs no list resolver");
    let dispatcher = Arc::new(dispatcher);

    let refused = dispatcher
        .list("accounts", KayakContext::new(), listing())
        .await;
    assert!(refused.is_err(), "the dispatcher refuses the listing");

    let rest = RestRouter::new(dispatcher.clone());
    let listed = rest
        .handle("GET", "/v1/accounts", "", None, KayakContext::new())
        .await;
    assert_eq!(listed.status, 404, "{:?}", listed.body);
    let fetched = rest
        .handle("GET", "/v1/accounts/01A", "", None, KayakContext::new())
        .await;
    assert_eq!(fetched.status, 200, "{:?}", fetched.body);

    let schema = build_schema(&[table()], dispatcher).unwrap();
    let sdl = schema.sdl();
    assert!(!sdl.contains("accounts("), "no list field: {sdl}");
    assert!(sdl.contains("account(id: ID!)"), "the get field: {sdl}");
    let generated = kayak::generate_sdl(&contract(faces), &[table()]).unwrap();
    assert!(!generated.contains("accounts("), "{generated}");
}

#[tokio::test]
async fn a_resource_that_withholds_its_getter_is_never_fetched() {
    let faces = ResourceFaces {
        list: true,
        get: false,
    };
    let dispatcher = Dispatcher::new(Arc::new(contract(faces)), lister(), vec![])
        .expect("a list-only resource needs no get resolver");
    let dispatcher = Arc::new(dispatcher);

    let refused = dispatcher
        .get(
            "accounts",
            KayakContext::new(),
            GetArgs { id: "01A".into() },
        )
        .await;
    assert!(refused.is_err(), "the dispatcher refuses the get");

    let rest = RestRouter::new(dispatcher.clone());
    let fetched = rest
        .handle("GET", "/v1/accounts/01A", "", None, KayakContext::new())
        .await;
    assert_eq!(fetched.status, 404, "{:?}", fetched.body);

    let schema = build_schema(&[table()], dispatcher).unwrap();
    let sdl = schema.sdl();
    assert!(sdl.contains("accounts("), "the list field: {sdl}");
    assert!(!sdl.contains("account(id"), "no get field: {sdl}");
}

/// The console neither asks for a listing the contract withholds nor
/// links a row to a page no getter can fill.
#[cfg(feature = "console")]
#[tokio::test]
async fn the_console_offers_only_the_declared_faces() {
    use kayak::runtime::{ConsoleConfig, ConsoleRouter};

    let console = |faces: ResourceFaces, resolvers: Resolvers| {
        let dispatcher = Dispatcher::new(Arc::new(contract(faces)), resolvers, vec![]).unwrap();
        ConsoleRouter::new(
            Arc::new(dispatcher),
            ConsoleConfig {
                base: "/console".to_owned(),
                title: "test".to_owned(),
            },
        )
    };

    let get_only = console(ResourceFaces::GET_ONLY, getter());
    let overview = get_only.page("/", "", KayakContext::new()).await;
    assert_eq!(overview.status, 200);
    assert!(overview.html.contains("not listable"), "{}", overview.html);
    assert!(
        !overview.html.contains("cannot be listed"),
        "{}",
        overview.html
    );
    let listing = get_only.page("/r/accounts", "", KayakContext::new()).await;
    assert_eq!(listing.status, 200, "{}", listing.html);
    assert!(
        listing.html.contains("withholds this listing"),
        "{}",
        listing.html
    );
    let reference = get_only.page("/reference", "", KayakContext::new()).await;
    assert!(
        !reference.html.contains("accounts_list"),
        "{}",
        reference.html
    );
    assert!(reference.html.contains("account_get"), "{}", reference.html);

    let list_only = console(ResourceFaces::LIST_ONLY, lister());
    let listing = list_only.page("/r/accounts", "", KayakContext::new()).await;
    assert_eq!(listing.status, 200, "{}", listing.html);
    assert!(listing.html.contains("01A"), "{}", listing.html);
    assert!(
        !listing.html.contains("/console/r/accounts/01A"),
        "no link to a page nothing can fill: {}",
        listing.html,
    );
    let reference = list_only.page("/reference", "", KayakContext::new()).await;
    assert!(
        reference.html.contains("accounts_list"),
        "{}",
        reference.html
    );
    assert!(
        !reference.html.contains("account_get"),
        "{}",
        reference.html
    );
}

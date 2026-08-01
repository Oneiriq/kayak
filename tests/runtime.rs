//! The runtime end to end: user-defined resolvers behind the
//! contract-enforcing dispatcher, middleware around every operation,
//! and the dynamic GraphQL schema serving what the SDL generator
//! prints.
#![cfg(feature = "graphql")]

use std::sync::{Arc, Mutex};

use async_graphql::futures_util::{stream, StreamExt as _};
use janus::runtime::graphql::build_schema;
use janus::runtime::{
    Dispatcher, JanusContext, JanusError, ListArgs, ListOutput, Middleware, Next, Operation,
    Outcome, Payload, Resolvers, RowStream, SortDirection, WatchArgs,
};
use janus::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, GraphqlNames, Resource, TypeRef,
};
use surql::schema::{
    datetime_field, index, int_field, string_field, table_schema, TableDefinition, TableMode,
};

fn file_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("path")),
            built(string_field("state")),
            built(int_field("size_bytes").nullable(true)),
            built(datetime_field("created_at")),
        ])
        .with_indexes([index("idx_listing", ["tenant_id", "state", "created_at"])])
}

fn contract() -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        resources: vec![Resource {
            name: "files".into(),
            table: "file".into(),
            fields: vec![
                FieldExposure::column("path"),
                FieldExposure::column("state"),
                FieldExposure::renamed("size_bytes", "size"),
                FieldExposure::column("created_at"),
            ],
            pinned: vec!["tenant_id".into()],
            filterable: vec!["state".into()],
            sortable: vec!["created_at".into()],
            max_page_size: 100,
            graphql: None,
            watchable: false,
            actions: vec![
                Action {
                    name: "issue_url".into(),
                    method: "POST".into(),
                    path: "/{id}/url".into(),
                    input: vec![ActionField {
                        name: "ttl_secs".into(),
                        kind: TypeRef::Int,
                        required: false,
                        description: None,
                    }],
                    output: ActionOutput::Json,
                    description: None,
                    graphql_field: None,
                },
                Action {
                    name: "remove".into(),
                    method: "DELETE".into(),
                    path: "/{id}".into(),
                    input: vec![],
                    output: ActionOutput::None,
                    description: None,
                    graphql_field: None,
                },
            ],
        }],
    }
}

fn rows() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "id": "01A", "path": "a.txt", "state": "ready",
            "size": 3, "created_at": "2026-07-30T00:00:00Z",
        }),
        serde_json::json!({
            "id": "01B", "path": "b.txt", "state": "uploading",
            "size": null, "created_at": "2026-07-30T01:00:00Z",
        }),
    ]
}

#[derive(Debug, Clone, PartialEq)]
struct Tenant(String);

/// Rejects any operation without a tenant in the context.
struct RequireTenant;

impl Middleware for RequireTenant {
    fn handle<'a>(
        &'a self,
        operation: Operation,
        ctx: JanusContext,
        payload: Payload,
        next: Next,
    ) -> janus::runtime::BoxFuture<'a, Result<Outcome, JanusError>> {
        Box::pin(async move {
            if ctx.get::<Tenant>().is_none() {
                return Err(JanusError::Unauthorized("tenant required".into()));
            }
            next.run(operation, ctx, payload).await
        })
    }
}

/// Records enter/exit around every operation, proving the onion.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl Middleware for Recorder {
    fn handle<'a>(
        &'a self,
        operation: Operation,
        ctx: JanusContext,
        payload: Payload,
        next: Next,
    ) -> janus::runtime::BoxFuture<'a, Result<Outcome, JanusError>> {
        Box::pin(async move {
            let label = format!(
                "{}:{}",
                operation.resource,
                operation
                    .action
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", operation.kind)),
            );
            self.0.lock().unwrap().push(format!("enter:{label}"));
            let outcome = next.run(operation, ctx, payload).await;
            self.0.lock().unwrap().push(format!("exit:{label}"));
            outcome
        })
    }
}

struct Fixture {
    schema: async_graphql::dynamic::Schema,
    seen_list_args: Arc<Mutex<Option<ListArgs>>>,
    seen_watch_args: Arc<Mutex<Option<WatchArgs>>>,
    recorded: Arc<Mutex<Vec<String>>>,
}

fn fixture(contract: Contract) -> Fixture {
    let seen_list_args = Arc::new(Mutex::new(None));
    let seen_watch_args = Arc::new(Mutex::new(None));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let watchable = contract.resources[0].watchable;

    let capture = seen_list_args.clone();
    let mut resolvers = Resolvers::new()
        .list("files", move |_ctx, args: ListArgs| {
            let capture = capture.clone();
            async move {
                *capture.lock().unwrap() = Some(args.clone());
                let mut items: Vec<_> = rows()
                    .into_iter()
                    .filter(|row| match args.filters.get("state") {
                        Some(state) => row["state"] == *state,
                        None => true,
                    })
                    .collect();
                items.truncate(args.limit as usize);
                Ok(ListOutput {
                    items,
                    next_cursor: Some("cursor-1".into()),
                })
            }
        })
        .get("files", |_ctx, args| async move {
            Ok(rows().into_iter().find(|row| row["id"] == *args.id))
        })
        .action("files", "issue_url", |_ctx, args| async move {
            let ttl = args
                .input
                .get("ttl_secs")
                .and_then(|v| v.as_i64())
                .unwrap_or(600);
            let id = args.id.expect("instance action");
            Ok(Some(
                serde_json::json!({ "url": format!("https://cdn/{id}"), "ttl": ttl }),
            ))
        })
        .action("files", "remove", |_ctx, args| async move {
            assert!(args.id.is_some());
            Ok(None)
        });

    // Registered only when the contract declares it, which is the rule
    // the dispatcher enforces in both directions.
    if watchable {
        let capture = seen_watch_args.clone();
        resolvers = resolvers.watch("files", move |_ctx, args: WatchArgs| {
            let capture = capture.clone();
            async move {
                *capture.lock().unwrap() = Some(args.clone());
                let matched: Vec<_> = rows()
                    .into_iter()
                    .filter(|row| match args.filters.get("state") {
                        Some(state) => row["state"] == *state,
                        None => true,
                    })
                    .map(Ok)
                    .collect();
                Ok(Box::pin(stream::iter(matched)) as RowStream)
            }
        });
    }

    let dispatcher = Dispatcher::new(
        Arc::new(contract),
        resolvers,
        vec![
            Arc::new(RequireTenant) as Arc<dyn Middleware>,
            Arc::new(Recorder(recorded.clone())),
        ],
    )
    .unwrap();
    let schema = build_schema(&[file_table()], Arc::new(dispatcher)).unwrap();
    Fixture {
        schema,
        seen_list_args,
        seen_watch_args,
        recorded,
    }
}

/// The same contract with the resource opened for watching.
fn watched_contract() -> Contract {
    let mut contract = contract();
    contract.resources[0].watchable = true;
    contract
}

fn tenant_request(query: &str) -> async_graphql::Request {
    async_graphql::Request::new(query).data(JanusContext::new().with(Tenant("acme".into())))
}

#[tokio::test]
async fn list_flows_through_contract_middleware_and_resolver() {
    let fixture = fixture(contract());
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ files(limit: 5000, state: "ready", sort: CREATED_AT_DESC) {
                items { id path size created_at } nextCursor } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["files"]["items"][0]["id"], "01A");
    assert_eq!(data["files"]["items"][0]["size"], 3);
    assert_eq!(data["files"]["nextCursor"], "cursor-1");
    assert_eq!(data["files"]["items"].as_array().unwrap().len(), 1);

    // The dispatcher clamped the limit and translated the wire names.
    let seen = fixture.seen_list_args.lock().unwrap().clone().unwrap();
    assert_eq!(seen.limit, 100, "5000 must clamp to max_page_size");
    assert_eq!(seen.filters["state"], "ready");
    assert_eq!(seen.sort, Some(("created_at".into(), SortDirection::Desc)),);

    // The chain ran around the resolver.
    let recorded = fixture.recorded.lock().unwrap().clone();
    assert_eq!(recorded, vec!["enter:files:List", "exit:files:List"]);
}

#[tokio::test]
async fn missing_context_short_circuits_in_middleware() {
    let fixture = fixture(contract());
    let response = fixture
        .schema
        .execute(r#"{ files { items { id } } }"#)
        .await;
    assert_eq!(response.errors.len(), 1);
    let error = &response.errors[0];
    assert!(error.message.contains("tenant required"), "{error:?}");
    let code = error
        .extensions
        .as_ref()
        .and_then(|x| x.get("code"))
        .map(|v| format!("{v}"));
    assert_eq!(code.as_deref(), Some("\"unauthorized\""));
    // Short-circuited BEFORE the recorder: nothing recorded.
    assert!(fixture.recorded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn get_returns_row_or_null_and_nullables_render() {
    let fixture = fixture(contract());
    let response = fixture
        .schema
        .execute(tenant_request(r#"{ file(id: "01B") { id size state } }"#))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["file"]["id"], "01B");
    assert_eq!(data["file"]["size"], serde_json::Value::Null);

    let response = fixture
        .schema
        .execute(tenant_request(r#"{ file(id: "NOPE") { id } }"#))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(
        response.data.into_json().unwrap()["file"],
        serde_json::Value::Null,
    );
}

#[tokio::test]
async fn mutations_dispatch_typed_inputs_and_map_outputs() {
    let fixture = fixture(contract());
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"mutation { fileIssueUrl(id: "01A", ttlSecs: 300) }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["fileIssueUrl"]["url"], "https://cdn/01A");
    assert_eq!(data["fileIssueUrl"]["ttl"], 300);

    let response = fixture
        .schema
        .execute(tenant_request(r#"mutation { fileRemove(id: "01A") }"#))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert_eq!(response.data.into_json().unwrap()["fileRemove"], true);

    let recorded = fixture.recorded.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            "enter:files:issue_url",
            "exit:files:issue_url",
            "enter:files:remove",
            "exit:files:remove",
        ]
    );
}

#[tokio::test]
async fn graphql_name_overrides_are_served_and_breaking_to_change() {
    let mut renamed = contract();
    renamed.resources[0].graphql = Some(GraphqlNames {
        type_name: Some("StoredFile".into()),
        list_field: Some("storedFiles".into()),
        get_field: Some("storedFile".into()),
        watch_field: None,
    });
    renamed.resources[0].actions[0].graphql_field = Some("mintUrl".into());

    let fixture = fixture(renamed.clone());
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ storedFiles(state: "ready") { items { id path } } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["storedFiles"]["items"][0]["path"], "a.txt");

    let sdl = fixture.schema.sdl();
    assert!(sdl.contains("type StoredFile "), "{sdl}");
    assert!(sdl.contains("mintUrl"), "{sdl}");

    // Renaming any GraphQL name out from under deployed clients is
    // breaking; effective names are what the differ compares.
    let changes = janus::diff(&contract(), &renamed);
    let breaking: Vec<&str> = changes
        .iter()
        .filter(|c| c.is_breaking())
        .map(janus::Change::message)
        .collect();
    let joined = breaking.join("\n");
    assert!(
        joined.contains("graphql type renamed File -> StoredFile"),
        "{joined}"
    );
    assert!(
        joined.contains("graphql list field renamed files -> storedFiles"),
        "{joined}"
    );
    assert!(
        joined.contains("graphql get field renamed file -> storedFile"),
        "{joined}"
    );
    assert!(
        joined.contains("graphql field renamed fileIssueUrl -> mintUrl"),
        "{joined}",
    );
}

#[tokio::test]
async fn dynamic_schema_agrees_with_generated_sdl() {
    let fixture = fixture(contract());
    let live = fixture.schema.sdl();
    for line in [
        "type File ",
        "size: Int\n",
        "path: String!",
        "created_at: DateTime",
        "enum FileSort ",
        "CREATED_AT_DESC",
        "type FilePage ",
        "nextCursor: String",
        "file(id: ID!): File",
        "scalar DateTime",
        "scalar JSON",
    ] {
        assert!(live.contains(line), "live schema missing {line:?}:\n{live}");
    }
    // And the static artifact carries the same shapes.
    let generated = janus::generate_sdl(&contract(), &[file_table()]).unwrap();
    for line in ["enum FileSort {", "file(id: ID!): File", "scalar DateTime"] {
        assert!(generated.contains(line), "generated SDL missing {line:?}");
    }
}

#[tokio::test]
async fn incomplete_resolvers_refuse_to_build() {
    let error = Dispatcher::new(Arc::new(contract()), Resolvers::new(), vec![]).unwrap_err();
    assert!(error.to_string().contains("no list resolver"), "{error}");
}

#[tokio::test]
async fn subscriptions_stream_rows_narrowed_by_the_list_filters() {
    let fixture = fixture(watched_contract());
    let responses: Vec<_> = fixture
        .schema
        .execute_stream(tenant_request(
            r#"subscription { fileChanged(state: "ready") { id path state } }"#,
        ))
        .collect()
        .await;

    assert_eq!(responses.len(), 1, "one row matches the filter");
    assert!(responses[0].errors.is_empty(), "{:?}", responses[0].errors);
    let data = responses[0].data.clone().into_json().unwrap();
    assert_eq!(data["fileChanged"]["id"], "01A");
    assert_eq!(data["fileChanged"]["path"], "a.txt");

    // The filter reached the resolver under its COLUMN name.
    let seen = fixture.seen_watch_args.lock().unwrap().clone().unwrap();
    assert_eq!(seen.filters["state"], "ready");

    // The chain ran ONCE, at open. Two rows would have recorded twice.
    let recorded = fixture.recorded.lock().unwrap().clone();
    assert_eq!(recorded, vec!["enter:files:Watch", "exit:files:Watch"]);
}

#[tokio::test]
async fn subscriptions_are_authorized_before_any_row_flows() {
    let fixture = fixture(watched_contract());
    let responses: Vec<_> = fixture
        .schema
        .execute_stream(r#"subscription { fileChanged { id } }"#)
        .collect()
        .await;

    assert_eq!(responses.len(), 1, "the refusal is the only payload");
    let error = &responses[0].errors[0];
    assert!(error.message.contains("tenant required"), "{error:?}");
    assert!(
        responses[0].data.clone().into_json().unwrap().is_null(),
        "no row accompanies the refusal",
    );
    assert!(fixture.seen_watch_args.lock().unwrap().is_none());
}

#[tokio::test]
async fn watching_is_declared_and_registered_together_or_not_at_all() {
    // Declared, unregistered: the resource would answer nothing.
    let declared_only = Resolvers::new()
        .list("files", |_ctx, _args| async { Ok(ListOutput::default()) })
        .get("files", |_ctx, _args| async { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async { Ok(None) })
        .action("files", "remove", |_ctx, _args| async { Ok(None) });
    let error = Dispatcher::new(Arc::new(watched_contract()), declared_only, vec![]).unwrap_err();
    assert!(error.to_string().contains("no watch resolver"), "{error}");

    // Registered, undeclared: a resolver nothing can ever call.
    let registered_only = Resolvers::new()
        .list("files", |_ctx, _args| async { Ok(ListOutput::default()) })
        .get("files", |_ctx, _args| async { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async { Ok(None) })
        .action("files", "remove", |_ctx, _args| async { Ok(None) })
        .watch("files", |_ctx, _args| async {
            Ok(Box::pin(stream::empty()) as RowStream)
        });
    let error = Dispatcher::new(Arc::new(contract()), registered_only, vec![]).unwrap_err();
    assert!(
        error.to_string().contains("does not mark it watchable"),
        "{error}",
    );
}

#[tokio::test]
async fn the_subscription_root_appears_only_for_watchable_resources() {
    let quiet = fixture(contract());
    assert!(
        !quiet.schema.sdl().contains("type Subscription"),
        "an unwatchable contract has no Subscription root",
    );

    let live = fixture(watched_contract());
    let served = live.schema.sdl();
    assert!(served.contains("type Subscription"), "{served}");
    assert!(
        served.contains("fileChanged(state: String): File!"),
        "{served}"
    );

    // And the checked-in artifact prints the same root.
    let generated = janus::generate_sdl(&watched_contract(), &[file_table()]).unwrap();
    assert!(
        generated.contains("  fileChanged(state: String): File!"),
        "{generated}",
    );
}

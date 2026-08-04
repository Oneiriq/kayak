//! The runtime end to end: user-defined resolvers behind the
//! contract-enforcing dispatcher, middleware around every operation,
//! and the dynamic GraphQL schema serving what the SDL generator
//! prints.
#![cfg(feature = "graphql")]

use std::sync::{Arc, Mutex};

use async_graphql::futures_util::{stream, StreamExt as _};
use janus::runtime::graphql::build_schema;
use janus::runtime::{
    Dispatcher, Guards, JanusContext, JanusError, ListArgs, ListOutput, MemoryRateStore,
    Middleware, Next, Operation, Outcome, Payload, Principal, Resolvers, RowStream, SortDirection,
    SubListArgs, WatchArgs,
};
use janus::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, GraphqlNames, Query, Resource,
    SubResource, TypeRef,
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

/// The sub-collection's own table, indexed for its own listing.
fn version_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("file_version")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("file")),
            built(int_field("ordinal")),
            built(string_field("digest").nullable(true)),
            built(datetime_field("created_at")),
        ])
        .with_indexes([index("idx_versions", ["file", "created_at"])])
}

/// The same contract with a sub-collection on `files`.
fn contract_with_versions() -> Contract {
    let mut contract = contract();
    contract.resources[0].sub_resources = vec![SubResource {
        name: "versions".into(),
        table: "file_version".into(),
        parent_key: "file".into(),
        fields: vec![
            FieldExposure::column("ordinal"),
            FieldExposure::column("digest"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec![],
        filterable: vec![],
        sortable: vec!["created_at".into()],
        max_page_size: 50,
        description: Some("Every stored version of this file.".into()),
        graphql: None,
    }];
    contract
}

fn contract() -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        limits: None,
        rate_classes: vec![],
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
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
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
                    requires: vec![],
                    rate_class: None,
                },
                Action {
                    name: "remove".into(),
                    method: "DELETE".into(),
                    path: "/{id}".into(),
                    input: vec![],
                    output: ActionOutput::None,
                    description: None,
                    graphql_field: None,
                    requires: vec![],
                    rate_class: None,
                },
            ],
            content: None,
        }],
        queries: vec![],
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
    dispatcher: Arc<Dispatcher>,
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

    for declared in &contract.queries.clone() {
        let name = declared.name.clone();
        resolvers = resolvers.query(&name, move |_ctx, args| {
            let term = args
                .input
                .get("q")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            async move {
                Ok(serde_json::json!({
                    "mode": "hybrid",
                    "items": [{ "file": "01A", "excerpt": term }],
                }))
            }
        });
    }

    for sub in &contract.resources[0].sub_resources.clone() {
        let sub_name = sub.name.clone();
        resolvers = resolvers.sub_list("files", &sub_name, move |_ctx, args: SubListArgs| {
            let parent = args.parent_id.clone();
            async move {
                // Two versions of 01A and none of anything else, so the
                // parent id is provably reaching the resolver.
                let items = if parent == "01A" {
                    vec![
                        serde_json::json!({
                            "id": "01A-v1", "ordinal": 1, "digest": "aaa",
                            "created_at": "2026-07-30T00:00:00Z",
                        }),
                        serde_json::json!({
                            "id": "01A-v2", "ordinal": 2, "digest": null,
                            "created_at": "2026-07-30T01:00:00Z",
                        }),
                    ]
                } else {
                    Vec::new()
                };
                Ok(ListOutput {
                    items: items.into_iter().take(args.limit as usize).collect(),
                    next_cursor: None,
                })
            }
        });
    }

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
    let dispatcher = Arc::new(dispatcher);
    let schema = build_schema(&[file_table(), version_table()], dispatcher.clone()).unwrap();
    Fixture {
        schema,
        dispatcher,
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

#[tokio::test]
async fn a_sub_collection_is_reached_through_its_parent() {
    let fixture = fixture(contract_with_versions());
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ file(id: "01A") { path versions(limit: 5, sort: CREATED_AT_DESC) {
                items { id ordinal digest } nextCursor } } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["file"]["path"], "a.txt");
    let items = data["file"]["versions"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["ordinal"], 1);
    // A nullable column on the sub type renders null, same as anywhere.
    assert_eq!(items[1]["digest"], serde_json::Value::Null);

    // The chain wrapped the sub-listing as its own operation.
    let recorded = fixture.recorded.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            "enter:files:Get",
            "exit:files:Get",
            "enter:files:SubList",
            "exit:files:SubList",
        ],
    );
}

#[tokio::test]
async fn a_sub_collection_enforces_its_own_declarations() {
    let fixture = fixture(contract_with_versions());

    // The sub-resource's ceiling is 50, the parent's is 100.
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ file(id: "01A") { versions(limit: 5000) { items { id } } } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(
        data["file"]["versions"]["items"].as_array().unwrap().len(),
        2,
    );

    // A sort the sub-resource never declared is refused.
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ file(id: "01A") { versions(sort: ORDINAL_ASC) { items { id } } } }"#,
        ))
        .await;
    assert!(
        !response.errors.is_empty(),
        "an undeclared sort must refuse"
    );

    // A different parent gets its own rows.
    let response = fixture
        .schema
        .execute(tenant_request(
            r#"{ file(id: "01B") { versions { items { id } } } }"#,
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert!(data["file"]["versions"]["items"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_declared_sub_collection_needs_its_resolver() {
    let bare = Resolvers::new()
        .list("files", |_ctx, _args| async { Ok(ListOutput::default()) })
        .get("files", |_ctx, _args| async { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async { Ok(None) })
        .action("files", "remove", |_ctx, _args| async { Ok(None) });
    let error = Dispatcher::new(Arc::new(contract_with_versions()), bare, vec![]).unwrap_err();
    assert!(
        error.to_string().contains("sub-resource versions"),
        "{error}",
    );
}

#[tokio::test]
async fn sub_collections_reach_every_generated_surface() {
    let declared = contract_with_versions();
    let tables = [file_table(), version_table()];

    let sdl = janus::generate_sdl(&declared, &tables).unwrap();
    assert!(
        sdl.contains(
            "  versions(limit: Int = 50, cursor: String, sort: FileVersionSort): FileVersionPage!"
        ),
        "{sdl}",
    );
    assert!(sdl.contains("type FileVersion {"), "{sdl}");
    assert!(sdl.contains("type FileVersionPage {"), "{sdl}");
    // The type name composes with the parent, so two parents may each
    // carry a `versions` collection.
    assert!(!sdl.contains("type Version {"), "{sdl}");

    let doc = janus::generate_openapi(&declared, &tables).unwrap();
    let listing = &doc["paths"]["/v1/files/{id}/versions"]["get"];
    assert_eq!(listing["operationId"], "list_versions_files");
    assert_eq!(
        listing["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/FileVersionPage",
    );

    let artifacts =
        janus::generate::generate_all(&declared, &tables, janus::generate::TARGETS).unwrap();
    assert!(artifacts["client.rs"].contains("pub async fn list_versions_files"));
    assert!(artifacts["client.ts"].contains("listVersionsFiles(id: string"));
    assert!(artifacts["client.py"].contains("def list_versions_files(self, id: str"));
    assert!(artifacts["client.go"].contains("func (c *Client) ListVersionsFiles(id string"));

    // Adding one is additive; taking it away is not.
    let changes = janus::diff(&contract(), &declared);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");
    let changes = janus::diff(&declared, &contract());
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("sub-resource versions removed")),
        "{changes:?}",
    );
}

#[tokio::test]
async fn declared_limits_bound_what_the_served_schema_accepts() {
    // Without limits, a nested selection with several fields runs.
    let open = fixture(contract());
    let query = r#"{ files(limit: 2) { items { id path state size created_at } nextCursor } }"#;
    let response = open.schema.execute(tenant_request(query)).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    // A depth ceiling refuses the same operation before any resolver.
    let mut capped = contract();
    capped.limits = Some(janus::ContractLimits {
        max_depth: Some(2),
        max_complexity: None,
        max_watches_per_principal: None,
    });
    let shallow = fixture(capped);
    let response = shallow.schema.execute(tenant_request(query)).await;
    assert!(!response.errors.is_empty(), "the depth ceiling refuses");
    assert!(
        shallow.recorded.lock().unwrap().is_empty(),
        "refused before the chain, so nothing recorded",
    );

    // A complexity ceiling refuses alias amplification the same way.
    let mut narrow = contract();
    narrow.limits = Some(janus::ContractLimits {
        max_depth: None,
        max_complexity: Some(3),
        max_watches_per_principal: None,
    });
    let thin = fixture(narrow);
    let response = thin.schema.execute(tenant_request(query)).await;
    assert!(
        !response.errors.is_empty(),
        "the complexity ceiling refuses"
    );
}

#[test]
fn consumption_refusals_carry_their_own_statuses() {
    let too_large = JanusError::PayloadTooLarge("4 GiB".into());
    assert_eq!(too_large.status(), 413);
    assert_eq!(too_large.code(), "payload_too_large");

    let metered = JanusError::TooManyRequests("rate class exceeded".into());
    assert_eq!(metered.status(), 429);
    assert_eq!(metered.code(), "too_many_requests");
}

/// The contract with a read scope on the resource and a write scope
/// on one action.
fn scoped_contract() -> Contract {
    let mut contract = contract();
    contract.resources[0].reads_require = vec!["files_read".into()];
    contract.resources[0].actions[1].requires = vec!["files_write".into()];
    contract
}

/// A request whose caller holds the given scopes, beside the tenant
/// the middleware requires.
fn scoped_request(query: &str, scopes: &[&str]) -> async_graphql::Request {
    let principal = Principal::new("key-01", scopes.iter().map(|s| s.to_string()));
    async_graphql::Request::new(query).data(
        JanusContext::new()
            .with(Tenant("acme".into()))
            .with(principal),
    )
}

#[tokio::test]
async fn declared_scopes_gate_reads_and_actions() {
    let fixture = fixture(scoped_contract());

    // An identified caller without the read scope is refused by name.
    let response = fixture
        .schema
        .execute(scoped_request(r#"{ files { items { id } } }"#, &["other"]))
        .await;
    let error = &response.errors[0];
    assert!(error.message.contains("files_read"), "{error:?}");
    let code = error
        .extensions
        .as_ref()
        .and_then(|x| x.get("code"))
        .map(|v| format!("{v}"));
    assert_eq!(code.as_deref(), Some("\"forbidden\""));

    // An anonymous caller is a different refusal: no identity at all.
    let response = fixture
        .schema
        .execute(tenant_request(r#"{ files { items { id } } }"#))
        .await;
    let error = &response.errors[0];
    let code = error
        .extensions
        .as_ref()
        .and_then(|x| x.get("code"))
        .map(|v| format!("{v}"));
    assert_eq!(code.as_deref(), Some("\"unauthorized\""));

    // The right scope passes, and the read scope does not leak into
    // permission for the guarded action.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ files { items { id } } }"#,
            &["files_read"],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    let response = fixture
        .schema
        .execute(scoped_request(
            r#"mutation { fileRemove(id: "01A") }"#,
            &["files_read"],
        ))
        .await;
    assert!(
        response.errors[0].message.contains("files_write"),
        "{:?}",
        response.errors,
    );
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"mutation { fileRemove(id: "01A") }"#,
            &["files_read", "files_write"],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    // The unguarded action stays open to any admitted caller.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"mutation { fileIssueUrl(id: "01A", ttlSecs: 60) }"#,
            &[],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[tokio::test]
async fn scopes_gate_subscriptions_at_open() {
    let mut contract = contract_with_versions();
    contract.resources[0].watchable = true;
    contract.resources[0].reads_require = vec!["files_read".into()];

    let fixture = fixture(contract);
    let responses: Vec<_> = fixture
        .schema
        .execute_stream(scoped_request(
            r#"subscription { fileChanged { id } }"#,
            &["other"],
        ))
        .collect()
        .await;
    assert_eq!(responses.len(), 1, "the refusal is the only payload");
    assert!(
        responses[0].errors[0].message.contains("files_read"),
        "{:?}",
        responses[0].errors,
    );
    assert!(fixture.seen_watch_args.lock().unwrap().is_none());

    // Sub-collections read under the parent requirement too.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ file(id: "01A") { versions { items { id } } } }"#,
            &["files_read"],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[test]
fn scope_tightening_is_breaking_and_visible() {
    let open = contract();
    let scoped = scoped_contract();

    let changes = janus::diff(&open, &scoped);
    let breaking: Vec<&str> = changes
        .iter()
        .filter(|c| c.is_breaking())
        .map(janus::Change::message)
        .collect();
    let joined = breaking.join(
        "
",
    );
    assert!(
        joined.contains("reads now require scope files_read"),
        "{joined}",
    );
    assert!(
        joined.contains("action remove now requires scope files_write"),
        "{joined}",
    );
    // Loosening refuses nothing.
    let changes = janus::diff(&scoped, &open);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");

    // The document says what the dispatcher will enforce.
    let doc = janus::generate_openapi(&scoped, &[file_table(), version_table()]).unwrap();
    assert_eq!(
        doc["paths"]["/v1/files"]["get"]["x-requires-scopes"][0],
        "files_read",
    );
    assert_eq!(
        doc["paths"]["/v1/files/{id}"]["delete"]["x-requires-scopes"][0],
        "files_write",
    );
    assert!(doc["paths"]["/v1/files/{id}/url"]["post"]
        .get("x-requires-scopes")
        .is_none());
}

/// The contract with a metered read class: 25 units a minute, so one
/// ten-row page fits twice and a third exhausts it.
fn metered_contract() -> Contract {
    let mut contract = contract();
    contract.rate_classes = vec![janus::RateClass {
        name: "reads".into(),
        units_per_minute: 25,
    }];
    contract.resources[0].rate_class = Some("reads".into());
    contract
}

/// A fixture over a dispatcher built with the given rate store.
fn metered_fixture(contract: Contract) -> Fixture {
    let seen_list_args = Arc::new(Mutex::new(None));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let capture = seen_list_args.clone();
    let resolvers = Resolvers::new()
        .list("files", move |_ctx, args: ListArgs| {
            let capture = capture.clone();
            async move {
                *capture.lock().unwrap() = Some(args.clone());
                Ok(ListOutput::default())
            }
        })
        .get("files", |_ctx, _args| async { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async {
            Ok(Some(serde_json::json!({})))
        })
        .action("files", "remove", |_ctx, _args| async { Ok(None) });
    let dispatcher = Dispatcher::with_rate_store(
        Arc::new(contract),
        resolvers,
        vec![Arc::new(RequireTenant) as Arc<dyn Middleware>],
        Arc::new(MemoryRateStore::new()),
    )
    .unwrap();
    let dispatcher = Arc::new(dispatcher);
    let schema = build_schema(&[file_table(), version_table()], dispatcher.clone()).unwrap();
    Fixture {
        schema,
        dispatcher,
        seen_list_args,
        seen_watch_args: Arc::new(Mutex::new(None)),
        recorded,
    }
}

#[tokio::test]
async fn a_metered_read_spends_its_row_limit_and_exhausts() {
    let fixture = metered_fixture(metered_contract());
    let query = r#"{ files(limit: 10) { items { id } } }"#;

    // Two ten-row pages fit a 25-unit budget; the third refuses with
    // the retryable code, naming the class.
    for _ in 0..2 {
        let response = fixture.schema.execute(scoped_request(query, &[])).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    }
    let response = fixture.schema.execute(scoped_request(query, &[])).await;
    let error = &response.errors[0];
    assert!(error.message.contains("reads"), "{error:?}");
    let code = error
        .extensions
        .as_ref()
        .and_then(|x| x.get("code"))
        .map(|v| format!("{v}"));
    assert_eq!(code.as_deref(), Some("\"too_many_requests\""));

    // The refusal happened before the resolver: only two listings ran.
    // A different principal has its own bucket and still passes.
    let principal = Principal::new("key-02", std::iter::empty());
    let other = async_graphql::Request::new(query).data(
        JanusContext::new()
            .with(Tenant("acme".into()))
            .with(principal),
    );
    let response = fixture.schema.execute(other).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    // Unmetered operations spend nothing whoever asks.
    let response = fixture
        .schema
        .execute(scoped_request(r#"mutation { fileRemove(id: "01A") }"#, &[]))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
}

#[test]
fn a_metered_contract_refuses_to_build_without_a_ledger() {
    let resolvers = Resolvers::new()
        .list("files", |_ctx, _args| async { Ok(ListOutput::default()) })
        .get("files", |_ctx, _args| async { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async { Ok(None) })
        .action("files", "remove", |_ctx, _args| async { Ok(None) });
    let error = Dispatcher::new(Arc::new(metered_contract()), resolvers, vec![]).unwrap_err();
    assert!(
        error.to_string().contains("no rate store is registered"),
        "{error}",
    );
}

#[test]
fn rate_movement_diffs_and_undefined_classes_refuse() {
    let open = contract();
    let metered = metered_contract();

    // Attaching a class to unmetered reads introduces refusals.
    let changes = janus::diff(&open, &metered);
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("now metered by rate class reads")),
        "{changes:?}",
    );
    // Shrinking a budget is breaking; growing it refuses nothing.
    let mut shrunk = metered_contract();
    shrunk.rate_classes[0].units_per_minute = 10;
    let changes = janus::diff(&metered, &shrunk);
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("budget lowered 25 -> 10")),
        "{changes:?}",
    );
    let changes = janus::diff(&shrunk, &metered);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");
    let changes = janus::diff(&metered, &open);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");

    // A reference to a class the contract never defines refuses at
    // validation, by name.
    let mut dangling = contract();
    dangling.resources[0].rate_class = Some("phantom".into());
    let violations = janus::validate(&dangling, &[file_table()]);
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("undefined rate class")),
        "{violations:?}",
    );
}

/// The contract with its sensitive columns guarded: `state` (which is
/// also filterable) and `created_at` (which is sortable), plus the
/// sub-collection's digest. One guard decides all three.
fn guarded_contract() -> Contract {
    let mut contract = contract_with_versions();
    contract.resources[0].watchable = true;
    contract.resources[0].fields = vec![
        FieldExposure::column("path"),
        FieldExposure::column("state").with_guard("audit_only"),
        FieldExposure::renamed("size_bytes", "size"),
        FieldExposure::column("created_at").with_guard("audit_only"),
    ];
    contract.resources[0].sub_resources[0].fields = vec![
        FieldExposure::column("ordinal"),
        FieldExposure::column("digest").with_guard("audit_only"),
        FieldExposure::column("created_at"),
    ];
    contract
}

/// A fixture whose dispatcher carries the audit_only guard: visible
/// exactly to principals holding the audit scope.
fn guarded_fixture() -> Fixture {
    let seen_watch_args = Arc::new(Mutex::new(None));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let watch_capture = seen_watch_args.clone();
    let resolvers = Resolvers::new()
        .list("files", |_ctx, args: ListArgs| async move {
            let mut items = rows();
            items.truncate(args.limit as usize);
            Ok(ListOutput {
                items,
                next_cursor: None,
            })
        })
        .get("files", |_ctx, args| async move {
            Ok(rows().into_iter().find(|row| row["id"] == *args.id))
        })
        .sub_list("files", "versions", |_ctx, _args: SubListArgs| async move {
            Ok(ListOutput {
                items: vec![serde_json::json!({
                    "id": "01A-v1", "ordinal": 1, "digest": "aaa",
                    "created_at": "2026-07-30T00:00:00Z",
                })],
                next_cursor: None,
            })
        })
        .action("files", "issue_url", |_ctx, _args| async move {
            // A Json action whose payload happens to carry a key named
            // like a guarded field. Free-form output is the action's
            // own shape; projection must not eat it.
            Ok(Some(
                serde_json::json!({ "url": "https://cdn/x", "state": "minted" }),
            ))
        })
        .action("files", "remove", |_ctx, _args| async { Ok(None) })
        .watch("files", move |_ctx, args: WatchArgs| {
            let capture = watch_capture.clone();
            async move {
                *capture.lock().unwrap() = Some(args.clone());
                let items: Vec<_> = rows().into_iter().map(Ok).collect();
                Ok(Box::pin(stream::iter(items)) as RowStream)
            }
        });
    let guards = Guards::new().guard("audit_only", |ctx, _row| {
        ctx.get::<Principal>().is_some_and(|p| p.has("audit"))
    });
    let dispatcher = Dispatcher::with_policies(
        Arc::new(guarded_contract()),
        resolvers,
        vec![Arc::new(RequireTenant) as Arc<dyn Middleware>],
        None,
        guards,
    )
    .unwrap();
    let dispatcher = Arc::new(dispatcher);
    let schema = build_schema(&[file_table(), version_table()], dispatcher.clone()).unwrap();
    Fixture {
        schema,
        dispatcher,
        seen_list_args: Arc::new(Mutex::new(None)),
        seen_watch_args,
        recorded,
    }
}

#[tokio::test]
async fn guarded_fields_are_omitted_for_callers_the_guard_denies() {
    let fixture = guarded_fixture();
    let query = r#"{ files { items { id path state created_at } } }"#;

    // Denied: the guarded fields render null; the open ones survive.
    let response = fixture
        .schema
        .execute(scoped_request(query, &["other"]))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let row = &data["files"]["items"][0];
    assert_eq!(row["path"], "a.txt");
    assert!(row["state"].is_null(), "{row:?}");
    assert!(row["created_at"].is_null(), "{row:?}");

    // Allowed: the same query shows everything.
    let response = fixture
        .schema
        .execute(scoped_request(query, &["audit"]))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["files"]["items"][0]["state"], "ready");

    // The get face projects identically.
    let response = fixture
        .schema
        .execute(scoped_request(r#"{ file(id: "01A") { state } }"#, &[]))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    assert!(response.data.into_json().unwrap()["file"]["state"].is_null());

    // Sub-collection rows project through their own field list.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ file(id: "01A") { versions { items { ordinal digest } } } }"#,
            &[],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let version = &data["file"]["versions"]["items"][0];
    assert_eq!(version["ordinal"], 1);
    assert!(version["digest"].is_null(), "{version:?}");

    // A Json action's own keys are its own shape, never projected.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"mutation { fileIssueUrl(id: "01A") }"#,
            &[],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["fileIssueUrl"]["state"], "minted");
}

#[tokio::test]
async fn hidden_columns_refuse_narrowing_and_streams_project() {
    let fixture = guarded_fixture();

    // Filtering on a column the caller cannot see is reading it.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ files(state: "quarantined") { items { id } } }"#,
            &["other"],
        ))
        .await;
    let error = &response.errors[0];
    assert!(
        error.message.contains("requires permission to see it"),
        "{error:?}",
    );

    // Sorting leaks ordering the same way.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ files(sort: CREATED_AT_DESC) { items { id } } }"#,
            &[],
        ))
        .await;
    assert!(
        response.errors[0].message.contains("sorting on created_at"),
        "{:?}",
        response.errors,
    );

    // The audit scope unlocks both.
    let response = fixture
        .schema
        .execute(scoped_request(
            r#"{ files(state: "ready", sort: CREATED_AT_DESC) { items { id state } } }"#,
            &["audit"],
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);

    // Streamed rows project exactly like listed ones.
    let responses: Vec<_> = fixture
        .schema
        .execute_stream(scoped_request(
            r#"subscription { fileChanged { id state } }"#,
            &["other"],
        ))
        .collect()
        .await;
    assert!(!responses.is_empty());
    for response in &responses {
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.clone().into_json().unwrap();
        assert!(
            data["fileChanged"]["state"].is_null(),
            "a streamed row leaked a guarded field: {data:?}",
        );
    }
}

#[test]
fn guards_are_declared_and_registered_together_or_not_at_all() {
    let resolvers = || {
        Resolvers::new()
            .list("files", |_ctx, _args| async { Ok(ListOutput::default()) })
            .get("files", |_ctx, _args| async { Ok(None) })
            .sub_list("files", "versions", |_ctx, _args| async {
                Ok(ListOutput::default())
            })
            .action("files", "issue_url", |_ctx, _args| async { Ok(None) })
            .action("files", "remove", |_ctx, _args| async { Ok(None) })
            .watch("files", |_ctx, _args| async {
                Ok(Box::pin(stream::empty()) as RowStream)
            })
    };

    // Declared, unregistered: would silently show what it hides.
    let error = Dispatcher::new(Arc::new(guarded_contract()), resolvers(), vec![]).unwrap_err();
    assert!(error.to_string().contains("guard audit_only"), "{error}",);

    // Registered, unreferenced: dead policy that reads as live.
    let mut open = contract_with_versions();
    open.resources[0].watchable = true;
    let guards = Guards::new().guard("audit_only", |_, _| true);
    let error =
        Dispatcher::with_policies(Arc::new(open), resolvers(), vec![], None, guards).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no field in the contract names it"),
        "{error}",
    );
}

#[test]
fn guarded_renders_nullable_everywhere_and_diffs_as_breaking() {
    let tables = [file_table(), version_table()];
    let guarded = guarded_contract();

    // SDL and the served schema drop the bang on guarded fields.
    let sdl = janus::generate_sdl(&guarded, &tables).unwrap();
    assert!(
        sdl.contains(
            "  state: String
"
        ),
        "{sdl}"
    );
    assert!(
        sdl.contains(
            "  created_at: DateTime
"
        ),
        "{sdl}"
    );
    assert!(sdl.contains("  path: String!"), "{sdl}");
    assert!(
        sdl.contains(
            "  digest: String
"
        ),
        "{sdl}"
    );

    // OpenAPI: guarded fields leave required and carry the policy.
    let doc = janus::generate_openapi(&guarded, &tables).unwrap();
    let schema = &doc["components"]["schemas"]["File"];
    assert_eq!(schema["properties"]["state"]["x-guard"], "audit_only");
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(required.contains(&"path"));
    assert!(!required.contains(&"state"), "{required:?}");

    // Clients render the guarded field optional in every language.
    let artifacts =
        janus::generate::generate_all(&guarded, &tables, janus::generate::TARGETS).unwrap();
    assert!(artifacts["client.rs"].contains("pub state: Option<String>,"));
    assert!(artifacts["client.ts"].contains("state?: string"));
    assert!(artifacts["client.py"].contains("state: str | None = None"));
    assert!(artifacts["client.go"].contains("State *string"));

    // Guarding an open field is breaking; unguarding is compatible.
    let mut open = contract_with_versions();
    open.resources[0].watchable = true;
    let changes = janus::diff(&open, &guarded);
    assert!(
        changes.iter().any(|c| c.is_breaking()
            && c.message()
                .contains("field state now guarded by audit_only")),
        "{changes:?}",
    );
    let changes = janus::diff(&guarded, &open);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");

    // Swapping the policy behind a field changes who sees it.
    let mut swapped = guarded_contract();
    swapped.resources[0].fields[1] = FieldExposure::column("state").with_guard("owner_only");
    let changes = janus::diff(&guarded, &swapped);
    assert!(
        changes.iter().any(|c| c.is_breaking()
            && c.message()
                .contains("guard changed audit_only -> owner_only")),
        "{changes:?}",
    );
}

#[test]
fn a_hand_written_face_computes_the_same_hidden_set() {
    let contract = guarded_contract();
    let guards = Guards::new().guard("audit_only", |ctx: &JanusContext, _row| {
        ctx.get::<Principal>().is_some_and(|p| p.has("audit"))
    });

    let denied = JanusContext::new().with(Principal::new("k1", std::iter::empty()));
    let hidden = janus::runtime::hidden_fields(&contract, "files", None, &guards, &denied);
    let names: Vec<&str> = hidden.iter().map(|h| h.api_name.as_str()).collect();
    assert_eq!(names, vec!["state", "created_at"], "{hidden:?}");

    // Stripping a wire row removes exactly those keys, matching what
    // the dispatcher projects.
    let mut row = serde_json::json!({
        "id": "01A", "path": "a.txt", "state": "ready",
        "created_at": "2026-07-30T00:00:00Z",
    });
    janus::runtime::strip_hidden(&mut row, &hidden);
    assert_eq!(row, serde_json::json!({ "id": "01A", "path": "a.txt" }),);

    // Sub-collections resolve through the parent, and an allowed
    // caller hides nothing.
    let sub = janus::runtime::hidden_fields(&contract, "files", Some("versions"), &guards, &denied);
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].api_name, "digest");
    let allowed = JanusContext::new().with(Principal::new("k2", ["audit".to_owned()]));
    assert!(janus::runtime::hidden_fields(&contract, "files", None, &guards, &allowed).is_empty());
}

#[tokio::test]
async fn the_watch_ceiling_holds_and_slots_free_on_drop() {
    let mut contract = contract_with_versions();
    contract.resources[0].watchable = true;
    contract.limits = Some(janus::ContractLimits {
        max_depth: None,
        max_complexity: None,
        max_watches_per_principal: Some(1),
    });
    let fixture = fixture(contract);
    let query = r#"subscription { fileChanged { id } }"#;

    // The first subscription holds the caller's only slot. The stream
    // stays OPEN by holding the response stream unpolled to
    // completion: polling one item keeps the slot alive.
    let mut first = fixture.schema.execute_stream(scoped_request(query, &[]));
    let opening = first.next().await.expect("the stream yields");
    assert!(opening.errors.is_empty(), "{:?}", opening.errors);

    // A second open by the SAME caller refuses with the retryable
    // code; a different principal has its own slots.
    let refused: Vec<_> = fixture
        .schema
        .execute_stream(scoped_request(query, &[]))
        .collect()
        .await;
    let error = &refused[0].errors[0];
    assert!(error.message.contains("watch ceiling"), "{error:?}");
    let code = error
        .extensions
        .as_ref()
        .and_then(|x| x.get("code"))
        .map(|v| format!("{v}"));
    assert_eq!(code.as_deref(), Some("\"too_many_requests\""));

    let other = async_graphql::Request::new(query).data(
        JanusContext::new()
            .with(Tenant("acme".into()))
            .with(Principal::new("key-two", std::iter::empty())),
    );
    let opened: Vec<_> = fixture.schema.execute_stream(other).collect().await;
    assert!(
        opened.iter().all(|r| r.errors.is_empty()),
        "{:?}",
        opened.iter().map(|r| &r.errors).collect::<Vec<_>>(),
    );

    // Dropping the held stream frees the slot; the same caller opens
    // again.
    drop(first);
    let reopened: Vec<_> = fixture
        .schema
        .execute_stream(scoped_request(query, &[]))
        .collect()
        .await;
    assert!(
        reopened.iter().all(|r| r.errors.is_empty()),
        "{:?}",
        reopened.iter().map(|r| &r.errors).collect::<Vec<_>>(),
    );
}

fn searching_contract() -> Contract {
    let mut contract = contract();
    contract.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: vec![
            ActionField {
                name: "q".into(),
                kind: TypeRef::String,
                required: true,
                description: None,
            },
            ActionField {
                name: "limit".into(),
                kind: TypeRef::Int,
                required: false,
                description: None,
            },
        ],
        description: Some("Retrieval across the tenant's text.".into()),
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    }];
    contract
}

fn scoped_searching_contract() -> Contract {
    let mut contract = searching_contract();
    contract.queries[0].requires = vec!["read".into()];
    contract
}

/// A query answers on the GraphQL face through the same dispatcher
/// every other operation uses, with its declared arguments checked.
#[tokio::test]
async fn contract_queries_answer_on_the_graphql_face() {
    let fixture = fixture(searching_contract());
    let response = fixture
        .schema
        .execute(tenant_request(r#"{ search(q: "invoice", limit: 5) }"#))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["search"]["items"][0]["excerpt"], "invoice");

    // The chain ran around it, so middleware, scopes, and metering
    // apply to queries exactly as they apply to listings.
    let recorded = fixture.recorded.lock().unwrap().clone();
    assert_eq!(recorded, vec!["enter:search:Query", "exit:search:Query"]);
}

/// The declaration is the contract: a missing required parameter and
/// an undeclared one both refuse before any resolver runs.
#[tokio::test]
async fn query_parameters_answer_to_their_declaration() {
    let fixture = fixture(searching_contract());
    let response = fixture.schema.execute(tenant_request("{ search }")).await;
    assert!(
        !response.errors.is_empty(),
        "a required argument is missing"
    );

    let response = fixture
        .schema
        .execute(tenant_request(r#"{ search(q: "x", nope: 1) }"#))
        .await;
    assert!(!response.errors.is_empty(), "an undeclared argument");
}

/// Both directions of the completeness gate: a declared query with no
/// resolver refuses to build, and a resolver for a query the contract
/// never declared refuses too.
#[test]
fn query_resolvers_are_gated_in_both_directions() {
    let contract = searching_contract();
    let err = Dispatcher::new(Arc::new(contract.clone()), Resolvers::new(), vec![]).unwrap_err();
    assert!(format!("{err}").contains("no list resolver"), "{err}");

    let complete = Resolvers::new()
        .list("files", |_ctx, _args: ListArgs| async move {
            Ok(ListOutput::default())
        })
        .get("files", |_ctx, _args| async move { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async move { Ok(None) })
        .action("files", "remove", |_ctx, _args| async move { Ok(None) })
        .sub_list("files", "versions", |_ctx, _args: SubListArgs| async move {
            Ok(ListOutput::default())
        });
    let err = Dispatcher::new(Arc::new(contract.clone()), complete, vec![]).unwrap_err();
    assert!(format!("{err}").contains("query search"), "{err}");

    let stray = Resolvers::new()
        .list("files", |_ctx, _args: ListArgs| async move {
            Ok(ListOutput::default())
        })
        .get("files", |_ctx, _args| async move { Ok(None) })
        .action("files", "issue_url", |_ctx, _args| async move { Ok(None) })
        .action("files", "remove", |_ctx, _args| async move { Ok(None) })
        .sub_list("files", "versions", |_ctx, _args: SubListArgs| async move {
            Ok(ListOutput::default())
        })
        .query(
            "search",
            |_ctx, _args| async move { Ok(serde_json::json!({})) },
        )
        .query(
            "nowhere",
            |_ctx, _args| async move { Ok(serde_json::json!({})) },
        );
    let err = Dispatcher::new(Arc::new(contract), stray, vec![]).unwrap_err();
    assert!(format!("{err}").contains("nowhere"), "{err}");
}

/// A query's scopes are enforced by the same path resources use: an
/// unidentified caller is refused before the resolver runs.
#[tokio::test]
async fn query_scopes_refuse_unidentified_callers() {
    let fixture = fixture(scoped_searching_contract());
    let response = fixture
        .schema
        .execute(tenant_request(r#"{ search(q: "invoice") }"#))
        .await;
    let message = format!("{:?}", response.errors);
    assert!(message.contains("unauthorized"), "{message}");
}

/// An ownership guard sees each row: the caller keeps the field on
/// rows they own and loses it on the rest, within one listing. The
/// rowless form still refuses narrowing, because a caller who only
/// partially sees a column must not filter by it.
#[tokio::test]
async fn ownership_guards_project_row_by_row() {
    let mut contract = guarded_contract();
    contract.resources[0].fields = vec![
        FieldExposure::column("path"),
        FieldExposure::column("state").with_guard("owner_or_admin"),
    ];
    contract.resources[0].filterable = vec!["state".into()];
    // The inherited fixture guards its versions sub-collection with a
    // different guard and declares watchability; this test is about
    // the resource's own fields.
    contract.resources[0].sub_resources.clear();
    contract.resources[0].watchable = false;
    contract.resources[0].actions.clear();
    let resolvers = Resolvers::new()
        .list("files", |_ctx, _args: ListArgs| async move {
            Ok(ListOutput {
                items: vec![
                    serde_json::json!({
                        "id": "01A", "path": "mine.txt", "state": "ready",
                        "created_by": "alice",
                    }),
                    serde_json::json!({
                        "id": "01B", "path": "theirs.txt", "state": "ready",
                        "created_by": "bob",
                    }),
                ],
                next_cursor: None,
            })
        })
        .get("files", |_ctx, _args| async move { Ok(None) });
    let guards = Guards::new().guard("owner_or_admin", |ctx, row| {
        let Some(principal) = ctx.get::<Principal>() else {
            return false;
        };
        if principal.has("admin") {
            return true;
        }
        row.and_then(|r| r.get("created_by"))
            .and_then(|v| v.as_str())
            .is_some_and(|owner| owner == principal.subject)
    });
    let dispatcher = Dispatcher::with_policies(
        Arc::new(contract),
        resolvers,
        vec![Arc::new(RequireTenant) as Arc<dyn Middleware>],
        None,
        guards,
    )
    .unwrap();
    let schema = build_schema(&[file_table()], Arc::new(dispatcher)).unwrap();

    let alice = |query: &str| {
        async_graphql::Request::new(query.to_owned()).data(
            JanusContext::new()
                .with(Tenant("acme".into()))
                .with(Principal::new("alice", ["read".to_owned()])),
        )
    };

    let response = schema
        .execute(alice(r#"{ files { items { path state } } }"#))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let items = data["files"]["items"].as_array().unwrap();
    assert_eq!(items[0]["path"], "mine.txt");
    assert_eq!(items[0]["state"], "ready", "the owner keeps the field");
    assert!(
        items[1]["state"].is_null(),
        "the stranger loses it: {items:?}"
    );

    // Narrowing by the guarded column refuses for the partial viewer.
    let response = schema
        .execute(alice(r#"{ files(state: "ready") { items { path } } }"#))
        .await;
    assert!(
        !response.errors.is_empty(),
        "filtering a partially visible column must refuse",
    );
}

fn tenant_ctx() -> JanusContext {
    JanusContext::new().with(Tenant("acme".into()))
}

fn rest(fixture: &Fixture) -> janus::runtime::RestRouter {
    janus::runtime::RestRouter::new(fixture.dispatcher.clone())
}

/// The REST face routes what the contract declares, through the same
/// dispatcher and middleware chain as every other face.
#[tokio::test]
async fn rest_lists_gets_and_acts_from_the_declaration() {
    let fixture = fixture(contract_with_versions());
    let router = rest(&fixture);

    let listed = router
        .handle(
            "GET",
            "/v1/files",
            "limit=1&state=ready",
            None,
            tenant_ctx(),
        )
        .await;
    assert_eq!(listed.status, 200, "{:?}", listed.body);
    assert_eq!(listed.body["items"].as_array().unwrap().len(), 1);
    let seen = fixture.seen_list_args.lock().unwrap().clone().unwrap();
    assert_eq!(seen.limit, 1);
    assert_eq!(
        seen.filters.get("state"),
        Some(&serde_json::Value::String("ready".into())),
        "query pairs beyond paging become filters",
    );

    let fetched = router
        .handle("GET", "/v1/files/01A", "", None, tenant_ctx())
        .await;
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body["id"], serde_json::json!("01A"));

    let absent = router
        .handle("GET", "/v1/files/nope", "", None, tenant_ctx())
        .await;
    assert_eq!(absent.status, 404);

    let acted = router
        .handle(
            "POST",
            "/v1/files/01A/url",
            "",
            Some(serde_json::json!({ "ttl_secs": 60 })),
            tenant_ctx(),
        )
        .await;
    assert_eq!(acted.status, 200, "{:?}", acted.body);

    let subbed = router
        .handle(
            "GET",
            "/v1/files/01A/versions",
            "limit=5",
            None,
            tenant_ctx(),
        )
        .await;
    assert_eq!(subbed.status, 200, "{:?}", subbed.body);
    assert!(subbed.body["items"].is_array());
}

/// Refusals carry REST statuses: unknown paths 404, known paths with
/// the wrong method 405, refused identity as the middleware decides,
/// malformed paging 400.
#[tokio::test]
async fn rest_refuses_with_status_shaped_answers() {
    let fixture = fixture(contract());
    let router = rest(&fixture);

    let unknown = router
        .handle("GET", "/v1/nothing", "", None, tenant_ctx())
        .await;
    assert_eq!(unknown.status, 404);

    let wrong_method = router
        .handle("DELETE", "/v1/files", "", None, tenant_ctx())
        .await;
    assert_eq!(wrong_method.status, 405);

    let anonymous = router
        .handle("GET", "/v1/files", "", None, JanusContext::new())
        .await;
    assert_eq!(anonymous.status, 401, "{:?}", anonymous.body);

    let garbled = router
        .handle("GET", "/v1/files", "limit=many", None, tenant_ctx())
        .await;
    assert_eq!(garbled.status, 400);
}

/// A contract query serves at its declared path with its declared
/// parameter types coerced from the query string.
#[tokio::test]
async fn rest_serves_contract_queries_with_typed_parameters() {
    let fixture = fixture(searching_contract());
    let router = rest(&fixture);

    let answered = router
        .handle(
            "GET",
            "/v1/search",
            "q=hello%20world&limit=3",
            None,
            tenant_ctx(),
        )
        .await;
    assert_eq!(answered.status, 200, "{:?}", answered.body);
    assert_eq!(
        answered.body["items"][0]["excerpt"],
        serde_json::json!("hello world"),
        "the percent-decoded term reached the resolver",
    );
}

#[cfg(feature = "console")]
mod console_pages {
    use super::*;
    use janus::runtime::{ConsoleConfig, ConsoleRouter, FormOutcome};

    fn console(fixture: &Fixture) -> ConsoleRouter {
        ConsoleRouter::new(
            fixture.dispatcher.clone(),
            ConsoleConfig {
                base: "/console".to_owned(),
                title: "test".to_owned(),
            },
        )
    }

    /// The overview and the resource pages render from the
    /// declaration, through the dispatcher, with caller input
    /// escaped on the way back out.
    #[tokio::test]
    async fn pages_render_from_the_declaration() {
        let fixture = fixture(contract_with_versions());
        let console = console(&fixture);

        let overview = console.page("/", "", tenant_ctx()).await;
        assert_eq!(overview.status, 200);
        assert!(overview.html.contains("/console/r/files"));

        let listing = console.page("/r/files", "", tenant_ctx()).await;
        assert_eq!(listing.status, 200);
        assert!(listing.html.contains("01A"), "fixture rows render");
        assert!(
            listing.html.contains("/console/r/files/01A"),
            "ids link to the detail page",
        );

        let detail = console.page("/r/files/01A", "", tenant_ctx()).await;
        assert_eq!(detail.status, 200);
        assert!(detail.html.contains("a.txt"), "fields render");
        assert!(detail.html.contains("versions"), "sub-collections render");
        assert!(
            detail.html.contains("/console/r/files/01A/a/issue_url"),
            "declared actions become forms",
        );

        let absent = console.page("/r/files/nope", "", tenant_ctx()).await;
        assert_eq!(absent.status, 404);
    }

    /// A hostile filter value comes back escaped, never as markup.
    #[tokio::test]
    async fn caller_input_is_escaped() {
        let fixture = fixture(contract_with_versions());
        let console = console(&fixture);
        let page = console
            .page(
                "/r/files",
                "state=%3Cscript%3Ealert(1)%3C/script%3E",
                tenant_ctx(),
            )
            .await;
        assert!(
            !page.html.contains("<script>alert"),
            "markup in caller input must not survive",
        );
    }

    /// An action that answers shows what it answered, because the
    /// answer is often the point: a signed URL, a secret that appears
    /// once. An action that answers nothing redirects back instead.
    #[tokio::test]
    async fn forms_dispatch_actions() {
        let fixture = fixture(contract_with_versions());
        let console = console(&fixture);

        let outcome = console
            .submit(
                "/r/files/01A/a/issue_url",
                &[("ttl_secs".to_owned(), "60".to_owned())],
                tenant_ctx(),
            )
            .await;
        match outcome {
            FormOutcome::Page(page) => {
                assert_eq!(page.status, 200);
                assert!(
                    page.html.contains("https://cdn/01A"),
                    "the answer is shown, never discarded",
                );
                assert!(
                    page.html.contains("/console/r/files/01A?done=issue_url"),
                    "with a way back to the instance",
                );
            }
            FormOutcome::Redirect(target) => {
                panic!("an answer must not be thrown away: {target}")
            }
        }

        // `remove` answers nothing, so the redirect stands.
        let silent = console
            .submit("/r/files/01A/a/remove", &[], tenant_ctx())
            .await;
        match silent {
            FormOutcome::Redirect(target) => {
                assert!(target.contains("/console/r/files/01A?done=remove"));
            }
            FormOutcome::Page(page) => panic!("expected a redirect, got {}", page.status),
        }

        let refused = console
            .submit(
                "/r/files/01A/a/issue_url",
                &[("ttl_secs".to_owned(), "60".to_owned())],
                JanusContext::new(),
            )
            .await;
        match refused {
            FormOutcome::Page(page) => assert_eq!(page.status, 401),
            FormOutcome::Redirect(target) => panic!("expected refusal, got {target}"),
        }
    }

    /// The confirmation reaches the instance page as well as the
    /// listing.
    #[tokio::test]
    async fn a_silent_action_confirms_on_the_instance() {
        let fixture = fixture(contract_with_versions());
        let console = console(&fixture);
        let page = console
            .page("/r/files/01A", "done=remove", tenant_ctx())
            .await;
        assert!(
            page.html.contains("action remove completed"),
            "the instance page reads the outcome it was sent back with",
        );
    }

    /// The sort control carries what is applied, so the next narrow
    /// keeps it.
    #[tokio::test]
    async fn the_sort_control_remembers() {
        let fixture = fixture(contract_with_versions());
        let console = console(&fixture);
        let page = console
            .page("/r/files", "sort=created_at:desc", tenant_ctx())
            .await;
        assert!(
            page.html
                .contains(r#"<option value="created_at:desc" selected>"#),
            "the applied sort is the selected option",
        );
        let bare = console.page("/r/files", "", tenant_ctx()).await;
        assert!(
            bare.html.contains(r#"<option value="" selected>"#),
            "declared order is selected when nothing is applied",
        );
    }

    /// A JSON-declared field refuses what is not JSON, rather than
    /// passing a string along for a resolver to store as nonsense.
    #[tokio::test]
    async fn a_json_field_refuses_prose() {
        let mut contract = contract_with_versions();
        contract.resources[0].actions[0].input.push(ActionField {
            name: "labels".into(),
            kind: TypeRef::Json,
            required: false,
            description: None,
        });
        let fixture = fixture(contract);
        let console = console(&fixture);

        let refused = console
            .submit(
                "/r/files/01A/a/issue_url",
                &[("labels".to_owned(), "one, two".to_owned())],
                tenant_ctx(),
            )
            .await;
        match refused {
            FormOutcome::Page(page) => {
                assert_eq!(page.status, 400);
                assert!(page.html.contains("labels takes JSON"));
            }
            FormOutcome::Redirect(target) => panic!("expected a refusal, got {target}"),
        }

        let accepted = console
            .submit(
                "/r/files/01A/a/issue_url",
                &[("labels".to_owned(), r#"["one", "two"]"#.to_owned())],
                tenant_ctx(),
            )
            .await;
        assert!(
            matches!(accepted, FormOutcome::Page(ref page) if page.status == 200),
            "well-formed JSON passes",
        );
    }

    /// The query panel runs a declared query with decoded, typed
    /// parameters.
    #[tokio::test]
    async fn the_query_panel_answers() {
        let fixture = fixture(searching_contract());
        let console = console(&fixture);
        let page = console
            .page("/q/search", "q=hello+world&limit=3", tenant_ctx())
            .await;
        assert_eq!(page.status, 200);
        assert!(
            page.html.contains("hello world"),
            "the decoded term reached the resolver and rendered",
        );
    }
}

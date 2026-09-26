//! An input that takes several of a set, on every face.
//!
//! The OpenAPI document and the MCP manifest publish such an input as
//! an array. A caller who follows them sends a JSON array, or on REST
//! the key once per value. The GraphQL schema and the generated clients
//! send one comma-separated string. These tests hold the runtime to all
//! three forms, and hold the resolver to the one shape it reads.

use serde_json::{json, Value};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

use kayak::{Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, TypeRef};

fn several(name: &str, options: &[&str]) -> ActionField {
    ActionField {
        name: name.into(),
        kind: TypeRef::String,
        required: false,
        multiple: true,
        description: None,
        options: options.iter().map(|option| (*option).to_owned()).collect(),
    }
}

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
            filterable: vec![],
            sortable: vec![],
            max_page_size: 100,
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
            faces: Default::default(),
            actions: vec![Action {
                name: "tag".into(),
                method: "POST".into(),
                path: "/{id}/tag".into(),
                input: vec![several("labels", &["red", "green", "blue"])],
                output: ActionOutput::Json,
                description: None,
                graphql_field: None,
                requires: vec![],
                rate_class: None,
            }],
            content: None,
            filter_options: Default::default(),
        }],
        queries: vec![Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![
                ActionField {
                    name: "q".into(),
                    kind: TypeRef::String,
                    required: true,
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                },
                several("facets", &["content_type", "access"]),
            ],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
            searches: vec![],
            backing: vec![],
        }],
    }
}

fn tables() -> Vec<TableDefinition> {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    vec![table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields([built(string_field("state"))])
        .with_indexes([index("idx_state", ["state"])])]
}

fn tool_named<'a>(manifest: &'a Value, name: &str) -> &'a Value {
    manifest["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap()
}

/// Both documents describe the input as an array of the set. The
/// OpenAPI query parameter names no style, so the default holds, and
/// the default is the key repeated.
#[test]
fn openapi_and_mcp_publish_an_array() {
    let contract = contract();
    let openapi = kayak::generate_openapi(&contract, &tables()).unwrap();
    let parameters = openapi["paths"]["/v1/search"]["get"]["parameters"]
        .as_array()
        .unwrap();
    let facets = parameters.iter().find(|p| p["name"] == "facets").unwrap();
    assert_eq!(facets["in"], "query");
    assert_eq!(facets["schema"]["type"], "array");
    assert_eq!(
        facets["schema"]["items"]["enum"],
        json!(["content_type", "access"]),
    );
    assert!(facets.get("style").is_none(), "{facets}");
    assert!(facets.get("explode").is_none(), "{facets}");
    let body = &openapi["paths"]["/v1/files/{id}/tag"]["post"]["requestBody"]["content"]
        ["application/json"]["schema"];
    assert_eq!(body["properties"]["labels"]["type"], "array", "{body}");

    let manifest = kayak::generate_mcp_tools(&contract).unwrap();
    for (tool, input) in [("search", "facets"), ("file_tag", "labels")] {
        let schema = &tool_named(&manifest, tool)["inputSchema"]["properties"][input];
        assert_eq!(schema["type"], "array", "{tool}: {schema}");
    }
}

#[cfg(feature = "runtime")]
mod runtime {
    use std::sync::Arc;

    use kayak::runtime::{
        ActionArgs, Dispatcher, GetArgs, KayakContext, KayakError, ListArgs, ListOutput, QueryArgs,
        Resolvers, RestRouter,
    };

    use super::*;

    /// A dispatcher whose resolvers answer with the input they were
    /// handed, so a test reads the shape a resolver sees.
    fn dispatcher() -> Arc<Dispatcher> {
        let resolvers = Resolvers::new()
            .list("files", |_ctx, _args: ListArgs| async move {
                Ok(ListOutput::default())
            })
            .get("files", |_ctx, _args: GetArgs| async move { Ok(None) })
            .action("files", "tag", |_ctx, args: ActionArgs| async move {
                Ok(Some(json!({ "labels": args.input.get("labels") })))
            })
            .query("search", |_ctx, args: QueryArgs| async move {
                Ok(json!({ "facets": args.input.get("facets") }))
            });
        Arc::new(Dispatcher::new(Arc::new(contract()), resolvers, vec![]).unwrap())
    }

    async fn rest(method: &str, path: &str, query: &str, body: Option<Value>) -> (u16, Value) {
        let answer = RestRouter::new(dispatcher())
            .handle(method, path, query, body, KayakContext::new())
            .await;
        (answer.status, answer.body)
    }

    /// REST reads the key repeated, the form the OpenAPI document
    /// describes, and the one comma-separated value the generated
    /// clients send. Before, a repeated key kept only its last value.
    #[tokio::test]
    async fn rest_takes_the_key_repeated_or_one_comma_separated_value() {
        for query in [
            "q=x&facets=content_type&facets=access",
            "q=x&facets=content_type,access",
            "q=x&facets=content_type%2Caccess",
            "q=x&facets=content_type&facets=&facets=access",
        ] {
            let (status, body) = rest("GET", "/v1/search", query, None).await;
            assert_eq!(status, 200, "{query}: {body}");
            assert_eq!(body["facets"], "content_type,access", "{query}");
        }
        let (status, body) = rest("GET", "/v1/search", "q=x&facets=access", None).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["facets"], "access");

        let (status, body) = rest("GET", "/v1/search", "q=x&facets=access&facets=size", None).await;
        assert_eq!(status, 400, "{body}");
        let refusal = body["error"].as_str().unwrap();
        assert!(refusal.contains("any of content_type, access"), "{refusal}");
    }

    /// A JSON body carries the array the OpenAPI document describes, or
    /// the comma-separated string callers have always sent.
    #[tokio::test]
    async fn a_rest_body_takes_an_array_or_one_comma_separated_string() {
        for labels in [
            json!(["red", "blue"]),
            json!("red,blue"),
            json!(["red,blue"]),
        ] {
            let body = Some(json!({ "labels": labels }));
            let (status, answer) = rest("POST", "/v1/files/01A/tag", "", body).await;
            assert_eq!(status, 200, "{labels}: {answer}");
            assert_eq!(answer["labels"], "red,blue", "{labels}");
        }
        for labels in [json!(["red", "pink"]), json!(["red", 3]), json!(7)] {
            let body = Some(json!({ "labels": labels }));
            let (status, answer) = rest("POST", "/v1/files/01A/tag", "", body).await;
            assert_eq!(status, 400, "{labels}: {answer}");
        }
    }

    /// A host serving `tools/call` hands the arguments to the
    /// dispatcher as they arrived, with the instance id lifted out for
    /// an action.
    async fn call_tool(tool: &str, arguments: Value) -> Result<Value, KayakError> {
        let mut input = arguments.as_object().unwrap().clone();
        let dispatcher = dispatcher();
        let ctx = KayakContext::new();
        match tool {
            "search" => dispatcher.query("search", ctx, QueryArgs { input }).await,
            "file_tag" => {
                let id = input
                    .remove("id")
                    .and_then(|id| id.as_str().map(str::to_owned));
                let args = ActionArgs { id, input };
                let answer = dispatcher.action("files", "tag", ctx, args).await?;
                Ok(answer.unwrap_or_default())
            }
            other => panic!("no tool {other}"),
        }
    }

    /// An MCP call carries the array the manifest publishes, or the
    /// comma-separated string an agent written against the old form
    /// sends. Before, the array was refused as the wrong type.
    #[tokio::test]
    async fn an_mcp_call_takes_an_array_or_one_comma_separated_string() {
        for facets in [
            json!(["content_type", "access"]),
            json!("content_type,access"),
        ] {
            let answer = call_tool("search", json!({ "q": "x", "facets": facets }))
                .await
                .unwrap();
            assert_eq!(answer["facets"], "content_type,access", "{facets}");
        }
        let answer = call_tool("file_tag", json!({ "id": "01A", "labels": ["green"] }))
            .await
            .unwrap();
        assert_eq!(answer["labels"], "green");

        let refused = call_tool("search", json!({ "q": "x", "facets": ["size"] }))
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("any of content_type, access"),
            "{refused}",
        );
    }

    /// The GraphQL schema declares the input a `String`, so the
    /// comma-separated form is the one it carries.
    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn graphql_takes_one_comma_separated_string() {
        let schema = kayak::runtime::graphql::build_schema(&tables(), dispatcher()).unwrap();
        let run = |document: &'static str| {
            let schema = schema.clone();
            async move {
                let request = async_graphql::Request::new(document).data(KayakContext::new());
                let response = schema.execute(request).await;
                assert!(response.errors.is_empty(), "{:?}", response.errors);
                response.data.into_json().unwrap()
            }
        };
        let data = run(r#"{ search(q: "x", facets: "content_type,access") }"#).await;
        assert_eq!(data["search"]["facets"], "content_type,access");
        let data = run(r#"mutation { fileTag(id: "01A", labels: "red,blue") }"#).await;
        assert_eq!(data["fileTag"]["labels"], "red,blue");
    }

    /// A checked box submits its own pair, so a console form repeats
    /// the key once per value chosen. Before, only the last box
    /// reached the resolver.
    #[cfg(feature = "console")]
    #[tokio::test]
    async fn a_console_form_keeps_every_checked_box() {
        use kayak::runtime::{ConsoleConfig, ConsoleRouter, FormOutcome};

        let console = ConsoleRouter::new(
            dispatcher(),
            ConsoleConfig {
                base: "/console".to_owned(),
                title: "test".to_owned(),
            },
        );
        let pairs = |values: &[&str]| -> Vec<(String, String)> {
            values
                .iter()
                .map(|value| ("labels".to_owned(), (*value).to_owned()))
                .collect()
        };
        for form in [pairs(&["red", "blue"]), pairs(&["red,blue"])] {
            match console
                .submit("/r/files/01A/a/tag", &form, KayakContext::new())
                .await
            {
                FormOutcome::Page(page) => {
                    assert_eq!(page.status, 200, "{}", page.html);
                    assert!(page.html.contains("red,blue"), "{form:?}: {}", page.html);
                }
                FormOutcome::Redirect(target) => panic!("expected the answer, got {target}"),
            }
        }

        let page = console
            .page(
                "/q/search",
                "q=x&facets=content_type&facets=access",
                KayakContext::new(),
            )
            .await;
        assert_eq!(page.status, 200, "{}", page.html);
        assert!(page.html.contains("content_type,access"), "{}", page.html);
    }
}

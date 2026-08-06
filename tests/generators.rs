//! Every generator over one action-bearing contract, golden-tested.
//!
//! `JANUS_BLESS=1 cargo test` re-blesses all goldens deliberately;
//! anything else that changes an artifact is drift and fails.

use janus::clients::{
    generate_client_go, generate_client_py, generate_client_rs, generate_client_ts,
};
use janus::diff::{diff, Change};
use janus::generate::{generate_all, TARGETS};
use janus::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, SubResource,
    TypeRef,
};
use surql::schema::{
    datetime_field, index, int_field, object_field, string_field, table_schema, unique_index,
    TableDefinition, TableMode,
};

fn file_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("file")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("path")),
            built(string_field("state")),
            built(string_field("content_type")),
            built(int_field("size_bytes").nullable(true)),
            built(string_field("digest").nullable(true)),
            built(object_field("metadata")),
            built(datetime_field("created_at")),
        ])
        .with_indexes([
            unique_index("uniq_live_path", ["tenant_id", "path", "live_marker"]),
            index("idx_listing", ["tenant_id", "state", "created_at"]),
        ])
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
                FieldExposure::column("content_type"),
                FieldExposure::renamed("size_bytes", "size"),
                FieldExposure::column("digest"),
                FieldExposure::column("metadata"),
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
                    input: vec![
                        ActionField {
                            name: "ttl_secs".into(),
                            kind: TypeRef::Int,
                            required: false,
                            description: None,
                            options: Vec::new(),
                        },
                        ActionField {
                            name: "max_uses".into(),
                            kind: TypeRef::Int,
                            required: false,
                            description: None,
                            options: Vec::new(),
                        },
                    ],
                    output: ActionOutput::Json,
                    description: Some("Issue a signed URL for a servable file.".into()),
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
                    description: Some("Soft-delete the file.".into()),
                    graphql_field: None,
                    requires: vec![],
                    rate_class: None,
                },
            ],
            content: None,
            filter_options: Default::default(),
        }],
        // One query with a path parameter and one without, so the
        // goldens carry both shapes and the CLI test's real Python and
        // Go toolchains parse what the generators emit for them.
        queries: vec![
            Query {
                name: "search".into(),
                path: "/v1/search".into(),
                input: vec![
                    ActionField {
                        name: "q".into(),
                        kind: TypeRef::String,
                        required: true,
                        description: Some("What to look for.".into()),
                        options: Vec::new(),
                    },
                    ActionField {
                        name: "limit".into(),
                        kind: TypeRef::Int,
                        required: false,
                        description: None,
                        options: Vec::new(),
                    },
                ],
                description: Some("Retrieval across the tenant's text.".into()),
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: None,
            },
            Query {
                name: "file_text".into(),
                path: "/v1/files/{id}/text".into(),
                input: vec![ActionField {
                    name: "id".into(),
                    kind: TypeRef::String,
                    required: true,
                    description: None,
                    options: Vec::new(),
                }],
                description: None,
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: None,
            },
        ],
    }
}

#[test]
fn all_targets_generate_and_match_goldens() {
    let artifacts = generate_all(&contract(), &[file_table()], TARGETS).unwrap();
    assert_eq!(artifacts.len(), TARGETS.len());
    for (filename, content) in &artifacts {
        let golden_path = format!(
            "{}/tests/golden/full_{}",
            env!("CARGO_MANIFEST_DIR"),
            filename,
        );
        if std::env::var("JANUS_BLESS").is_ok() {
            std::fs::write(&golden_path, content).unwrap();
        }
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|_| panic!("{golden_path} missing; JANUS_BLESS=1 to create"));
        assert_eq!(
            content.trim(),
            golden.trim(),
            "{filename} drifted from its golden; JANUS_BLESS=1 to re-bless deliberately",
        );
    }
}

#[test]
fn sdl_carries_types_sorts_and_mutations() {
    let sdl = janus::generate_sdl(&contract(), &[file_table()]).unwrap();
    assert!(sdl.contains("type File {"), "{sdl}");
    assert!(
        sdl.contains("size: Int\n"),
        "nullable drops the bang: {sdl}"
    );
    assert!(sdl.contains("path: String!"), "{sdl}");
    assert!(sdl.contains("metadata: JSON!"), "{sdl}");
    assert!(sdl.contains("created_at: DateTime!"), "{sdl}");
    assert!(sdl.contains("enum FileSort {"), "{sdl}");
    assert!(sdl.contains("CREATED_AT_DESC"), "{sdl}");
    assert!(
        sdl.contains(
            "files(limit: Int = 100, cursor: String, state: String, sort: FileSort): FilePage!"
        ),
        "{sdl}",
    );
    assert!(sdl.contains("file(id: ID!): File"), "{sdl}");
    assert!(
        sdl.contains("fileIssueUrl(id: ID!, ttlSecs: Int, maxUses: Int): JSON!"),
        "{sdl}",
    );
    assert!(sdl.contains("fileRemove(id: ID!): Boolean!"), "{sdl}");
    assert!(sdl.starts_with("scalar DateTime\nscalar JSON\n"), "{sdl}");
}

#[test]
fn openapi_carries_action_paths() {
    let doc = janus::generate_openapi(&contract(), &[file_table()]).unwrap();
    let issue = &doc["paths"]["/v1/files/{id}/url"]["post"];
    assert_eq!(issue["operationId"], "issue_url_files");
    assert_eq!(
        issue["requestBody"]["content"]["application/json"]["schema"]["properties"]["ttl_secs"]
            ["type"],
        "integer",
    );
    let list = &doc["paths"]["/v1/files"]["get"];
    assert_eq!(
        list["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/FilePage",
        "list responses are the page envelope, matching the SDL and clients",
    );
    assert!(doc["components"]["schemas"]["FilePage"]["properties"]["next_cursor"].is_object());
    let remove = &doc["paths"]["/v1/files/{id}"]["delete"];
    assert_eq!(remove["responses"]["204"]["description"], "No content.");
    // The plain get on the same path coexists with the delete action.
    assert!(doc["paths"]["/v1/files/{id}"]["get"].is_object());
}

#[test]
fn clients_carry_types_and_action_methods() {
    let artifacts = generate_all(&contract(), &[file_table()], TARGETS).unwrap();
    let rust = &artifacts["client.rs"];
    assert!(rust.contains("pub struct File {"), "{rust}");
    assert!(rust.contains("pub size: Option<i64>,"), "{rust}");
    assert!(rust.contains("pub async fn list_files"), "{rust}");
    assert!(rust.contains("pub async fn issue_url_file"), "{rust}");
    assert!(rust.contains("pub async fn remove_file"), "{rust}");

    let ts = &artifacts["client.ts"];
    assert!(ts.contains("export interface File {"), "{ts}");
    assert!(ts.contains("size?: number"), "{ts}");
    assert!(
        ts.contains("issueUrlFile(id: string, input: Record<string, unknown>)"),
        "{ts}"
    );
    assert!(ts.contains("removeFile(id: string): Promise<void>"), "{ts}");

    let python = &artifacts["client.py"];
    assert!(python.contains("class File:"), "{python}");
    assert!(python.contains("size: int | None = None"), "{python}");
    assert!(
        python.contains("def issue_url_file(self, id: str"),
        "{python}"
    );
    assert!(python.contains("def list_files(self"), "{python}");

    let go = &artifacts["client.go"];
    assert!(go.contains("type File struct {"), "{go}");
    assert!(go.contains("Size *int64 `json:\"size\"`"), "{go}");
    assert!(
        go.contains("func (c *Client) IssueUrlFile(id string, input map[string]any)"),
        "{go}"
    );
    assert!(
        go.contains("func (c *Client) RemoveFile(id string) error"),
        "{go}"
    );
}

#[test]
fn unknown_target_is_refused() {
    let error = generate_all(&contract(), &[file_table()], &["client-cobol"]).unwrap_err();
    assert!(error.to_string().contains("client-cobol"), "{error}");
}

#[test]
fn action_validation_fires() {
    let mut bad = contract();
    bad.resources[0].actions.push(Action {
        name: "issue_url".into(), // duplicate
        method: "FETCH".into(),   // bad verb
        path: "no-slash".into(),  // bad path
        input: vec![],
        output: ActionOutput::Json,
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    });
    let violations = janus::validate(&bad, &[file_table()]);
    let text = violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("duplicate action name"), "{text}");
    assert!(text.contains("FETCH"), "{text}");
    assert!(text.contains("must start with '/'"), "{text}");
}

#[test]
fn differ_classifies_changes() {
    let old = contract();

    // Compatible: add a field, an optional input, a new action.
    let mut compatible = contract();
    compatible.resources[0]
        .fields
        .push(FieldExposure::column("tenant_id"));
    compatible.resources[0].actions[0].input.push(ActionField {
        name: "note".into(),
        kind: TypeRef::String,
        required: false,
        description: None,
        options: Vec::new(),
    });
    let changes = diff(&old, &compatible);
    assert!(!changes.is_empty());
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");

    // Breaking: remove a field, remove a sort, move an action, add a
    // required input, lower the page size.
    let mut breaking = contract();
    breaking.resources[0]
        .fields
        .retain(|f| f.api_name() != "size");
    breaking.resources[0].sortable.clear();
    breaking.resources[0].actions[0].method = "PUT".into();
    breaking.resources[0].actions[1].input.push(ActionField {
        name: "reason".into(),
        kind: TypeRef::String,
        required: true,
        description: None,
        options: Vec::new(),
    });
    breaking.resources[0].max_page_size = 10;
    let changes = diff(&old, &breaking);
    let breaking_messages: Vec<&str> = changes
        .iter()
        .filter(|c| c.is_breaking())
        .map(Change::message)
        .collect();
    assert_eq!(breaking_messages.len(), 5, "{breaking_messages:?}");
    let joined = breaking_messages.join("\n");
    assert!(joined.contains("field size removed"), "{joined}");
    assert!(joined.contains("sort created_at removed"), "{joined}");
    assert!(joined.contains("action issue_url moved"), "{joined}");
    assert!(joined.contains("gained required input reason"), "{joined}");
    assert!(joined.contains("max_page_size lowered"), "{joined}");

    // Opening a subscription is additive; closing one strands every
    // deployed subscriber, and so does renaming its field.
    let mut opened = contract();
    opened.resources[0].watchable = true;
    let changes = diff(&old, &opened);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");
    assert!(
        changes
            .iter()
            .any(|c| c.message().contains("became watchable")),
        "{changes:?}",
    );

    let mut closed = contract();
    closed.resources[0].watchable = true;
    let changes = diff(&opened, &closed);
    assert!(changes.is_empty(), "{changes:?}");
    let changes = diff(&opened, &old);
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("no longer watchable")),
        "{changes:?}",
    );

    let mut renamed_watch = opened.clone();
    renamed_watch.resources[0].graphql = Some(janus::GraphqlNames {
        watch_field: Some("fileTouched".into()),
        ..Default::default()
    });
    let changes = diff(&opened, &renamed_watch);
    assert!(
        changes.iter().any(|c| c.is_breaking()
            && c.message()
                .contains("graphql watch field renamed fileChanged -> fileTouched")),
        "{changes:?}",
    );

    // Same wire name over a different column is breaking.
    let mut retargeted = contract();
    retargeted.resources[0].fields[0] = FieldExposure::renamed("digest", "path");
    let changes = diff(&old, &retargeted);
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("now reads column")),
        "{changes:?}",
    );

    // Removing a resource is breaking; adding one is not.
    let empty = Contract {
        resources: vec![],
        ..contract()
    };
    let changes = diff(&old, &empty);
    assert!(changes.iter().any(|c| c.is_breaking()));
    let changes = diff(&empty, &old);
    assert!(changes.iter().all(|c| !c.is_breaking()));
}

#[test]
fn limits_are_visible_and_their_tightening_is_breaking() {
    let open = contract();
    let mut capped = contract();
    capped.limits = Some(janus::ContractLimits {
        max_depth: Some(10),
        max_complexity: Some(500),
        max_watches_per_principal: None,
    });

    // The document carries what the served schema will enforce.
    let doc = janus::generate_openapi(&capped, &[file_table()]).unwrap();
    assert_eq!(doc["x-limits"]["max_depth"], 10);
    assert_eq!(doc["x-limits"]["max_complexity"], 500);
    let bare = janus::generate_openapi(&open, &[file_table()]).unwrap();
    assert!(bare.get("x-limits").is_none(), "no ceilings, no extension");

    // Introducing a ceiling refuses operations that used to run.
    let changes = diff(&open, &capped);
    assert!(
        changes
            .iter()
            .filter(|c| c.is_breaking())
            .any(|c| c.message().contains("max_depth introduced at 10")),
        "{changes:?}",
    );

    // Lowering is breaking; raising is compatible; removing is
    // compatible.
    let mut lowered = capped.clone();
    lowered.limits = Some(janus::ContractLimits {
        max_depth: Some(8),
        max_complexity: Some(500),
        max_watches_per_principal: None,
    });
    let changes = diff(&capped, &lowered);
    assert!(
        changes
            .iter()
            .any(|c| c.is_breaking() && c.message().contains("max_depth lowered 10 -> 8")),
        "{changes:?}",
    );
    let changes = diff(&lowered, &capped);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");
    let changes = diff(&capped, &open);
    assert!(changes.iter().all(|c| !c.is_breaking()), "{changes:?}");
}

/// Content faces render into the OpenAPI document and answer to the
/// differ: bytes join the artifact, and removing a face breaks.
#[test]
fn content_faces_are_documented_and_governed() {
    let mut with_content = contract();
    with_content.resources[0].content = Some(janus::ContentFaces {
        upload: true,
        download: true,
    });
    let document = janus::generate_openapi(&with_content, &[file_table()]).unwrap();
    let content = &document["paths"]["/v1/files/{id}/content"];
    assert!(content["put"]["requestBody"]["content"]["application/octet-stream"].is_object());
    assert!(content["get"]["responses"]["200"]["content"]["application/octet-stream"].is_object());

    let mut without = with_content.clone();
    without.resources[0].content = Some(janus::ContentFaces {
        upload: false,
        download: true,
    });
    let changes = diff(&with_content, &without);
    assert!(
        changes.iter().any(|c| c.is_breaking()),
        "removing the upload face must break: {changes:?}",
    );
    let changes = diff(&without, &with_content);
    assert!(
        changes.iter().all(|c| !c.is_breaking()),
        "adding a face is compatible: {changes:?}",
    );
}

#[test]
fn python_client_indentation_survives_sub_resources() {
    // The sub-resource page template once carried its Rust source
    // indentation into the generated Python, which is an
    // IndentationError there. Generate from a contract that has a
    // sub-resource and hold every line to the client's real indent
    // budget.
    let mut contract = contract();
    contract.resources[0].sub_resources.push(SubResource {
        name: "revisions".into(),
        table: "doc_revision".into(),
        parent_key: "doc".into(),
        fields: vec![
            FieldExposure::column("number"),
            FieldExposure::column("created_at"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec![],
        sortable: vec![],
        max_page_size: 100,
        description: None,
        graphql: None,
    });
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    let revision_table = table_schema("doc_revision")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("doc")),
            built(int_field("number")),
            built(datetime_field("created_at")),
        ])
        .with_indexes([index("idx_revisions", ["tenant_id", "doc", "created_at"])]);
    let python = janus::clients::generate_client_py(&contract, &[file_table(), revision_table])
        .expect("python renders");
    assert!(
        python.contains("def list_revisions_"),
        "the sub-resource method renders",
    );
    for (index, line) in python.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        assert!(
            indent <= 8,
            "line {} over-indented ({indent}): {line:?}",
            index + 1,
        );
        assert!(
            !line.trim_start().contains("          "),
            "line {} carries an interior whitespace run: {line:?}",
            index + 1,
        );
    }
}

/// Queries reach the four clients.
///
/// They are the contract's declared reads that are not listings, and
/// the generators covered every other face first: OpenAPI, the SDL,
/// the MCP manifest, the console, and the runtime routers all read
/// `contract.queries`, while the clients did not. A caller holding a
/// generated SDK had no way to reach a search the contract declares.
#[test]
fn clients_carry_query_methods() {
    let mut contract = contract();
    contract.queries = vec![
        Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![
                ActionField {
                    name: "q".into(),
                    kind: TypeRef::String,
                    required: true,
                    description: None,
                    options: Vec::new(),
                },
                ActionField {
                    name: "limit".into(),
                    kind: TypeRef::Int,
                    required: false,
                    description: None,
                    options: Vec::new(),
                },
            ],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        },
        Query {
            name: "file_text".into(),
            path: "/v1/files/{id}/text".into(),
            input: vec![ActionField {
                name: "id".into(),
                kind: TypeRef::String,
                required: true,
                description: None,
                options: Vec::new(),
            }],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        },
    ];
    let schema = vec![file_table()];

    let rust = generate_client_rs(&contract, &schema).unwrap();
    assert!(
        rust.contains("pub async fn search(&self, q: &str, limit: Option<i64>)"),
        "{rust}",
    );
    // reqwest's own encoder, because a search term carries spaces and
    // ampersands and hand-joined pairs would ship them raw.
    assert!(rust.contains(".query(&[(\"q\", q)])"), "{rust}");
    assert!(
        rust.contains("pub async fn file_text(&self, id: &str)"),
        "the path parameter is a parameter, and not also a query value: {rust}",
    );

    let ts = generate_client_ts(&contract, &schema).unwrap();
    assert!(
        ts.contains("search(q: string, limit?: number): Promise<unknown>"),
        "{ts}"
    );
    assert!(ts.contains("fileText(id: string)"), "{ts}");
    // No parameters beyond the path one, so nothing builds a query
    // string that would always be empty.
    assert!(ts.contains("`/v1/files/${id}/text`"), "{ts}");
    assert!(!ts.contains("`/v1/files/${id}/text${suffix}`"), "{ts}");

    let py = generate_client_py(&contract, &schema).unwrap();
    assert!(
        py.contains("def search(self, q: str, limit: int | None = None) -> Any:"),
        "{py}",
    );
    assert!(py.contains("urllib.parse.urlencode(query)"), "{py}");
    assert!(py.contains("def file_text(self, id: str) -> Any:"), "{py}");

    let go = generate_client_go(&contract, &schema).unwrap();
    assert!(
        go.contains("func (c *Client) Search(q string, limit int64) (any, error)"),
        "{go}"
    );
    assert!(
        go.contains("func (c *Client) FileText(id string) (any, error)"),
        "{go}"
    );
    assert!(go.contains("\"/v1/files/\" + id + \"/text\""), "{go}");
}

/// A required parameter cannot follow an optional one in TypeScript or
/// Python, so the generators order them even when the contract does
/// not.
#[test]
fn required_query_parameters_lead() {
    let mut contract = contract();
    contract.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: vec![
            ActionField {
                name: "cursor".into(),
                kind: TypeRef::String,
                required: false,
                description: None,
                options: Vec::new(),
            },
            ActionField {
                name: "q".into(),
                kind: TypeRef::String,
                required: true,
                description: None,
                options: Vec::new(),
            },
        ],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    }];
    let schema = vec![file_table()];

    let ts = generate_client_ts(&contract, &schema).unwrap();
    assert!(
        ts.contains("search(q: string, cursor?: string)"),
        "an optional parameter ahead of a required one will not parse: {ts}",
    );
    let py = generate_client_py(&contract, &schema).unwrap();
    assert!(
        py.contains("def search(self, q: str, cursor: str | None = None)"),
        "{py}",
    );
}

/// A closed set reaches every face and is enforced on the wire.
///
/// A menu the server does not honour is worse than a text box: the
/// console offers three values, the OpenAPI document promises three,
/// and the wire quietly takes a fourth. So the declaration renders as
/// an `enum` in both documents and the dispatcher refuses anything
/// outside it, ahead of the resolver.
#[test]
fn a_closed_set_reaches_the_documents() {
    let mut contract = contract();
    contract.queries = vec![Query {
        name: "search".into(),
        path: "/v1/search".into(),
        input: vec![ActionField {
            name: "mode".into(),
            kind: TypeRef::String,
            required: false,
            description: None,
            options: vec!["lexical".into(), "semantic".into(), "hybrid".into()],
        }],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
    }];
    let schema = vec![file_table()];

    let openapi =
        serde_json::to_string(&janus::openapi::generate_openapi(&contract, &schema).unwrap())
            .unwrap();
    // Named against the parameter, since an unrelated `enum` elsewhere
    // in the document would otherwise satisfy this.
    assert!(
        openapi.contains(r#""enum":["lexical","semantic","hybrid"]"#),
        "the mode parameter carries the set: {openapi}",
    );

    let mcp = serde_json::to_string(&janus::mcp::generate_mcp_tools(&contract)).unwrap();
    assert!(
        mcp.contains("\"enum\""),
        "the manifest carries it too: {mcp}"
    );
    assert!(mcp.contains("hybrid"), "{mcp}");
}

/// Narrowing what a caller may send breaks them; widening does not.
#[test]
fn the_differ_reads_a_narrowing_set_as_breaking() {
    let open = |options: Vec<String>| {
        let mut contract = contract();
        contract.queries = vec![Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![ActionField {
                name: "mode".into(),
                kind: TypeRef::String,
                required: false,
                description: None,
                options,
            }],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        }];
        contract
    };

    // Anything, then only two: callers sending a third now fail.
    let changes = diff(&open(vec![]), &open(vec!["a".into(), "b".into()]));
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Breaking(m) if m.contains("takes only"))),
        "{changes:?}",
    );

    // Losing a value refuses callers who were sending it.
    let changes = diff(&open(vec!["a".into(), "b".into()]), &open(vec!["a".into()]));
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Breaking(m) if m.contains("no longer takes b"))),
        "{changes:?}",
    );

    // Gaining one accepts more than before, which breaks nobody.
    let changes = diff(&open(vec!["a".into()]), &open(vec!["a".into(), "b".into()]));
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Compatible(m) if m.contains("also takes b"))),
        "{changes:?}",
    );
    assert!(
        !changes.iter().any(|c| matches!(c, Change::Breaking(_))),
        "{changes:?}",
    );
}

/// Filter options describe a column a caller may narrow by, so naming
/// one that is not filterable offers a menu beside a refusal.
#[test]
fn filter_options_answer_to_the_filterable_list() {
    let mut contract = contract();
    contract.resources[0]
        .filter_options
        .insert("content_type".into(), vec!["text/plain".into()]);
    let violations = janus::validate(&contract, &[file_table()]);
    assert!(
        violations
            .iter()
            .any(|v| format!("{v}").contains("not filterable")),
        "{violations:?}",
    );

    // A column that is filterable passes.
    let mut ok = contract.clone();
    ok.resources[0].filter_options.clear();
    ok.resources[0]
        .filter_options
        .insert("state".into(), vec!["ready".into(), "failed".into()]);
    assert_eq!(janus::validate(&ok, &[file_table()]), vec![]);
}

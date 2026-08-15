//! Every generator over one action-bearing contract, golden-tested.
//!
//! `JANUS_BLESS=client-go cargo test` re-blesses one golden, a comma
//! separated list re-blesses several, and `JANUS_BLESS=1` re-blesses
//! all of them. Anything else that changes an artifact is drift and
//! fails. See `tests/common` for why a bless names its target.

mod common;

use janus::clients::{
    generate_client_go, generate_client_py, generate_client_rs, generate_client_rs_blocking,
    generate_client_ts,
};
use janus::diff::{diff, Change};
use janus::generate::{generate_all, TARGETS};
use janus::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, SearchBacking,
    SearchKind, SubResource, TypeRef,
};
use surql::schema::{
    array_field, bm25_index, datetime_field, hnsw_index, index, int_field, object_field,
    string_field, table_schema, unique_index, HnswDistanceType, MTreeVectorType, TableDefinition,
    TableMode,
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

/// Copal's `text_chunk` shape: the table the fixture's search query
/// declares its backing against, carrying the two search indexes.
fn chunk_table() -> TableDefinition {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    table_schema("text_chunk")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("tenant_id")),
            built(string_field("body")),
            built(array_field("embedding").nullable(true)),
        ])
        .with_indexes([
            bm25_index("idx_chunk_body", ["body"], "copal_text"),
            hnsw_index(
                "idx_chunk_embedding",
                "embedding",
                768,
                HnswDistanceType::Cosine,
                MTreeVectorType::F32,
                None,
                None,
            ),
        ])
}

fn schema() -> Vec<TableDefinition> {
    vec![file_table(), chunk_table()]
}

fn contract() -> Contract {
    Contract {
        name: "copal".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        // Copal's convention, now declared rather than hardcoded in four
        // generators. Stating it here must reproduce the goldens byte for
        // byte -- that equality is the proof the mechanism is faithful to
        // the behaviour it replaced.
        auth: janus::AuthScheme::Header {
            name: "x-copal-tenant".into(),
            credential: "tenant".into(),
        },
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
                            multiple: false,
                            description: None,
                            options: Vec::new(),
                        },
                        ActionField {
                            name: "max_uses".into(),
                            kind: TypeRef::Int,
                            required: false,
                            multiple: false,
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
            faces: Default::default(),
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
                        multiple: false,
                        description: Some("What to look for.".into()),
                        options: Vec::new(),
                    },
                    ActionField {
                        name: "limit".into(),
                        kind: TypeRef::Int,
                        required: false,
                        multiple: false,
                        description: None,
                        options: Vec::new(),
                    },
                ],
                description: Some("Retrieval across the tenant's text.".into()),
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: None,
                // Copal's fused search, declared: BM25 candidates and
                // HNSW neighbours over the same passages, so the
                // goldens carry a backed query and prove the backing
                // is capacity metadata rather than wire shape. The
                // vector half is optional and pinned to a width, so
                // the goldens carry both riders too.
                searches: vec![SearchKind::Lexical, SearchKind::Vector],
                backing: vec![
                    SearchBacking {
                        table: "text_chunk".into(),
                        column: "body".into(),
                        index: "idx_chunk_body".into(),
                        kind: SearchKind::Lexical,
                        dimension: None,
                        optional: false,
                    },
                    SearchBacking {
                        table: "text_chunk".into(),
                        column: "embedding".into(),
                        index: "idx_chunk_embedding".into(),
                        kind: SearchKind::Vector,
                        dimension: Some(768),
                        optional: true,
                    },
                ],
            },
            Query {
                name: "file_text".into(),
                path: "/v1/files/{id}/text".into(),
                input: vec![ActionField {
                    name: "id".into(),
                    kind: TypeRef::String,
                    required: true,
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                }],
                description: None,
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: None,
                searches: vec![],
                backing: vec![],
            },
        ],
    }
}

#[test]
fn all_targets_generate_and_match_goldens() {
    let artifacts = generate_all(&contract(), &schema(), TARGETS).unwrap();
    assert_eq!(artifacts.len(), TARGETS.len());
    for (filename, content) in &artifacts {
        common::check_golden(filename, content);
    }
}

#[test]
fn the_blocking_client_matches_its_golden() {
    // Opt-in, so it is not in TARGETS and the loop above never sees it.
    let artifacts = generate_all(&contract(), &schema(), &["client-rs-blocking"]).unwrap();
    let content = &artifacts["client_blocking.rs"];
    common::check_golden("client_blocking.rs", content);
}

#[test]
fn sdl_carries_types_sorts_and_mutations() {
    let sdl = janus::generate_sdl(&contract(), &schema()).unwrap();
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
    let doc = janus::generate_openapi(&contract(), &schema()).unwrap();
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
    let artifacts = generate_all(&contract(), &schema(), TARGETS).unwrap();
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
    let error = generate_all(&contract(), &schema(), &["client-cobol"]).unwrap_err();
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
    let violations = janus::validate(&bad, &schema());
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
        multiple: false,
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
        multiple: false,
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
    let doc = janus::generate_openapi(&capped, &schema()).unwrap();
    assert_eq!(doc["x-limits"]["max_depth"], 10);
    assert_eq!(doc["x-limits"]["max_complexity"], 500);
    let bare = janus::generate_openapi(&open, &schema()).unwrap();
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
    let document = janus::generate_openapi(&with_content, &schema()).unwrap();
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
    let python = janus::clients::generate_client_py(
        &contract,
        &[file_table(), chunk_table(), revision_table],
    )
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
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                },
                ActionField {
                    name: "limit".into(),
                    kind: TypeRef::Int,
                    required: false,
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                },
            ],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
            searches: vec![],
            backing: vec![],
        },
        Query {
            name: "file_text".into(),
            path: "/v1/files/{id}/text".into(),
            input: vec![ActionField {
                name: "id".into(),
                kind: TypeRef::String,
                required: true,
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
        },
    ];
    let schema = schema();

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
                multiple: false,
                description: None,
                options: Vec::new(),
            },
            ActionField {
                name: "q".into(),
                kind: TypeRef::String,
                required: true,
                multiple: false,
                description: None,
                options: Vec::new(),
            },
        ],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![],
        backing: vec![],
    }];
    let schema = schema();

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
/// A menu the server does not honor is worse than a text box: the
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
            multiple: false,
            description: None,
            options: vec!["lexical".into(), "semantic".into(), "hybrid".into()],
        }],
        description: None,
        graphql_field: None,
        requires: vec![],
        rate_class: None,
        searches: vec![],
        backing: vec![],
    }];
    let schema = schema();

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
                multiple: false,
                description: None,
                options,
            }],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
            searches: vec![],
            backing: vec![],
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

/// A backing is capacity metadata, not wire shape.
///
/// Stripping the fixture's backings must move exactly two artifacts:
/// the OpenAPI document, where the operation description states the
/// machinery, and the MCP manifest, where it rides the annotations
/// beside scope and rate. The SDL and all four clients are the wire a
/// caller holds, and they come out byte-identical, which is the whole
/// design: the contract promises more without the API saying anything
/// different.
///
/// The declared searches go with them, and for a second reason as
/// well as the first: leaving a declaration behind with nothing under
/// it does not generate at all, which is the gate and is asserted
/// next door.
#[test]
fn a_backing_changes_no_wire_surface() {
    let backed = generate_all(&contract(), &schema(), TARGETS).unwrap();
    let mut stripped_contract = contract();
    for query in &mut stripped_contract.queries {
        query.backing.clear();
        query.searches.clear();
    }
    let stripped = generate_all(&stripped_contract, &schema(), TARGETS).unwrap();
    for (filename, content) in &backed {
        let bare = &stripped[filename];
        if matches!(filename.as_str(), "openapi.json" | "mcp-tools.json") {
            assert_ne!(content, bare, "{filename} should surface the backing");
        } else {
            assert_eq!(content, bare, "{filename} must not move for a backing");
        }
    }

    // And what the metadata faces say, exactly.
    let doc = janus::generate_openapi(&contract(), &schema()).unwrap();
    assert_eq!(
        doc["paths"]["/v1/search"]["get"]["description"],
        "Search backing: lexical via idx_chunk_body over text_chunk.body; \
         vector via idx_chunk_embedding over text_chunk.embedding where configured.",
    );
    let mcp = janus::generate_mcp_tools(&contract());
    let search = mcp["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search")
        .unwrap();
    assert_eq!(
        search["annotations"]["backing"][0]["index"],
        "idx_chunk_body"
    );
    assert_eq!(search["annotations"]["backing"][1]["kind"], "vector");
    // The width and the hedge ride along, which is the half an agent
    // reading before it calls actually needs: what this deployment
    // may not have.
    assert_eq!(search["annotations"]["backing"][1]["dimension"], 768);
    assert_eq!(search["annotations"]["backing"][1]["optional"], true);
    // And they stay off the backing that has neither.
    assert!(search["annotations"]["backing"][0]["dimension"].is_null());
    assert!(search["annotations"]["backing"][0]["optional"].is_null());
}

/// A declared search with nothing behind it does not generate.
///
/// This is the gate, stated where the artifacts are made: a contract
/// that promises semantic search over a column no vector index covers
/// produces no OpenAPI, no SDL, no clients — it produces the refusal
/// naming the query and the kind. Every other rule in the validator
/// reads something the author wrote and holds it to the schema; this
/// one reads what the author did NOT write, which is the shape an
/// unindexed neighbour search actually has when it ships.
#[test]
fn a_declared_search_with_no_backing_refuses_to_generate() {
    let mut promised = contract();
    promised.queries[0].backing.clear();
    let error = generate_all(&promised, &schema(), TARGETS)
        .expect_err("a promise with nothing behind it is not generated");
    let text = format!("{error}");
    assert!(text.contains("performs a lexical search"), "{text}");
    assert!(text.contains("performs a vector search"), "{text}");

    // And the repair is to declare the machinery, not to weaken the
    // rule: the same contract with its backings back generates.
    generate_all(&contract(), &schema(), TARGETS).expect("the backed contract generates");
}

/// Filter options describe a column a caller may narrow by, so naming
/// one that is not filterable offers a menu beside a refusal.
#[test]
fn filter_options_answer_to_the_filterable_list() {
    let mut contract = contract();
    contract.resources[0]
        .filter_options
        .insert("content_type".into(), vec!["text/plain".into()]);
    let violations = janus::validate(&contract, &schema());
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
    assert_eq!(janus::validate(&ok, &schema()), vec![]);
}

/// The credential a client sends is the one the contract declared.
///
/// Until `Contract.auth` existed, `x-copal-tenant` was hardcoded in
/// eight places across the four generators, so janus produced clients for
/// copal rather than for contracts: a service authenticating with a bearer
/// token got one that sent somebody else's header and no credential at all.
/// The fixture declares copal's header explicitly now, and the goldens did
/// not move by a byte -- so this checks the other two schemes instead.
#[test]
fn the_client_sends_the_credential_the_contract_declares() {
    let rust = |auth: janus::AuthScheme| {
        let mut c = contract();
        c.auth = auth;
        generate_client_rs(&c, &schema()).expect("generates")
    };

    // Bearer: the constructor takes a token and every request carries it
    // in the standard header, with the scheme prefix.
    let bearer = rust(janus::AuthScheme::Bearer);
    assert!(
        bearer.contains("pub fn new(base_url: impl Into<String>, token: impl Into<String>)"),
        "{bearer}"
    );
    assert!(
        bearer.contains(r#".header("authorization", format!("Bearer {}", self.token))"#),
        "{bearer}"
    );
    assert!(!bearer.contains("x-copal-tenant"), "no stale header");
    assert!(!bearer.contains("tenant"), "no stale credential name");

    // None: no credential to carry, so no field, no parameter, no header.
    let open = rust(janus::AuthScheme::None);
    assert!(
        open.contains("pub fn new(base_url: impl Into<String>) -> Self"),
        "{open}"
    );
    assert!(
        !open.contains(".header("),
        "an open API sets no auth header"
    );
    assert!(!open.contains("token"), "{open}");

    // A header scheme names the credential after the service's own word
    // for it, not after the header.
    let keyed = rust(janus::AuthScheme::Header {
        name: "x-api-key".into(),
        credential: "api_key".into(),
    });
    assert!(
        keyed.contains("pub fn new(base_url: impl Into<String>, api_key: impl Into<String>)"),
        "{keyed}"
    );
    assert!(
        keyed.contains(r#".header("x-api-key", &self.api_key)"#),
        "{keyed}"
    );

    // Every request in the file is authenticated, not just the first --
    // the whole reason the expression is built once and spliced.
    let requests = bearer.matches("self.http.").count();
    let headers = bearer.matches(r#".header("authorization""#).count();
    assert_eq!(requests, headers, "every request carries the credential");
    // The other three languages carry the same declaration, each in its
    // own idiom. Checked together because the bug being prevented was one
    // generator drifting from the rest.
    let bearer_of =
        |gen: fn(&Contract, &[TableDefinition]) -> Result<String, janus::GenerateError>| {
            let mut c = contract();
            c.auth = janus::AuthScheme::Bearer;
            gen(&c, &schema()).expect("generates")
        };
    let ts = bearer_of(generate_client_ts);
    assert!(
        ts.contains("constructor(private baseUrl: string, private token: string)"),
        "{ts}"
    );
    assert!(
        ts.contains("'authorization': `Bearer ${this.token}`"),
        "{ts}"
    );
    assert!(!ts.contains("x-copal-tenant"), "{ts}");

    let py = bearer_of(generate_client_py);
    assert!(
        py.contains("def __init__(self, base_url: str, token: str)"),
        "{py}"
    );
    assert!(
        py.contains("headers = {'authorization': f'Bearer {self.token}'}"),
        "{py}"
    );
    assert!(!py.contains("x-copal-tenant"), "{py}");

    let go = bearer_of(generate_client_go);
    assert!(
        go.contains("func NewClient(baseURL, token string) *Client"),
        "{go}"
    );
    assert!(
        go.contains(r#"request.Header.Set("authorization", "Bearer "+c.Token)"#),
        "{go}"
    );
    assert!(!go.contains("x-copal-tenant"), "{go}");

    // Go aligns its struct types to the widest field name, and the
    // credential can now BE the widest.
    let wide = {
        let mut c = contract();
        c.auth = janus::AuthScheme::Header {
            name: "x-api-key".into(),
            credential: "authorization_key".into(),
        };
        generate_client_go(&c, &schema()).expect("generates")
    };
    // Assert the property rather than a hand-counted string: every type
    // in the struct starts at the same column, which is what gofmt would
    // produce and what a hardcoded pad width would get wrong the moment a
    // credential name outgrew `BaseURL`.
    let struct_body = wide
        .split("type Client struct {\n")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("the client struct");
    let type_starts: Vec<usize> = struct_body
        .lines()
        .map(|line| {
            let field = line.trim_start_matches('\t');
            let name = field.split_whitespace().next().expect("a field name");
            field[name.len()..].len() - field[name.len()..].trim_start().len() + name.len()
        })
        .collect();
    assert!(
        type_starts.windows(2).all(|w| w[0] == w[1]),
        "gofmt aligns the type column: {struct_body:?} -> {type_starts:?}",
    );
    assert!(struct_body.contains("AuthorizationKey"), "{struct_body}");
}

/// The OpenAPI document says how to authenticate.
///
/// It described every path and never mentioned a credential, so a reader
/// had to infer one from an example — and the four generated clients each
/// hardcoded their own answer. Both now read the same declaration, which
/// is what keeps the document and the SDKs from disagreeing.
#[test]
fn the_openapi_document_declares_the_security_scheme() {
    let doc = |auth: janus::AuthScheme| {
        let mut c = contract();
        c.auth = auth;
        janus::generate_openapi(&c, &schema()).expect("generates")
    };

    let bearer = doc(janus::AuthScheme::Bearer);
    assert_eq!(
        bearer["components"]["securitySchemes"]["bearer"],
        serde_json::json!({ "type": "http", "scheme": "bearer" }),
    );
    assert_eq!(
        bearer["security"],
        serde_json::json!([{ "bearer": [] }]),
        "and it is required of the whole API, not merely defined",
    );

    let keyed = doc(janus::AuthScheme::Header {
        name: "x-api-key".into(),
        credential: "api_key".into(),
    });
    assert_eq!(
        keyed["components"]["securitySchemes"]["header"],
        serde_json::json!({ "type": "apiKey", "in": "header", "name": "x-api-key" }),
    );

    // An open API says nothing rather than declaring an empty scheme,
    // which a reader would have to interpret.
    let open = doc(janus::AuthScheme::None);
    assert!(open["components"]["securitySchemes"].is_null(), "{open}");
    assert!(open["security"].is_null(), "{open}");
}

#[test]
fn a_bless_names_the_artifact_it_means_to_rewrite() {
    use common::wants;

    assert!(
        !wants(None, "client-go"),
        "an unset variable blesses nothing"
    );
    assert!(wants(Some("client-go"), "client-go"));
    assert!(
        !wants(Some("client-go"), "client-py"),
        "and only that artifact -- this is the whole point",
    );

    assert!(wants(Some("client-go,openapi"), "openapi"), "a list");
    assert!(
        wants(Some(" client-go , openapi "), "client-go"),
        "loosely spaced"
    );

    for artifact in common::BLESSABLE {
        assert!(wants(Some("1"), artifact), "1 still blesses everything");
        assert!(wants(Some("all"), artifact), "and so does the spelled form");
    }
}

#[test]
#[should_panic(expected = "no such artifact")]
fn a_bless_that_names_nothing_real_fails_loudly() {
    // Go is `client-go` here and "golang" appears nowhere, so this is a
    // plausible thing to type. Blessing nothing quietly would read as a
    // clean run and leave you believing a golden had moved.
    let _ = common::wants(Some("golang"), "client-go");
}

#[test]
#[should_panic(expected = "no such artifact")]
fn a_typo_beside_a_real_name_still_fails() {
    let _ = common::wants(Some("client-go,typpo"), "client-go");
}

#[test]
fn every_target_can_be_blessed_by_name() {
    // Two readers that would otherwise drift: adding a target without
    // teaching the bless gate its name leaves an artifact that can only
    // be re-blessed by blessing all of them.
    for target in TARGETS {
        assert!(
            common::BLESSABLE.contains(target),
            "target {target} has no bless name; add it to tests/common",
        );
    }
    let artifacts = generate_all(&contract(), &schema(), TARGETS).unwrap();
    for filename in artifacts.keys() {
        // Panics if a generated filename has no artifact name.
        let name = common::artifact_of(filename);
        assert!(common::BLESSABLE.contains(&name));
    }
}

#[test]
fn the_blocking_client_never_suspends() {
    let client = generate_client_rs_blocking(&contract(), &schema()).unwrap();
    assert!(
        !client.contains(".await"),
        "a blocking client that awaits does not compile without a runtime",
    );
    assert!(!client.contains("async fn"), "nor does an async fn");
    assert!(
        client.contains("reqwest::blocking::Client"),
        "and it reaches for the blocking module",
    );
    assert!(
        client.contains("features = [\"json\", \"blocking\"]"),
        "which the header tells the caller to enable: {}",
        client.lines().take(4).collect::<Vec<_>>().join("\n"),
    );
}

#[test]
fn the_two_rust_flavours_describe_the_same_contract() {
    let asynchronous = generate_client_rs(&contract(), &schema()).unwrap();
    let blocking = generate_client_rs_blocking(&contract(), &schema()).unwrap();

    // Everything the client says ABOUT the contract has to survive the
    // flavour change: the types, the renames, the nullability, the auth
    // header, the URLs, the query shaping. Rather than spot-check those
    // one at a time, undo the four differences and demand the rest be
    // identical -- which also asserts there are only four.
    let converted = asynchronous
        .replace(
            "features = [\"json\"]",
            "features = [\"json\", \"blocking\"]",
        )
        .replace("reqwest::Client", "reqwest::blocking::Client")
        .replace("pub async fn", "pub fn")
        .replace(".await", "");
    assert_eq!(
        converted, blocking,
        "the flavours differ somewhere other than how a call suspends",
    );
}

#[test]
fn the_blocking_client_is_not_in_a_default_run() {
    assert!(
        !TARGETS.contains(&"client-rs-blocking"),
        "a second Rust client in the default set churns every consumer",
    );
    let artifacts = generate_all(&contract(), &schema(), TARGETS).unwrap();
    assert!(!artifacts.contains_key("client_blocking.rs"));

    // Both at once land in different files rather than racing for one.
    let both = generate_all(&contract(), &schema(), &["client-rs", "client-rs-blocking"]).unwrap();
    assert_eq!(both.len(), 2, "{:?}", both.keys().collect::<Vec<_>>());
}

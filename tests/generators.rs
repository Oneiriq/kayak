//! Every generator over one action-bearing contract, golden-tested.
//!
//! `JANUS_BLESS=1 cargo test` re-blesses all goldens deliberately;
//! anything else that changes an artifact is drift and fails.

use janus::diff::{diff, Change};
use janus::generate::{generate_all, TARGETS};
use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure, Resource, TypeRef};
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
                        },
                        ActionField {
                            name: "max_uses".into(),
                            kind: TypeRef::Int,
                            required: false,
                            description: None,
                        },
                    ],
                    output: ActionOutput::Json,
                    description: Some("Issue a signed URL for a servable file.".into()),
                },
                Action {
                    name: "remove".into(),
                    method: "DELETE".into(),
                    path: "/{id}".into(),
                    input: vec![],
                    output: ActionOutput::None,
                    description: Some("Soft-delete the file.".into()),
                },
            ],
        }],
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
            .unwrap_or_else(|_| panic!("{golden_path} missing — JANUS_BLESS=1 to create"));
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

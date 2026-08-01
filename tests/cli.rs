//! The janus binary end to end: generate from contract+schema files,
//! diff with CI-able exit codes, and, where the toolchains exist,
//! syntax-check the generated Python and Go clients with the real
//! compilers rather than trusting the goldens alone.

use std::process::Command;

use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure, Resource, TypeRef};
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
        limits: None,
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
            sub_resources: vec![],
            actions: vec![Action {
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
            }],
        }],
    }
}

fn tool_available(name: &str, probe: &[&str]) -> bool {
    Command::new(name)
        .args(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn generate_and_diff_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let contract_path = dir.path().join("contract.json");
    let schema_path = dir.path().join("schema.json");
    let out_dir = dir.path().join("generated");
    std::fs::write(
        &contract_path,
        serde_json::to_string_pretty(&contract()).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();

    // Generate every target.
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "generate",
            "--contract",
            contract_path.to_str().unwrap(),
            "--schema",
            schema_path.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    for artifact in [
        "openapi.json",
        "schema.graphql",
        "client.rs",
        "client.ts",
        "client.py",
        "client.go",
    ] {
        assert!(out_dir.join(artifact).exists(), "{artifact} missing");
    }
    // The OpenAPI artifact parses back.
    let openapi: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("openapi.json")).unwrap())
            .unwrap();
    assert_eq!(openapi["openapi"], "3.1.0");

    // Real-compiler smoke checks, where the toolchain exists.
    if tool_available("python", &["--version"]) {
        let check = Command::new("python")
            .args([
                "-m",
                "py_compile",
                out_dir.join("client.py").to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "generated client.py does not compile: {}",
            String::from_utf8_lossy(&check.stderr),
        );
    }
    if tool_available("gofmt", &["--help"]) || tool_available("gofmt", &["-h"]) {
        // gofmt parses the file; a parse error means invalid Go.
        let check = Command::new("gofmt")
            .args(["-e", out_dir.join("client.go").to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            check.status.success(),
            "generated client.go does not parse: {}",
            String::from_utf8_lossy(&check.stderr),
        );
    }

    // Diff: identical contracts exit 0.
    let same = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "diff",
            contract_path.to_str().unwrap(),
            contract_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(same.status.success());

    // Diff: a breaking edit exits non-zero and names the break.
    let mut broken = contract();
    broken.resources[0].sortable.clear();
    let broken_path = dir.path().join("broken.json");
    std::fs::write(&broken_path, serde_json::to_string_pretty(&broken).unwrap()).unwrap();
    let breaking = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "diff",
            contract_path.to_str().unwrap(),
            broken_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !breaking.status.success(),
        "breaking diff must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&breaking.stdout);
    assert!(stdout.contains("BREAKING"), "{stdout}");
    assert!(stdout.contains("sort created_at removed"), "{stdout}");
}

//! The janus binary end to end: generate from contract+schema files,
//! diff with CI-able exit codes, and, where the toolchains exist,
//! syntax-check the generated Python and Go clients with the real
//! compilers rather than trusting the goldens alone.

use std::process::Command;

use janus::generate::TARGETS;
use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure, Query, Resource, TypeRef};
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
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![],
        auth: Default::default(),
        resources: vec![Resource {
            name: "files".into(),
            table: "file".into(),
            identity: None,
            fields: vec![
                FieldExposure::column("path"),
                FieldExposure::column("state"),
                FieldExposure::renamed("size_bytes", "size"),
                FieldExposure::column("created_at"),
            ],
            pinned: vec!["tenant_id".into()],
            pinned_either: vec![],
            filterable: vec!["state".into()],
            sortable: vec!["created_at".into()],
            max_page_size: 100,
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
            actions: vec![Action {
                name: "issue_url".into(),
                method: "POST".into(),
                path: "/{id}/url".into(),
                input: vec![ActionField {
                    name: "ttl_secs".into(),
                    kind: TypeRef::Int,
                    required: false,
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                }],
                output: ActionOutput::Json,
                description: None,
                graphql_field: None,
                requires: vec![],
                rate_class: None,
            }],
            content: None,
            filter_options: Default::default(),
            faces: Default::default(),
        }],
        // Both query shapes, so the real Python and Go toolchains
        // below parse what the generators emit for them.
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
        ],
    }
}

fn tool_available(name: &str, probe: &[&str]) -> bool {
    Command::new(name)
        .args(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The scaffold path a service with an existing database takes: hand
/// the binary a schema, get a contract, generate from it without
/// editing anything. If that chain breaks, the scaffold is a draft
/// rather than a starting point.
#[test]
fn a_scaffolded_contract_generates_without_edits() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    let contract_path = dir.path().join("contract.json");
    let out_dir = dir.path().join("generated");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();

    let scaffolded = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "scaffold",
            "--schema",
            schema_path.to_str().unwrap(),
            "--out",
            contract_path.to_str().unwrap(),
            "--name",
            "copal",
        ])
        .output()
        .unwrap();
    assert!(
        scaffolded.status.success(),
        "scaffold failed: {}",
        String::from_utf8_lossy(&scaffolded.stderr),
    );
    let notes = String::from_utf8_lossy(&scaffolded.stderr);
    assert!(
        !notes.contains("needs an edit"),
        "a scaffold that does not validate is not a starting point: {notes}",
    );

    let generated = Command::new(env!("CARGO_BIN_EXE_janus"))
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
        generated.status.success(),
        "generate from a scaffold failed: {}",
        String::from_utf8_lossy(&generated.stderr),
    );
    assert!(out_dir.join("openapi.json").exists());
    assert!(out_dir.join("schema.graphql").exists());
}

/// Without `--schema` there is nothing to read, and a usage exit is
/// how CI tells that apart from a contract that could not be built.
#[test]
fn scaffold_without_a_schema_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args(["scaffold"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// `verify` refuses with a usage exit before touching any database:
/// missing flags under the feature, and a missing feature in a build
/// without it, are both a 2 rather than a connection attempt.
#[test]
fn verify_without_flags_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args(["verify"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
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

    // Generate every target, naming the opt-in blocking client so the
    // real toolchain below sees both Rust flavours.
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "generate",
            "--contract",
            contract_path.to_str().unwrap(),
            "--schema",
            schema_path.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
            "--targets",
            &format!("{},client-rs-blocking", TARGETS.join(",")),
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
        "client_blocking.rs",
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
    // Rust the same way, since rustfmt parses before it formats. This
    // catches syntax only -- a stray `.await` in the blocking client
    // parses fine and fails to build -- so the generators suite asserts
    // separately that the blocking flavour never suspends.
    if tool_available("rustfmt", &["--version"]) {
        for client in ["client.rs", "client_blocking.rs"] {
            let check = Command::new("rustfmt")
                .args([
                    "--edition",
                    "2021",
                    "--emit",
                    "stdout",
                    out_dir.join(client).to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(
                check.status.success(),
                "generated {client} does not parse: {}",
                String::from_utf8_lossy(&check.stderr),
            );
        }
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

/// Write `whole` into `at` as one file per entity, naming each with
/// `prefix` applied to its position.
fn split_into(whole: &Contract, at: &std::path::Path, prefix: impl Fn(usize, &str) -> String) {
    std::fs::create_dir_all(at.join("resources")).unwrap();
    std::fs::create_dir_all(at.join("queries")).unwrap();
    let mut head = serde_json::to_value(whole).unwrap();
    let fields = head.as_object_mut().unwrap();
    fields.remove("resources");
    fields.remove("queries");
    std::fs::write(
        at.join("contract.json"),
        serde_json::to_string_pretty(&head).unwrap(),
    )
    .unwrap();
    for (index, resource) in whole.resources.iter().enumerate() {
        std::fs::write(
            at.join("resources").join(prefix(index, &resource.name)),
            serde_json::to_string_pretty(resource).unwrap(),
        )
        .unwrap();
    }
    for (index, query) in whole.queries.iter().enumerate() {
        std::fs::write(
            at.join("queries").join(prefix(index, &query.name)),
            serde_json::to_string_pretty(query).unwrap(),
        )
        .unwrap();
    }
}

/// A contract split one entity per file generates what the single
/// file generates.
///
/// Splitting is how a reader gets to open `resources/files.json` and
/// know that everything in front of them is that entity. Files are
/// read in the order their names sort, so a split that keeps the
/// declared order produces the documents byte for byte, which is what
/// lets a service with artifacts already checked in take the move as
/// a pure change of shape.
#[test]
fn a_directory_of_entities_generates_what_one_file_generates() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();

    let whole = contract();
    let from_file = dir.path().join("contract.json");
    std::fs::write(&from_file, serde_json::to_string_pretty(&whole).unwrap()).unwrap();

    let ordered = dir.path().join("ordered");
    split_into(&whole, &ordered, |index, name| {
        format!("{index}-{name}.json")
    });

    let generate = |contract: &std::path::Path, out: &std::path::Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_janus"))
            .args([
                "generate",
                "--contract",
                contract.to_str().unwrap(),
                "--schema",
                schema_path.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "generate from {}: {}",
            contract.display(),
            String::from_utf8_lossy(&output.stderr),
        );
    };
    let one = dir.path().join("from-one");
    let many = dir.path().join("from-many");
    generate(&from_file, &one);
    generate(&ordered, &many);

    // Read what was actually produced rather than listing it here. The
    // hand-written list this replaces had six of the seven default
    // artifacts, having silently fallen a target behind: mcp-tools.json
    // was never compared, so the manifest could have differed between
    // the two input forms and nothing would have said so.
    let mut artifacts: Vec<String> = std::fs::read_dir(&one)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    artifacts.sort();
    assert_eq!(
        artifacts.len(),
        TARGETS.len(),
        "expected one file per default target: {artifacts:?}",
    );
    for artifact in artifacts {
        assert_eq!(
            std::fs::read_to_string(one.join(&artifact)).unwrap(),
            std::fs::read_to_string(many.join(&artifact)).unwrap(),
            "{artifact} differs between the one file and the directory",
        );
    }
}

/// Where the files sort decides where the entities print, and nothing
/// else.
///
/// Naming the files without regard to the declared order moves things
/// around inside the documents, because that order is the order they
/// were handed over. It is presentation: the differ matches entities
/// by name, so it reports the reordering as no change at all, and a
/// service that does not mind the churn can name its files whatever
/// reads best.
#[test]
fn file_names_decide_the_order_and_not_the_contract() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();
    let whole = contract();
    let from_file = dir.path().join("contract.json");
    std::fs::write(&from_file, serde_json::to_string_pretty(&whole).unwrap()).unwrap();

    // `file_text` sorts before `search`; the contract declares them
    // the other way around.
    let plain = dir.path().join("plain");
    split_into(&whole, &plain, |_, name| format!("{name}.json"));
    assert!(whole.queries.len() > 1, "the fixture has an order to lose");

    let out = dir.path().join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "generate",
            "--contract",
            plain.to_str().unwrap(),
            "--schema",
            schema_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let sdl = std::fs::read_to_string(out.join("schema.graphql")).unwrap();
    assert!(
        sdl.find("fileText(").unwrap() < sdl.find("search(").unwrap(),
        "the file names set the order: {sdl}",
    );

    // The contract is the same contract either way.
    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args(["diff", from_file.to_str().unwrap(), plain.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "diff refused: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no contract changes"),
        "reordering is not a contract change: {}",
        String::from_utf8_lossy(&output.stdout),
    );
}

/// What a directory contract refuses, and what it says.
///
/// Each of these is a shape someone will write by accident. A message
/// that names the file and the problem is the difference between a
/// two-second fix and a hunt through generated output for the entity
/// that went missing.
#[test]
fn a_directory_contract_refuses_what_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();
    let whole = contract();

    let refusal = |at: &std::path::Path| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_janus"))
            .args([
                "generate",
                "--contract",
                at.to_str().unwrap(),
                "--schema",
                schema_path.to_str().unwrap(),
                "--out",
                &format!("{}-out", at.to_str().unwrap()),
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{} was accepted", at.display());
        String::from_utf8_lossy(&output.stderr).into_owned()
    };

    // Entities left in the head, where they would be read twice or
    // not at all.
    let doubled = dir.path().join("doubled");
    split_into(&whole, &doubled, |_, name| format!("{name}.json"));
    std::fs::write(
        doubled.join("contract.json"),
        serde_json::to_string_pretty(&whole).unwrap(),
    )
    .unwrap();
    let said = refusal(&doubled);
    assert!(
        said.contains("contract.json") && said.contains("resources/"),
        "names the file and where they belong: {said}",
    );

    // An extension typo, which would otherwise be a resource that
    // quietly does not exist.
    let typo = dir.path().join("typo");
    split_into(&whole, &typo, |_, name| format!("{name}.json"));
    std::fs::rename(
        typo.join("resources").join("files.json"),
        typo.join("resources").join("files.jsonc"),
    )
    .unwrap();
    let said = refusal(&typo);
    assert!(
        said.contains("files.jsonc") && said.contains(".json"),
        "names the file it will not read: {said}",
    );

    // A directory that declares nothing.
    let bare = dir.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    let mut head = serde_json::to_value(&whole).unwrap();
    let fields = head.as_object_mut().unwrap();
    fields.remove("resources");
    fields.remove("queries");
    std::fs::write(
        bare.join("contract.json"),
        serde_json::to_string_pretty(&head).unwrap(),
    )
    .unwrap();
    let said = refusal(&bare);
    assert!(said.contains("declares no"), "says so plainly: {said}");

    // No head at all.
    let headless = dir.path().join("headless");
    std::fs::create_dir_all(headless.join("resources")).unwrap();
    let said = refusal(&headless);
    assert!(
        said.contains("contract.json"),
        "names what is missing: {said}"
    );
}

/// A byte-order mark in front of a contract is read through.
///
/// Several Windows editors write one. JSON has no place for it, so
/// what a reader got back was "expected value at line 1 column 1"
/// about a character their editor does not show them.
#[test]
fn a_byte_order_mark_does_not_hide_a_contract() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();

    let whole = contract();
    // One file, and a directory, each with a mark in front.
    let marked = dir.path().join("contract.json");
    std::fs::write(
        &marked,
        format!("\u{feff}{}", serde_json::to_string_pretty(&whole).unwrap()),
    )
    .unwrap();

    let split = dir.path().join("split");
    split_into(&whole, &split, |index, name| format!("{index}-{name}.json"));
    let head = split.join("contract.json");
    let text = std::fs::read_to_string(&head).unwrap();
    std::fs::write(&head, format!("\u{feff}{text}")).unwrap();

    for (label, at) in [("one file", &marked), ("a directory", &split)] {
        let out = dir.path().join(format!("out-{}", label.replace(' ', "-")));
        let output = Command::new(env!("CARGO_BIN_EXE_janus"))
            .args([
                "generate",
                "--contract",
                at.to_str().unwrap(),
                "--schema",
                schema_path.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{label} with a mark: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// Two files cannot declare the same resource.
///
/// One name is one REST prefix and one GraphQL field. Split across
/// files the collision is easy to write and impossible to see, so the
/// refusal names it.
#[test]
fn two_files_cannot_declare_the_same_resource() {
    let dir = tempfile::tempdir().unwrap();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&vec![file_table()]).unwrap(),
    )
    .unwrap();

    let whole = contract();
    let split = dir.path().join("split");
    split_into(&whole, &split, |_, name| format!("{name}.json"));
    let first = split.join("resources").join("files.json");
    std::fs::copy(&first, split.join("resources").join("also-files.json")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_janus"))
        .args([
            "generate",
            "--contract",
            split.to_str().unwrap(),
            "--schema",
            schema_path.to_str().unwrap(),
            "--out",
            dir.path().join("out").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "the collision was accepted");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("duplicate resource") && said.contains("files"),
        "the refusal names it: {said}",
    );
}

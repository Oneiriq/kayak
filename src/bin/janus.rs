//! The janus CLI.
//!
//! ```text
//! janus scaffold --schema schema.json --out contract.json
//! janus generate --contract contract.json --schema schema.json \
//!     --out generated [--targets openapi,sdl,client-rs,...]
//! janus diff old-contract.json new-contract.json
//! ```
//!
//! Contracts and schemas travel as data: the contract is the serialized
//! IR, the schema is a serialized `Vec<TableDefinition>` exported by the
//! owning service. `diff` exits non-zero on breaking changes, so CI can
//! gate on it directly.
//!
//! `scaffold` is where a service with an existing database starts. It
//! reads the schema and writes a contract that generates, so the first
//! run produces artifacts instead of a list of claims to repair.

use std::process::ExitCode;

use janus::diff::{diff, Change};
use janus::generate::{generate_all, TARGETS};
use janus::scaffold::scaffold;
use janus::Contract;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("scaffold") => run_scaffold(&arguments[1..]),
        Some("generate") => run_generate(&arguments[1..]),
        Some("diff") => run_diff(&arguments[1..]),
        _ => {
            eprintln!(
                "usage:\n  janus scaffold --schema <file> [--out <file>] [--name <name>] \
                 [--version <semver>] [--pinned <columns>]\n  \
                 janus generate --contract <file> --schema <file> --out <dir> \
                 [--targets {}]\n  janus diff <old-contract> <new-contract>",
                TARGETS.join(","),
            );
            ExitCode::from(2)
        }
    }
}

fn flag_value<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|a| a == name)
        .and_then(|i| arguments.get(i + 1))
        .map(String::as_str)
}

/// Write a contract the schema will validate against.
///
/// Everything it declines to guess goes to stderr, so a caller piping
/// the contract onward still sees what was left out, and stdout stays
/// a contract and nothing else.
fn run_scaffold(arguments: &[String]) -> ExitCode {
    let Some(schema_path) = flag_value(arguments, "--schema") else {
        eprintln!("scaffold requires --schema");
        return ExitCode::from(2);
    };
    let name = flag_value(arguments, "--name").unwrap_or("service");
    let version = flag_value(arguments, "--version").unwrap_or("0.1.0");
    // Tenancy is the usual server-bound column, and a deployment
    // without one says so with `--pinned ""` rather than by editing
    // the result afterward.
    let pinned: Vec<String> = flag_value(arguments, "--pinned")
        .unwrap_or("tenant_id")
        .split(',')
        .map(str::trim)
        .filter(|column| !column.is_empty())
        .map(str::to_owned)
        .collect();

    let schema: Vec<surql::schema::TableDefinition> = match read_json(schema_path) {
        Ok(schema) => schema,
        Err(error) => {
            eprintln!("schema {schema_path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    if schema.is_empty() {
        eprintln!("schema {schema_path} defines no tables");
        return ExitCode::FAILURE;
    }

    let made = scaffold(name, version, &schema, &pinned);
    let text = match serde_json::to_string_pretty(&made.contract) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("serialize contract: {error}");
            return ExitCode::FAILURE;
        }
    };

    match flag_value(arguments, "--out") {
        Some(path) => {
            if let Err(error) = std::fs::write(path, format!("{text}\n")) {
                eprintln!("write {path}: {error}");
                return ExitCode::FAILURE;
            }
            eprintln!("wrote {path}");
        }
        None => println!("{text}"),
    }

    for column in &made.withheld {
        eprintln!("withheld {column}: the name suggests a secret; expose it deliberately");
    }
    // A scaffold derives claims it can prove, and a name it cannot fix:
    // a table the wire format will not accept as an identifier is the
    // editing this reports rather than hides.
    for violation in janus::validate(&made.contract, &schema) {
        eprintln!("needs an edit: {violation}");
    }
    let filters: usize = made
        .contract
        .resources
        .iter()
        .map(|r| r.filterable.len())
        .sum();
    let sorts: usize = made
        .contract
        .resources
        .iter()
        .map(|r| r.sortable.len())
        .sum();
    eprintln!(
        "{} resources, {filters} filters and {sorts} sorts derived from indexes; narrow to what the API should offer before generating",
        made.contract.resources.len(),
    );
    ExitCode::SUCCESS
}

fn run_generate(arguments: &[String]) -> ExitCode {
    let (Some(contract_path), Some(schema_path), Some(out_dir)) = (
        flag_value(arguments, "--contract"),
        flag_value(arguments, "--schema"),
        flag_value(arguments, "--out"),
    ) else {
        eprintln!("generate requires --contract, --schema, and --out");
        return ExitCode::from(2);
    };
    let targets: Vec<&str> = match flag_value(arguments, "--targets") {
        Some(list) => list.split(',').map(str::trim).collect(),
        None => TARGETS.to_vec(),
    };

    let contract: Contract = match read_json(contract_path) {
        Ok(contract) => contract,
        Err(error) => {
            eprintln!("contract {contract_path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let schema: Vec<surql::schema::TableDefinition> = match read_json(schema_path) {
        Ok(schema) => schema,
        Err(error) => {
            eprintln!("schema {schema_path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    match generate_all(&contract, &schema, &targets) {
        Ok(artifacts) => {
            if let Err(error) = std::fs::create_dir_all(out_dir) {
                eprintln!("create {out_dir}: {error}");
                return ExitCode::FAILURE;
            }
            for (filename, content) in &artifacts {
                let path = format!("{out_dir}/{filename}");
                if let Err(error) = std::fs::write(&path, content) {
                    eprintln!("write {path}: {error}");
                    return ExitCode::FAILURE;
                }
                println!("wrote {path}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_diff(arguments: &[String]) -> ExitCode {
    let (Some(old_path), Some(new_path)) = (arguments.first(), arguments.get(1)) else {
        eprintln!("diff requires <old-contract> <new-contract>");
        return ExitCode::from(2);
    };
    let old: Contract = match read_json(old_path) {
        Ok(contract) => contract,
        Err(error) => {
            eprintln!("old contract {old_path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let new: Contract = match read_json(new_path) {
        Ok(contract) => contract,
        Err(error) => {
            eprintln!("new contract {new_path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let changes = diff(&old, &new);
    let mut breaking = false;
    for change in &changes {
        match change {
            Change::Breaking(message) => {
                breaking = true;
                println!("BREAKING   {message}");
            }
            Change::Compatible(message) => println!("compatible {message}"),
        }
    }
    if changes.is_empty() {
        println!("no contract changes");
    }
    if breaking {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

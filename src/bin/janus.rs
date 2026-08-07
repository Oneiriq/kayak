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
//! Anywhere a contract is read, a directory holding one entity per file
//! is read the same way:
//!
//! ```text
//! contract/
//!   contract.json        name, version, limits, rate classes
//!   resources/files.json one Resource
//!   queries/search.json  one Query
//! ```
//!
//! Files are read in the order their names sort, which is the order
//! entities reach the generators and so the order they appear in the
//! documents.
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
                 janus generate --contract <file-or-dir> --schema <file> --out <dir> \
                 [--targets {}]\n  janus diff <old-contract> <new-contract>\n\n  a contract is one \
                 .json file, or a directory holding contract.json beside \
                 resources/*.json and queries/*.json",
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

    let contract = match read_contract(contract_path) {
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
    let old = match read_contract(old_path) {
        Ok(contract) => contract,
        Err(error) => {
            eprintln!("old contract {old_path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let new = match read_contract(new_path) {
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
    serde_json::from_str(text_of(&text)).map_err(|e| e.to_string())
}

/// The text of a file, without a byte-order mark in front of it.
///
/// Several Windows editors write one, JSON has no place for it, and
/// what a reader gets back is "expected value at line 1 column 1"
/// about a character they cannot see.
fn text_of(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// A contract, from one file or from a directory of them.
///
/// One entity per file is what lets a reader open `resources/files.json`
/// and know that everything in front of them is that entity. The
/// assembled contract is what janus validates and generates from,
/// because the checks that matter span it: a rate class a resource
/// names, a query that collides with another query.
fn read_contract(path: &str) -> Result<Contract, String> {
    let at = std::path::Path::new(path);
    if !at.is_dir() {
        return read_json(path);
    }

    // The head carries what belongs to the contract rather than to any
    // one entity. Parsed as a value first so the entity lists can be
    // refused by name, and so every default the type declares still
    // applies to what remains.
    let head_path = at.join("contract.json");
    let text =
        std::fs::read_to_string(&head_path).map_err(|e| format!("{}: {e}", head_path.display()))?;
    let mut head: serde_json::Value = serde_json::from_str(text_of(&text))
        .map_err(|e| format!("{}: {e}", head_path.display()))?;
    let Some(fields) = head.as_object_mut() else {
        return Err(format!("{}: not an object", head_path.display()));
    };
    for (key, directory) in [("resources", "resources/"), ("queries", "queries/")] {
        if fields.contains_key(key) {
            return Err(format!(
                "{}: carries {key}, which in a directory contract live one per file under {directory}",
                head_path.display(),
            ));
        }
    }
    fields.insert("resources".into(), serde_json::Value::Array(Vec::new()));
    let mut contract: Contract =
        serde_json::from_value(head).map_err(|e| format!("{}: {e}", head_path.display()))?;

    contract.resources = read_each(&at.join("resources"))?;
    contract.queries = read_each(&at.join("queries"))?;
    if contract.resources.is_empty() && contract.queries.is_empty() {
        return Err(format!(
            "{}: declares no resources and no queries",
            at.display()
        ));
    }
    Ok(contract)
}

/// Every `.json` file in one directory, in the order their names sort.
///
/// Sorted because the artifacts are checked in and diffed: the order
/// entities reach the generators decides the order they appear in the
/// documents, and a directory listing is not an order anyone chose. A
/// file that is not `.json` is refused rather than skipped, so a typo
/// in an extension is a message instead of a missing resource.
fn read_each<T: serde::de::DeserializeOwned>(at: &std::path::Path) -> Result<Vec<T>, String> {
    if !at.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", at.display()))?;
        let path = entry.path();
        if path.is_dir() {
            return Err(format!("{}: holds a directory", path.display()));
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            return Err(format!("{}: not a .json file", path.display()));
        }
        paths.push(path);
    }
    paths.sort();

    let mut out = Vec::new();
    for path in paths {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push(
            serde_json::from_str(text_of(&text)).map_err(|e| format!("{}: {e}", path.display()))?,
        );
    }
    Ok(out)
}

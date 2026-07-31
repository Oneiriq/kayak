//! The janus CLI.
//!
//! ```text
//! janus generate --contract contract.json --schema schema.json \
//!     --out generated [--targets openapi,sdl,client-rs,...]
//! janus diff old-contract.json new-contract.json
//! ```
//!
//! Contracts and schemas travel as data: the contract is the serialized
//! IR, the schema is a serialized `Vec<TableDefinition>` exported by the
//! owning service. `diff` exits non-zero on breaking changes, so CI can
//! gate on it directly.

use std::process::ExitCode;

use janus::diff::{diff, Change};
use janus::generate::{generate_all, TARGETS};
use janus::Contract;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("generate") => run_generate(&arguments[1..]),
        Some("diff") => run_diff(&arguments[1..]),
        _ => {
            eprintln!(
                "usage:\n  janus generate --contract <file> --schema <file> --out <dir> \
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

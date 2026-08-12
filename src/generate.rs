//! Generation orchestration: one call, every requested artifact.
//!
//! The CLI is a thin wrapper over [`generate_all`], so tests exercise
//! the real logic without spawning processes.

use std::collections::BTreeMap;

use surql::schema::TableDefinition;

use crate::clients::{
    generate_client_go, generate_client_py, generate_client_rs, generate_client_ts,
};
use crate::ir::Contract;
use crate::openapi::{generate_openapi, GenerateError};
use crate::sdl::generate_sdl;

/// The targets one `generate` run produces when none are named.
///
/// `engine-policy` is accepted but deliberately not among them: its
/// clauses render through a token-claim vocabulary the deployment
/// owns, and the CLI holds only the default conventions. A contract
/// naming a guard outside them would fail the whole default run over
/// a face the caller never asked for, so the policy artifact is
/// opt-in by name.
pub const TARGETS: &[&str] = &[
    "openapi",
    "sdl",
    "mcp",
    "client-rs",
    "client-ts",
    "client-py",
    "client-go",
];

/// Generate the requested targets, returning filename -> content.
pub fn generate_all(
    contract: &Contract,
    schema: &[TableDefinition],
    targets: &[&str],
) -> Result<BTreeMap<String, String>, GenerateError> {
    let mut artifacts = BTreeMap::new();
    for target in targets {
        let (filename, content) = match *target {
            "mcp" => (
                "mcp-tools.json".to_owned(),
                format!(
                    "{}
",
                    serde_json::to_string_pretty(&crate::mcp::generate_mcp_tools(contract))
                        .expect("mcp manifest serializes"),
                ),
            ),
            "openapi" => (
                "openapi.json".to_owned(),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&generate_openapi(contract, schema)?)
                        .expect("openapi document serializes"),
                ),
            ),
            "sdl" => ("schema.graphql".to_owned(), generate_sdl(contract, schema)?),
            // Rendered with the default claim vocabulary; a deployment
            // whose tokens spell claims differently derives through
            // [`crate::policy::derive_policy`] instead, which is why
            // this target is opt-in rather than a default. The
            // artifact exists so the engine's row security is
            // review-visible beside the surfaces it mirrors: a scope
            // tightened in the contract shows up here as a changed
            // clause in the same commit. A guard outside the
            // vocabulary refuses the run, naming the guard.
            "engine-policy" => (
                "policy.json".to_owned(),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&crate::policy::derive_policy(
                        contract,
                        &crate::policy::ClaimVocabulary::default(),
                    )?)
                    .expect("engine policy serializes"),
                ),
            ),
            "client-rs" => (
                "client.rs".to_owned(),
                generate_client_rs(contract, schema)?,
            ),
            "client-ts" => (
                "client.ts".to_owned(),
                generate_client_ts(contract, schema)?,
            ),
            "client-py" => (
                "client.py".to_owned(),
                generate_client_py(contract, schema)?,
            ),
            "client-go" => (
                "client.go".to_owned(),
                generate_client_go(contract, schema)?,
            ),
            other => {
                return Err(GenerateError::UnknownTarget(other.to_owned()));
            }
        };
        artifacts.insert(filename, content);
    }
    Ok(artifacts)
}

//! Engine row security derived from the contract: the eighth face.
//!
//! A SurrealDB deployment can enforce a second time what the
//! application already enforces: table `PERMISSIONS` filter rows and
//! field `PERMISSIONS` redact columns for sessions authenticated as
//! callers rather than as the service. The clauses worth having are
//! exactly what the contract already declares — a resource whose
//! reads require a scope should admit only sessions holding it, and
//! a guarded field should come back absent for sessions its guard
//! denies. Deriving those clauses by hand in the service is how the
//! two layers drift: tighten a scope in the contract, forget to
//! re-derive, and the API refuses what the engine still serves, a
//! divergence nothing names because the differ never sees it. So the
//! derivation lives here, beside the seven faces it agrees with, and
//! the contract changes that move it (`reads_require`, a field's
//! guard) are classified by [`crate::diff`] like every other face's
//! inputs.
//!
//! Two things deliberately stay OUT of this module, and their absence
//! is a boundary rather than a gap:
//!
//! - **The mechanical tenancy floor.** A service that scopes tables
//!   by tenant wants every tenant-scoped table to admit only its
//!   tenant's rows and every table without the column closed to
//!   caller sessions entirely. That rule must derive from the SCHEMA,
//!   not the contract: the contract lists what is exposed, so a floor
//!   derived from it would be dodgeable by omission — a future table
//!   left out of the contract would sit above the floor instead of
//!   under it. The service keeps the floor where the schema lives;
//!   janus derives only what the contract declares.
//! - **Delete conjuncts.** Retention rules (a version row may be
//!   deleted only when nothing binds it) are enforceable policy the
//!   contract cannot declare yet. Until the IR can say them, they are
//!   the service's to state explicitly, not this module's to invent.
//!
//! The rendered strings speak a token-claim vocabulary — which claim
//! carries the scope list, what clause a named guard becomes — and
//! that vocabulary is deployment convention, not contract content.
//! [`ClaimVocabulary`] carries it, with defaults matching copal's
//! caller tokens, so the reference deployment's switch to this
//! derivation is a behavioral no-op (proven byte-for-byte in
//! `tests/policy.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::{Contract, FieldExposure};

/// The token-claim vocabulary the rendered clauses speak.
///
/// The contract says a read requires the `read` scope; only the
/// deployment knows the scope list rides its caller tokens as `sc`.
/// Splitting the two keeps the contract portable across deployments
/// whose tokens disagree, and keeps claim names out of the checked-in
/// IR where they would masquerade as API surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimVocabulary {
    /// The token claim holding the caller's scope list. A required
    /// scope renders as `$token.<claim> CONTAINS '<scope>'`.
    pub scopes_claim: String,
    /// The engine clause each named contract guard becomes. A guard
    /// the contract declares without an entry here refuses the
    /// derivation: rendering nothing would silently drop the engine
    /// layer for that column while the application layer kept
    /// enforcing, and the two layers exist to agree.
    pub guard_clauses: BTreeMap<String, String>,
}

impl Default for ClaimVocabulary {
    /// Copal's caller-token conventions: scopes ride as `sc`, the
    /// admin claim as `adm`, the principal handle as `pr`. Defaults so
    /// the reference deployment adopts this module without behavior
    /// change; any other deployment overrides what its tokens spell
    /// differently.
    fn default() -> Self {
        Self {
            scopes_claim: "sc".to_owned(),
            guard_clauses: BTreeMap::from([
                ("admin_only".to_owned(), "$token.adm = true".to_owned()),
                // Ownership at the engine: the author's principal
                // handle rides the token, so the second layer can say
                // what the application guard says. Tokens without the
                // claim fail the comparison, which is the
                // unknown-authorship rule again: treating unknown
                // authorship as ownership would widen access.
                (
                    "owner_or_admin".to_owned(),
                    "$token.adm = true OR created_by = $token.pr".to_owned(),
                ),
            ]),
        }
    }
}

/// The derived policy: what the contract declares, rendered as engine
/// clause strings for the service to fold into its schema
/// definitions. Serializable so it can travel as an artifact and be
/// reviewed beside the other seven.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnginePolicy {
    /// `(table, column, select clause)`: the engine redacts the
    /// column for caller sessions the clause denies.
    pub field_guards: Vec<(String, String, String)>,
    /// `(table, conjunct)`: appended to the table's select rule, for
    /// contract-backed tables whose reads require a scope.
    pub select_conjuncts: Vec<(String, String)>,
}

/// Why a policy could not be derived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// A guard the contract names has no clause in the vocabulary.
    #[error("contract guard {0:?} has no engine clause; add one before shipping the guard")]
    MissingGuardClause(String),
}

/// Derive the engine policy from the contract, so both enforcement
/// layers read one declaration set.
///
/// Field guards become engine column redactions, for resources and
/// their sub-resources alike. A resource's read scopes become one
/// conjunct on its table's select rule, and the same conjunct lands
/// on every sub-resource table, because a sub-collection is read
/// under its parent's requirement — it is reached through the parent,
/// and the engine face mirrors how the dispatcher enforces reads.
pub fn derive_policy(
    contract: &Contract,
    vocabulary: &ClaimVocabulary,
) -> Result<EnginePolicy, PolicyError> {
    let mut policy = EnginePolicy::default();
    for resource in &contract.resources {
        collect_guards(
            &resource.table,
            &resource.fields,
            vocabulary,
            &mut policy.field_guards,
        )?;
        for sub in &resource.sub_resources {
            collect_guards(
                &sub.table,
                &sub.fields,
                vocabulary,
                &mut policy.field_guards,
            )?;
        }
    }
    for resource in &contract.resources {
        if resource.reads_require.is_empty() {
            continue;
        }
        let conjunct = scope_conjunct(&resource.reads_require, vocabulary);
        policy
            .select_conjuncts
            .push((resource.table.clone(), conjunct.clone()));
        for sub in &resource.sub_resources {
            policy
                .select_conjuncts
                .push((sub.table.clone(), conjunct.clone()));
        }
    }
    Ok(policy)
}

/// One table's guarded exposures, rendered through the vocabulary.
fn collect_guards(
    table: &str,
    fields: &[FieldExposure],
    vocabulary: &ClaimVocabulary,
    into: &mut Vec<(String, String, String)>,
) -> Result<(), PolicyError> {
    for field in fields {
        let Some(guard) = &field.guard else {
            continue;
        };
        let clause = vocabulary
            .guard_clauses
            .get(guard)
            .ok_or_else(|| PolicyError::MissingGuardClause(guard.clone()))?;
        into.push((table.to_owned(), field.column.clone(), clause.clone()));
    }
    Ok(())
}

/// Required scopes as one engine conjunct: every scope must be held,
/// so the containment checks join with AND.
fn scope_conjunct(scopes: &[String], vocabulary: &ClaimVocabulary) -> String {
    scopes
        .iter()
        .map(|scope| format!("$token.{} CONTAINS '{scope}'", vocabulary.scopes_claim))
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Resource;

    fn resource(name: &str, table: &str) -> Resource {
        Resource {
            name: name.into(),
            table: table.into(),
            fields: vec![FieldExposure::column("path")],
            pinned: vec![],
            filterable: vec![],
            filter_options: Default::default(),
            faces: Default::default(),
            sortable: vec![],
            max_page_size: 100,
            actions: vec![],
            content: None,
            sub_resources: vec![],
            rate_class: None,
            reads_require: vec![],
            watchable: false,
            graphql: None,
        }
    }

    fn contract(resources: Vec<Resource>) -> Contract {
        Contract {
            name: "probe".into(),
            version: "0.1.0".into(),
            ir_revision: 1,
            api_prefix: "/v1".into(),
            rate_classes: vec![],
            limits: None,
            auth: Default::default(),
            resources,
            queries: vec![],
        }
    }

    #[test]
    fn an_open_contract_derives_an_empty_policy() {
        let policy = derive_policy(
            &contract(vec![resource("files", "file")]),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(policy, EnginePolicy::default());
    }

    #[test]
    fn every_required_scope_must_be_held() {
        let mut needy = resource("files", "file");
        needy.reads_require = vec!["read".into(), "audit".into()];
        let policy = derive_policy(&contract(vec![needy]), &Default::default()).unwrap();
        assert_eq!(
            policy.select_conjuncts,
            vec![(
                "file".to_owned(),
                "$token.sc CONTAINS 'read' AND $token.sc CONTAINS 'audit'".to_owned(),
            )],
        );
    }

    #[test]
    fn a_guard_outside_the_vocabulary_refuses_the_derivation() {
        let mut guarded = resource("files", "file");
        guarded.fields = vec![FieldExposure::column("digest").with_guard("finance_only")];
        let error = derive_policy(&contract(vec![guarded]), &Default::default()).unwrap_err();
        assert_eq!(
            error,
            PolicyError::MissingGuardClause("finance_only".into())
        );
        assert!(error.to_string().contains("finance_only"), "{error}");
    }

    #[test]
    fn the_vocabulary_is_the_deployments_to_respell() {
        let mut needy = resource("files", "file");
        needy.reads_require = vec!["read".into()];
        needy.fields = vec![FieldExposure::column("digest").with_guard("audit_only")];
        let vocabulary = ClaimVocabulary {
            scopes_claim: "scopes".into(),
            guard_clauses: BTreeMap::from([(
                "audit_only".to_owned(),
                "$token.roles CONTAINS 'auditor'".to_owned(),
            )]),
        };
        let policy = derive_policy(&contract(vec![needy]), &vocabulary).unwrap();
        assert_eq!(
            policy.select_conjuncts,
            vec![(
                "file".to_owned(),
                "$token.scopes CONTAINS 'read'".to_owned()
            )],
        );
        assert_eq!(
            policy.field_guards,
            vec![(
                "file".to_owned(),
                "digest".to_owned(),
                "$token.roles CONTAINS 'auditor'".to_owned(),
            )],
        );
    }

    #[test]
    fn the_policy_travels_as_data() {
        let mut needy = resource("files", "file");
        needy.reads_require = vec!["read".into()];
        let policy = derive_policy(&contract(vec![needy]), &Default::default()).unwrap();
        let json = serde_json::to_string(&policy).unwrap();
        let back: EnginePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, policy);
    }
}

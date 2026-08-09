//! Plan verification against a live database.
//!
//! Static validation proves an index EXISTS for every filter and sort
//! claim; it cannot prove the planner USES it. The gap is real: an
//! index can cover the right columns in an order the composed listing
//! cannot seek, a session parameter can change how the engine plans,
//! and an engine upgrade can re-cost a plan overnight. Every one of
//! those ships a listing that answers correctly and walks the table
//! to do it, which is the exact failure the static gate exists to
//! prevent, one layer down where the static gate cannot see.
//!
//! So this module asks the planner itself. For each resource it
//! composes one representative listing per filter claim and per sort
//! claim — the pins as equality binds, the claimed filter bound, the
//! claimed sort ordered, always with a `LIMIT` — and runs `EXPLAIN`
//! against a live database. A claim whose plan falls back to
//! iterating the table fails by name, the same way a validation
//! violation names its claim.
//!
//! The module rides the `verify` cargo feature because it is the one
//! part of janus that needs a database client, and janus deliberately
//! carries none: generation and diffing must stay runnable in CI jobs
//! and build scripts that have no database and no TLS stack. The
//! feature follows the `runtime`/`graphql`/`console` precedent: the
//! capability is real, and so is the cost of carrying it, so the
//! consumer chooses.

use serde_json::Value;

use surql::DatabaseClient;

use crate::ir::{Contract, Resource, SubResource};

/// One composed listing probe: which claim it exercises, and the
/// query whose plan answers for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The resource (or `resource.sub-resource`) holding the claim.
    pub scope: String,
    /// The claim, named the way validation names it: `filter state`,
    /// `sort created_at`.
    pub claim: String,
    /// The composed listing, `EXPLAIN` included.
    pub surql: String,
}

/// A claim the planner answered with a table walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanViolation {
    /// The resource (or `resource.sub-resource`) holding the claim.
    pub scope: String,
    /// The claim whose representative listing scanned.
    pub claim: String,
    /// The query that was planned.
    pub surql: String,
    /// The plan operation that convicted it.
    pub operation: String,
}

impl std::fmt::Display for PlanViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} plans as {:?} (the planner walks the table for: {})",
            self.scope, self.claim, self.operation, self.surql,
        )
    }
}

/// Why verification itself could not run (distinct from a claim
/// failing: a failed claim is a result, an unreachable database or a
/// refused query is not).
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The engine refused a probe query.
    #[error("probe {surql:?}: {reason}")]
    Query {
        /// The probe that was refused.
        surql: String,
        /// The engine's refusal.
        reason: String,
    },
    /// The engine answered a probe with something other than a plan.
    #[error("probe {surql:?}: expected a plan, got {got}")]
    NotAPlan {
        /// The probe whose answer had no plan shape.
        surql: String,
        /// What came back instead.
        got: String,
    },
}

/// Compose every probe the contract implies, without running any.
///
/// Pure so the composition is testable without an engine, and public
/// so a caller can see exactly what will be asked before pointing the
/// asker at production.
pub fn probes(contract: &Contract) -> Vec<Probe> {
    let mut out = Vec::new();
    for resource in &contract.resources {
        resource_probes(resource, &mut out);
        for sub in &resource.sub_resources {
            sub_resource_probes(&resource.name, sub, &mut out);
        }
    }
    out
}

/// Run every probe through `EXPLAIN`, returning the claims whose plan
/// walks the table. An empty vec is a verified contract.
pub async fn verify_contract(
    client: &DatabaseClient,
    contract: &Contract,
) -> Result<Vec<PlanViolation>, VerifyError> {
    let mut violations = Vec::new();
    for probe in probes(contract) {
        let answer = client
            .query(&probe.surql)
            .await
            .map_err(|e| VerifyError::Query {
                surql: probe.surql.clone(),
                reason: e.to_string(),
            })?;
        let plan = answer
            .get(0)
            .filter(|v| v.get("operator").is_some())
            .ok_or_else(|| VerifyError::NotAPlan {
                surql: probe.surql.clone(),
                got: answer.to_string(),
            })?;
        if let Some(operation) = table_walk(plan) {
            violations.push(PlanViolation {
                scope: probe.scope,
                claim: probe.claim,
                surql: probe.surql,
                operation,
            });
        }
    }
    Ok(violations)
}

fn resource_probes(resource: &Resource, out: &mut Vec<Probe>) {
    let pins: Vec<&str> = resource.pinned.iter().map(String::as_str).collect();
    push_probes(
        &resource.name,
        &resource.table,
        &pins,
        &resource.filterable,
        &resource.sortable,
        resource.max_page_size,
        |column| {
            resource
                .filter_options
                .get(column)
                .and_then(|options| options.first())
                .map(String::as_str)
        },
        out,
    );
}

fn sub_resource_probes(parent: &str, sub: &SubResource, out: &mut Vec<Probe>) {
    let pins = sub.bound_columns();
    push_probes(
        &format!("{parent}.{}", sub.name),
        &sub.table,
        &pins,
        &sub.filterable,
        &sub.sortable,
        sub.max_page_size,
        |_| None,
        out,
    );
}

/// One probe per filter claim and one per sort claim, over the same
/// bound spine a real listing carries.
#[allow(clippy::too_many_arguments)]
fn push_probes<'a>(
    scope: &str,
    table: &str,
    pins: &[&str],
    filterable: &[String],
    sortable: &[String],
    limit: u32,
    option_for: impl Fn(&str) -> Option<&'a str>,
    out: &mut Vec<Probe>,
) {
    let pin_binds: Vec<String> = pins
        .iter()
        .map(|column| format!("{column} = {}", probe_value(option_for(column))))
        .collect();
    for column in filterable {
        let mut binds = pin_binds.clone();
        binds.push(format!(
            "{column} = {}",
            probe_value(option_for(column.as_str()))
        ));
        out.push(Probe {
            scope: scope.to_owned(),
            claim: format!("filter {column}"),
            surql: format!(
                "SELECT * FROM {table} WHERE {} LIMIT {limit} EXPLAIN",
                binds.join(" AND "),
            ),
        });
    }
    for column in sortable {
        let where_clause = if pin_binds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", pin_binds.join(" AND "))
        };
        out.push(Probe {
            scope: scope.to_owned(),
            claim: format!("sort {column}"),
            surql: format!(
                "SELECT * FROM {table}{where_clause} ORDER BY {column} LIMIT {limit} EXPLAIN",
            ),
        });
    }
}

/// The literal a probe binds. A declared option is used when the
/// column has a closed set, because that is a value the API will
/// really see; otherwise a marker string, since `EXPLAIN` plans
/// without executing and the planner chooses on predicate shape, not
/// on whether a row matches.
fn probe_value(option: Option<&str>) -> String {
    let value = option.unwrap_or("janus-probe");
    format!("'{}'", value.replace('\'', "\\'"))
}

/// The operation that walks a table, found anywhere in the plan tree.
///
/// The vocabulary is PROBED, not guessed, against SurrealDB 3.x on
/// `mem://` (the probe lives on as the vocabulary test in
/// `tests/verify.rs`, so an engine upgrade that respells it fails
/// there by name instead of turning every verification silently
/// green). `EXPLAIN` returns one plan-tree object per statement —
/// `operator`, `attributes`, `children` — and the two scan leaves
/// observed are:
///
/// ```text
/// {"operator": "IndexScan", "attributes": {"index": "idx_listing",
///  "access": "['t1', 'ready']", "direction": "Forward", "limit": "5"}}
/// {"operator": "TableScan", "attributes": {"table": "file",
///  "direction": "Forward", "topk_pushdown": "yes"}}
/// ```
///
/// A sort the index order cannot serve shows as a `SortTopKByKey`
/// node OVER whichever scan feeds it (and an index-served sort elides
/// the sort node entirely), while a filter the index cannot narrow
/// becomes a residual `Filter` node over the pins' `IndexScan`. So
/// the scan leaf alone decides: the plan iterates the table exactly
/// when a `TableScan` node appears anywhere in the tree.
fn table_walk(node: &Value) -> Option<String> {
    if node.get("operator").and_then(Value::as_str) == Some("TableScan") {
        let table = node
            .pointer("/attributes/table")
            .and_then(Value::as_str)
            .unwrap_or("?");
        return Some(format!("TableScan over {table}"));
    }
    node.get("children")?
        .as_array()?
        .iter()
        .find_map(table_walk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FieldExposure;

    fn resource() -> Resource {
        Resource {
            name: "files".into(),
            table: "file".into(),
            fields: vec![FieldExposure::column("path")],
            pinned: vec!["tenant_id".into()],
            filterable: vec!["state".into()],
            filter_options: [(
                "state".to_owned(),
                vec!["ready".to_owned(), "failed".to_owned()],
            )]
            .into_iter()
            .collect(),
            sortable: vec!["created_at".into()],
            max_page_size: 100,
            actions: vec![],
            content: None,
            sub_resources: vec![SubResource {
                name: "versions".into(),
                table: "file_version".into(),
                parent_key: "file".into(),
                fields: vec![FieldExposure::column("number")],
                pinned: vec!["tenant_id".into()],
                filterable: vec![],
                sortable: vec!["created_at".into()],
                max_page_size: 50,
                description: None,
                graphql: None,
            }],
            rate_class: None,
            reads_require: vec![],
            watchable: false,
            graphql: None,
        }
    }

    fn contract() -> Contract {
        Contract {
            name: "probe".into(),
            version: "0.1.0".into(),
            ir_revision: 1,
            rate_classes: vec![],
            limits: None,
            resources: vec![resource()],
            queries: vec![],
        }
    }

    #[test]
    fn probes_carry_the_bound_spine_of_a_real_listing() {
        let composed = probes(&contract());
        let filter = composed
            .iter()
            .find(|p| p.scope == "files" && p.claim == "filter state")
            .expect("the filter claim is probed");
        // Pins first, the claim bound, the declared option as the
        // value, and the resource's own page ceiling as the LIMIT.
        assert_eq!(
            filter.surql,
            "SELECT * FROM file WHERE tenant_id = 'janus-probe' AND state = 'ready' \
             LIMIT 100 EXPLAIN",
        );
        let sort = composed
            .iter()
            .find(|p| p.scope == "files" && p.claim == "sort created_at")
            .expect("the sort claim is probed");
        assert_eq!(
            sort.surql,
            "SELECT * FROM file WHERE tenant_id = 'janus-probe' \
             ORDER BY created_at LIMIT 100 EXPLAIN",
        );
        // The sub-resource's parent key is bound the way the server
        // binds it, ahead of its own pins.
        let sub = composed
            .iter()
            .find(|p| p.scope == "files.versions")
            .expect("the sub-resource claim is probed");
        assert_eq!(
            sub.surql,
            "SELECT * FROM file_version WHERE file = 'janus-probe' AND \
             tenant_id = 'janus-probe' ORDER BY created_at LIMIT 50 EXPLAIN",
        );
    }

    #[test]
    fn a_quoted_value_cannot_break_out_of_its_literal() {
        assert_eq!(probe_value(Some("it's")), "'it\\'s'");
    }

    #[test]
    fn the_walk_finds_a_table_scan_at_any_depth() {
        // The exact shape the probe returned for an unserved sort:
        // SortTopKByKey over TableScan, under Limit and SelectProject.
        let plan = serde_json::json!({
            "operator": "SelectProject",
            "children": [{
                "operator": "Limit",
                "children": [{
                    "operator": "SortTopKByKey",
                    "children": [{
                        "operator": "TableScan",
                        "attributes": {"table": "file", "direction": "Forward"},
                    }],
                }],
            }],
        });
        assert_eq!(table_walk(&plan), Some("TableScan over file".to_owned()));

        let seeking = serde_json::json!({
            "operator": "SelectProject",
            "children": [{
                "operator": "IndexScan",
                "attributes": {"index": "idx_listing", "access": "['t1']"},
            }],
        });
        assert_eq!(table_walk(&seeking), None);
    }
}

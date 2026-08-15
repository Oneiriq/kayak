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
//! Search backings get the same treatment through their own
//! operators: a lexical backing is probed with `@@` and holds only if
//! the plan reaches the NAMED index, a vector backing with the
//! `<|k,EF|>` KNN form likewise. Reaching the named index is a
//! stronger demand than not scanning, deliberately — a search served
//! by some other index than the declared one is drift the contract
//! exists to catch. One boundary is the engine's, stated rather than
//! papered over: SurrealDB 3.x has removed MTREE (`DEFINE INDEX ...
//! MTREE` no longer parses, and `<|k|>` errors with "no longer
//! supported"), so while static validation accepts an MTREE-typed
//! definition for a vector backing, a live 3.x database cannot hold
//! one and verification composes only the HNSW form.
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

use crate::ir::{Contract, Query, Resource, SearchKind, SubResource};

/// One composed probe: which claim it exercises, the query whose plan
/// answers for it, and what the plan must show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The resource (or `resource.sub-resource`, or query) holding
    /// the claim.
    pub scope: String,
    /// The claim, named the way validation names it: `filter state`,
    /// `sort created_at`, `lexical backing text_chunk.body`.
    pub claim: String,
    /// The composed statement, `EXPLAIN` included.
    pub surql: String,
    /// What the plan must show for the claim to hold.
    pub expects: Expectation,
}

/// What convicts a probe's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// A listing claim: the plan must not fall back to iterating the
    /// table. WHICH index seeks is the planner's choice; any seek is
    /// a served listing.
    NoTableWalk,
    /// A search backing: the plan must reach the named index. Not
    /// scanning is not enough here, because the backing names its
    /// machinery — a search answered through some other index is the
    /// contract promising one thing and the deployment doing another.
    ReachesIndex(String),
    /// An optional backing: the plan must reach the named index
    /// wherever this database holds it. The claim is about machinery
    /// that is configured, not a claim that it is configured, so a
    /// database without the index answers by not having it — and a
    /// database with it answers to the standard above, unrelaxed.
    ReachesIndexIfDefined {
        /// The table the index would be defined on.
        table: String,
        /// The index the backing rests on where it exists.
        index: String,
    },
}

/// A claim the planner answered with something other than what the
/// claim rests on: a table walk for a listing, anything but the named
/// index for a search backing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanViolation {
    /// The resource (or `resource.sub-resource`, or query) holding
    /// the claim.
    pub scope: String,
    /// The claim whose representative probe was not served.
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
            "{}: {} plans as {:?} (probed with: {})",
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
    for query in &contract.queries {
        query_probes(query, &mut out);
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
        let conviction = match &probe.expects {
            Expectation::NoTableWalk => table_walk(plan),
            Expectation::ReachesIndex(index) => {
                if reaches_index(plan, index) {
                    None
                } else {
                    Some(plan_summary(plan))
                }
            }
            Expectation::ReachesIndexIfDefined { table, index } => {
                if reaches_index(plan, index) {
                    None
                } else {
                    // The plan is already summarised before the extra
                    // round trip is spent, because the answer only
                    // decides whether to REPORT what was already
                    // observed.
                    let summary = plan_summary(plan);
                    index_defined(client, table, index)
                        .await?
                        .then_some(summary)
                }
            }
        };
        if let Some(operation) = conviction {
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

/// Whether this database holds `index` on `table`.
///
/// Asked only when an optional backing's probe did not reach its
/// index, which is the only moment the answer changes anything: an
/// index the plan reached is serving whether or not anyone meant to
/// configure it, and a required backing owes the index regardless. So
/// the extra round trip is spent exactly once per optional backing
/// that came back unserved, and never on a verified contract.
async fn index_defined(
    client: &DatabaseClient,
    table: &str,
    index: &str,
) -> Result<bool, VerifyError> {
    let surql = format!("INFO FOR TABLE {table}");
    let answer = client
        .query(&surql)
        .await
        .map_err(|error| VerifyError::Query {
            surql: surql.clone(),
            reason: error.to_string(),
        })?;
    // `INFO FOR TABLE` answers with the definitions grouped by kind,
    // each group a map of name to the `DEFINE` statement that made it.
    Ok(answer
        .get(0)
        .and_then(|info| info.get("indexes"))
        .and_then(Value::as_object)
        .is_some_and(|indexes| indexes.contains_key(index)))
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
            expects: Expectation::NoTableWalk,
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
            expects: Expectation::NoTableWalk,
        });
    }
}

/// One probe per search backing, through the backing's own operator.
///
/// The probes are minimal on purpose: no pins, no residual filters,
/// just the operator the backing claims machinery for, because the
/// question is whether THAT operator reaches THAT index. The vector
/// literal is `[0]` whatever the index's dimension — probed on
/// SurrealDB 3.x, the planner resolves the index before it ever looks
/// at the literal's width (`KnnScan` either way, see the vocabulary
/// note on [`reaches_index`]) — so a contract needs no knowledge of
/// the embedding dimension to be verified.
fn query_probes(query: &Query, out: &mut Vec<Probe>) {
    for backing in &query.backing {
        let surql = match backing.kind {
            SearchKind::Lexical => format!(
                "SELECT * FROM {} WHERE {} @@ 'janus-probe' EXPLAIN",
                backing.table, backing.column,
            ),
            SearchKind::Vector => format!(
                "SELECT * FROM {} WHERE {} <|1,64|> [0] EXPLAIN",
                backing.table, backing.column,
            ),
        };
        out.push(Probe {
            scope: format!("query {}", query.name),
            claim: format!(
                "{} backing {}.{}",
                backing.kind, backing.table, backing.column,
            ),
            surql,
            expects: if backing.optional {
                Expectation::ReachesIndexIfDefined {
                    table: backing.table.clone(),
                    index: backing.index.clone(),
                }
            } else {
                Expectation::ReachesIndex(backing.index.clone())
            },
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

/// Whether the plan reaches the named index, anywhere in the tree.
///
/// The search vocabulary is PROBED the same way the scan vocabulary
/// was, against SurrealDB 3.x on `mem://` (and pinned alongside it in
/// `tests/verify.rs`). A `@@` predicate the FULLTEXT index serves and
/// a `<|k,EF|>` KNN the HNSW index serves each answer with one leaf
/// naming the index in its attributes:
///
/// ```text
/// SELECT * FROM text_chunk WHERE body @@ 'quick' EXPLAIN
/// -> {"operator": "FullTextScan", "attributes":
///     {"index": "idx_chunk_body", "query": "quick"}}
///
/// SELECT * FROM text_chunk WHERE embedding <|4,40|> [0.1, 0.2, 0.3] EXPLAIN
/// -> {"operator": "KnnScan", "attributes": {"index": "idx_chunk_embedding",
///     "dimension": "3", "ef": "40", "k": "4"}}
/// ```
///
/// With the index absent, `@@` degrades to a `TableScan` carrying the
/// predicate as an attribute, and `<|k,EF|>` to a bare `TableScan`;
/// the metric KNN form `<|k,COSINE|>` plans as `KnnTopK` OVER a
/// `TableScan` even when an HNSW index exists, which is why copal
/// renders the `<|k,EF|>` form and why the probe does too. Matching
/// on the `index` attribute rather than on the operator names keeps
/// the walker one rule for both kinds, and means a future operator
/// respelling fails the pinned vocabulary test instead of silently
/// widening what passes.
fn reaches_index(node: &Value, index: &str) -> bool {
    if node.pointer("/attributes/index").and_then(Value::as_str) == Some(index) {
        return true;
    }
    node.get("children")
        .and_then(Value::as_array)
        .is_some_and(|children| children.iter().any(|child| reaches_index(child, index)))
}

/// What a plan that missed its index was doing instead, for the
/// violation message: the table walk if there is one (the ordinary
/// degradation), otherwise whichever index-bearing node answered (the
/// exotic one: served, but not by the declared machinery), otherwise
/// the root operator.
fn plan_summary(node: &Value) -> String {
    if let Some(walk) = table_walk(node) {
        return walk;
    }
    if let Some(found) = other_index(node) {
        return found;
    }
    node.get("operator")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned()
}

fn other_index(node: &Value) -> Option<String> {
    if let Some(index) = node.pointer("/attributes/index").and_then(Value::as_str) {
        let operator = node.get("operator").and_then(Value::as_str).unwrap_or("?");
        return Some(format!("{operator} via {index}"));
    }
    node.get("children")?
        .as_array()?
        .iter()
        .find_map(other_index)
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
            auth: Default::default(),
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
    fn a_backing_is_probed_through_its_own_operator() {
        let mut contract = contract();
        contract.queries = vec![crate::ir::Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
            searches: vec![SearchKind::Lexical, SearchKind::Vector],
            backing: vec![
                crate::ir::SearchBacking {
                    table: "text_chunk".into(),
                    column: "body".into(),
                    index: "idx_chunk_body".into(),
                    kind: SearchKind::Lexical,
                    dimension: None,
                    optional: false,
                },
                crate::ir::SearchBacking {
                    table: "text_chunk".into(),
                    column: "embedding".into(),
                    index: "idx_chunk_embedding".into(),
                    kind: SearchKind::Vector,
                    dimension: Some(768),
                    optional: false,
                },
            ],
        }];
        let composed = probes(&contract);
        let lexical = composed
            .iter()
            .find(|p| p.claim == "lexical backing text_chunk.body")
            .expect("the lexical backing is probed");
        assert_eq!(
            lexical.surql,
            "SELECT * FROM text_chunk WHERE body @@ 'janus-probe' EXPLAIN",
        );
        assert_eq!(
            lexical.expects,
            Expectation::ReachesIndex("idx_chunk_body".into()),
        );
        let vector = composed
            .iter()
            .find(|p| p.claim == "vector backing text_chunk.embedding")
            .expect("the vector backing is probed");
        // `[0]` whatever the index dimension: the planner resolves
        // the index before it looks at the literal's width.
        assert_eq!(
            vector.surql,
            "SELECT * FROM text_chunk WHERE embedding <|1,64|> [0] EXPLAIN",
        );
        assert_eq!(
            vector.expects,
            Expectation::ReachesIndex("idx_chunk_embedding".into()),
        );
        assert!(composed
            .iter()
            .all(|p| p.scope != "query search" || p.claim.contains("backing")));
    }

    /// An optional backing is probed the same way and judged
    /// differently.
    ///
    /// The statement is identical — the question is still whether that
    /// operator reaches that index — but the expectation carries the
    /// table as well as the index, because a plan that missed has two
    /// explanations for an optional backing and only one for a
    /// required one, and telling them apart means asking the database
    /// whether the index is there at all.
    #[test]
    fn an_optional_backing_is_probed_but_excused_when_absent() {
        let mut contract = contract();
        contract.queries = vec![crate::ir::Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![],
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
            searches: vec![SearchKind::Vector],
            backing: vec![crate::ir::SearchBacking {
                table: "text_chunk".into(),
                column: "embedding".into(),
                index: "idx_chunk_embedding".into(),
                kind: SearchKind::Vector,
                dimension: None,
                optional: true,
            }],
        }];
        let composed = probes(&contract);
        let probe = composed
            .iter()
            .find(|p| p.claim == "vector backing text_chunk.embedding")
            .expect("an optional backing is still probed");
        assert_eq!(
            probe.surql,
            "SELECT * FROM text_chunk WHERE embedding <|1,64|> [0] EXPLAIN",
        );
        assert_eq!(
            probe.expects,
            Expectation::ReachesIndexIfDefined {
                table: "text_chunk".into(),
                index: "idx_chunk_embedding".into(),
            },
        );
    }

    #[test]
    fn the_walker_finds_the_named_index_and_names_what_answered_instead() {
        // The exact FullTextScan shape the probe returned for a served
        // `@@` (abridged to what the walker reads).
        let served = serde_json::json!({
            "operator": "SelectProject",
            "children": [{
                "operator": "FullTextScan",
                "attributes": {"index": "idx_chunk_body", "query": "quick"},
            }],
        });
        assert!(reaches_index(&served, "idx_chunk_body"));
        // A different index serving is NOT the declared machinery.
        assert!(!reaches_index(&served, "idx_chunk_embedding"));
        assert_eq!(plan_summary(&served), "FullTextScan via idx_chunk_body");

        // The metric-form degradation: KnnTopK over TableScan. The
        // walk is the conviction, and the summary says so.
        let degraded = serde_json::json!({
            "operator": "SelectProject",
            "children": [{
                "operator": "KnnTopK",
                "attributes": {"dimension": "3", "distance": "Cosine",
                               "field": "embedding", "k": "4"},
                "children": [{
                    "operator": "TableScan",
                    "attributes": {"table": "text_chunk", "direction": "Forward"},
                }],
            }],
        });
        assert!(!reaches_index(&degraded, "idx_chunk_embedding"));
        assert_eq!(plan_summary(&degraded), "TableScan over text_chunk");
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

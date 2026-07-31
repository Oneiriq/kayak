//! The build-time gate: a contract only compiles against the schema it
//! actually has.
//!
//! Every violation is a generation failure, not a runtime surprise. The
//! two index rules encode the operational lesson this library exists to
//! enforce: an unindexed filter or sort ships fine, works in the demo,
//! and becomes a table scan (or a hard 5xx on multi-field ORDER BY) in
//! production.
//!
//! Rules:
//! - pinned column: must exist on the table (it is server-bound, so no
//!   index requirement of its own).
//! - filterable column: must appear in at least one index on the table.
//! - sortable column: some index must contain it at a position where
//!   every EARLIER column is pinned or filterable — an index serves an
//!   ORDER BY only from a prefix whose head is equality-bound. A bare
//!   leading column is the degenerate case.

use surql::schema::TableDefinition;

use crate::ir::{Contract, Resource};

/// A single contract-vs-schema violation, formatted for humans in
/// generator output.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Violation {
    #[error("resource {resource}: table {table} does not exist in the schema")]
    UnknownTable { resource: String, table: String },

    #[error("resource {resource}: column {column} does not exist on table {table}")]
    UnknownColumn {
        resource: String,
        table: String,
        column: String,
    },

    #[error("resource {resource}: exposes no fields")]
    NoFields { resource: String },

    #[error("resource {resource}: duplicate API field name {name} (rename collision)")]
    DuplicateApiName { resource: String, name: String },

    #[error("resource {resource}: action {action}: {problem}")]
    InvalidAction {
        resource: String,
        action: String,
        problem: String,
    },

    #[error(
        "resource {resource}: filterable column {column} is not covered by any \
         index on {table} — filtering on it would scan the table"
    )]
    UnindexedFilter {
        resource: String,
        table: String,
        column: String,
    },

    #[error(
        "resource {resource}: sortable column {column} is not reachable as an \
         index sort suffix on {table} — some index must hold it with every \
         earlier column pinned or filterable, or ORDER BY falls off the index"
    )]
    UnindexedSort {
        resource: String,
        table: String,
        column: String,
    },
}

/// Validate a contract against schema definitions; empty means valid.
pub fn validate(contract: &Contract, schema: &[TableDefinition]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for resource in &contract.resources {
        match schema.iter().find(|t| t.name == resource.table) {
            Some(table) => validate_resource(resource, table, &mut violations),
            None => violations.push(Violation::UnknownTable {
                resource: resource.name.clone(),
                table: resource.table.clone(),
            }),
        }
    }
    violations
}

fn validate_resource(
    resource: &Resource,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    let column_exists = |column: &str| table.fields.iter().any(|f| f.name == column);
    let push_unknown = |column: &str, violations: &mut Vec<Violation>| {
        violations.push(Violation::UnknownColumn {
            resource: resource.name.clone(),
            table: resource.table.clone(),
            column: column.to_owned(),
        });
    };

    if resource.fields.is_empty() {
        violations.push(Violation::NoFields {
            resource: resource.name.clone(),
        });
    }

    let mut seen_api_names = std::collections::BTreeSet::new();
    for exposure in &resource.fields {
        if !column_exists(&exposure.column) {
            push_unknown(&exposure.column, violations);
        }
        if !seen_api_names.insert(exposure.api_name().to_owned()) {
            violations.push(Violation::DuplicateApiName {
                resource: resource.name.clone(),
                name: exposure.api_name().to_owned(),
            });
        }
    }

    for column in &resource.filterable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let covered = table
            .indexes
            .iter()
            .any(|index| index.columns.iter().any(|c| c == column));
        if !covered {
            violations.push(Violation::UnindexedFilter {
                resource: resource.name.clone(),
                table: resource.table.clone(),
                column: column.clone(),
            });
        }
    }

    for column in &resource.pinned {
        if !column_exists(column) {
            push_unknown(column, violations);
        }
    }

    let mut action_names = std::collections::BTreeSet::new();
    for action in &resource.actions {
        let mut problem = |text: String| {
            violations.push(Violation::InvalidAction {
                resource: resource.name.clone(),
                action: action.name.clone(),
                problem: text,
            });
        };
        if action.name.is_empty() {
            problem("empty action name".into());
        }
        if !action_names.insert(action.name.clone()) {
            problem("duplicate action name".into());
        }
        if !matches!(action.method.as_str(), "POST" | "PUT" | "DELETE" | "PATCH") {
            problem(format!(
                "method must be POST, PUT, DELETE, or PATCH, not {:?}",
                action.method,
            ));
        }
        if !action.path.is_empty() && !action.path.starts_with('/') {
            problem(format!("path {:?} must start with '/'", action.path));
        }
        let mut input_names = std::collections::BTreeSet::new();
        for field in &action.input {
            if field.name.is_empty() {
                problem("empty input field name".into());
            }
            if !input_names.insert(field.name.clone()) {
                problem(format!("duplicate input field {:?}", field.name));
            }
        }
    }

    // A column earlier in an index than the sort column must be
    // equality-boundable, or the index cannot serve the ORDER BY.
    let boundable = |column: &str| {
        resource.pinned.iter().any(|c| c == column)
            || resource.filterable.iter().any(|c| c == column)
    };
    for column in &resource.sortable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let reachable = table.indexes.iter().any(|index| {
            index
                .columns
                .iter()
                .position(|c| c == column)
                .is_some_and(|k| index.columns[..k].iter().all(|earlier| boundable(earlier)))
        });
        if !reachable {
            violations.push(Violation::UnindexedSort {
                resource: resource.name.clone(),
                table: resource.table.clone(),
                column: column.clone(),
            });
        }
    }
}

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

    #[error("{scope}: name {name:?} {problem}")]
    InvalidName {
        scope: String,
        name: String,
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
        validate_names(resource, &mut violations);
        match schema.iter().find(|t| t.name == resource.table) {
            Some(table) => validate_resource(resource, table, &mut violations),
            None => violations.push(Violation::UnknownTable {
                resource: resource.name.clone(),
                table: resource.table.clone(),
            }),
        }
    }

    // GraphQL type names are schema-global; two resources landing on
    // the same effective type name would shadow each other.
    let mut type_names = std::collections::BTreeMap::new();
    for resource in &contract.resources {
        if let Some(previous) = type_names.insert(resource.graphql_type_name(), &resource.name) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: resource.graphql_type_name(),
                problem: format!("collides with the GraphQL type name of resource {previous}"),
            });
        }
    }
    violations
}

/// `[a-z][a-z0-9_]*`, with `-` also allowed when `kebab` is set.
fn is_wire_ident(name: &str, kebab: bool) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || (kebab && c == '-')
        })
}

/// The GraphQL `Name` grammar: `[_A-Za-z][_0-9A-Za-z]*`.
fn is_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Names the contract author CHOSE must be valid for every surface
/// they reach; names that OVERRIDE something — field renames and
/// GraphQL overrides — additionally must never collide with a
/// SurrealDB v3 reserved name. Action, input, and resource names are
/// exempt from the reserved gate (verbs like `remove` or `update` are
/// legitimate action names and never reach the database); column names
/// belong to the schema layer, which handles its own escaping.
fn validate_names(resource: &Resource, violations: &mut Vec<Violation>) {
    let mut push = |scope: String, name: &str, problem: String| {
        violations.push(Violation::InvalidName {
            scope,
            name: name.to_owned(),
            problem,
        });
    };
    let scope = |suffix: &str| format!("resource {}{suffix}", resource.name);

    if !is_wire_ident(&resource.name, true) {
        push(
            scope(""),
            &resource.name,
            "must be lowercase snake or kebab case".into(),
        );
    }

    for exposure in &resource.fields {
        if let Some(rename) = &exposure.rename {
            if !is_wire_ident(rename, false) {
                push(
                    scope(""),
                    rename,
                    "rename must be lowercase snake case".into(),
                );
            }
            if crate::reserved::is_reserved(rename) {
                push(
                    scope(""),
                    rename,
                    "rename collides with a SurrealDB v3 reserved name".into(),
                );
            }
        }
    }

    let graphql_overrides = resource.graphql.as_ref().map_or(Vec::new(), |g| {
        [
            ("graphql.type_name", g.type_name.as_deref()),
            ("graphql.list_field", g.list_field.as_deref()),
            ("graphql.get_field", g.get_field.as_deref()),
        ]
        .into_iter()
        .filter_map(|(label, name)| name.map(|n| (label, n)))
        .collect()
    });
    for (label, name) in graphql_overrides {
        if !is_graphql_name(name) {
            push(
                scope(&format!(" {label}")),
                name,
                "is not a valid GraphQL name".into(),
            );
        }
        if name.starts_with("__") {
            push(
                scope(&format!(" {label}")),
                name,
                "GraphQL reserves the __ prefix for introspection".into(),
            );
        }
        if matches!(name, "Query" | "Mutation" | "Subscription") {
            push(
                scope(&format!(" {label}")),
                name,
                "collides with a GraphQL root type name".into(),
            );
        }
        if crate::reserved::is_reserved(name) {
            push(
                scope(&format!(" {label}")),
                name,
                "collides with a SurrealDB v3 reserved name".into(),
            );
        }
    }

    for action in &resource.actions {
        let action_scope = || scope(&format!(" action {}", action.name));
        if !action.name.is_empty() && !is_wire_ident(&action.name, false) {
            push(
                action_scope(),
                &action.name,
                "must be lowercase snake case".into(),
            );
        }
        if let Some(field) = &action.graphql_field {
            if !is_graphql_name(field) || field.starts_with("__") {
                push(
                    action_scope(),
                    field,
                    "graphql_field is not a valid GraphQL name".into(),
                );
            }
            if crate::reserved::is_reserved(field) {
                push(
                    action_scope(),
                    field,
                    "graphql_field collides with a SurrealDB v3 reserved name".into(),
                );
            }
        }
        for input in &action.input {
            if !input.name.is_empty() && !is_wire_ident(&input.name, false) {
                push(
                    action_scope(),
                    &input.name,
                    "input name must be lowercase snake case".into(),
                );
            }
        }
    }
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

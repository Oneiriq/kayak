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
//!   every EARLIER column is pinned or filterable; an index serves an
//!   ORDER BY only from a prefix whose head is equality-bound. A bare
//!   leading column is the degenerate case.
//!
//! Both index rules read [`crate::indexes`] rather than the table's
//! index list, because only a standard or unique index counts toward
//! either. A FULLTEXT or vector index covers a column without narrowing
//! an equality on it or ordering by it, and a claim resting on one is
//! the failure this file exists to catch wearing the disguise of the
//! thing that would have caught it.

use surql::schema::{IndexDefinition, IndexType, TableDefinition};

use crate::indexes::{ordering_indexes, serves_ordering};
use crate::ir::{Contract, FieldExposure, Resource, SubResource};

/// One listing surface's index-relevant claims, so a resource and a
/// sub-resource are checked by exactly one rulebook.
struct Listing<'a> {
    /// How the surface names itself in a violation.
    scope: String,
    table: &'a str,
    fields: &'a [FieldExposure],
    /// Columns the server equality-binds; credited to sort prefixes.
    bound: Vec<&'a str>,
    filterable: &'a [String],
    sortable: &'a [String],
}

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
         index on {table}; filtering on it would scan the table"
    )]
    UnindexedFilter {
        resource: String,
        table: String,
        column: String,
    },

    #[error(
        "resource {resource}: sortable column {column} is not reachable as an \
         index sort suffix on {table}; some index must hold it with every \
         earlier column pinned or filterable, or ORDER BY falls off the index"
    )]
    UnindexedSort {
        resource: String,
        table: String,
        column: String,
    },

    #[error(
        "resource {resource}: {claim} column {column} is indexed on {table}, but \
         only by the {index_type} index {index}, which serves neither an equality \
         filter nor an ORDER BY; add a standard index holding {column}, or drop \
         the claim"
    )]
    WrongIndexType {
        resource: String,
        table: String,
        column: String,
        /// `filterable` or `sortable`: which claim the index fails to
        /// answer, so one sentence serves both.
        claim: String,
        index: String,
        index_type: IndexType,
    },
}

/// Validate a contract against schema definitions; empty means valid.
pub fn validate(contract: &Contract, schema: &[TableDefinition]) -> Vec<Violation> {
    let mut violations = Vec::new();
    // Two resources under one name would claim the same REST prefix
    // and the same GraphQL field, and the one written second would
    // win without saying so.
    let mut resource_names = std::collections::BTreeSet::new();
    for resource in &contract.resources {
        if !resource_names.insert(resource.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: resource.name.clone(),
                problem: "duplicate resource".into(),
            });
        }
        validate_names(resource, &mut violations);
        validate_filter_options(resource, &mut violations);
        match schema.iter().find(|t| t.name == resource.table) {
            Some(table) => validate_resource(resource, table, &mut violations),
            None => violations.push(Violation::UnknownTable {
                resource: resource.name.clone(),
                table: resource.table.clone(),
            }),
        }
    }

    // Rate classes: names sound and unique, references resolvable. A
    // reference to a class the contract never defines would meter
    // against a budget nobody wrote down.
    let mut class_names = std::collections::BTreeSet::new();
    for class in &contract.rate_classes {
        if !is_wire_ident(&class.name, false) {
            violations.push(Violation::InvalidName {
                scope: format!("rate class {}", class.name),
                name: class.name.clone(),
                problem: "must be lowercase snake case".into(),
            });
        }
        if !class_names.insert(class.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("rate class {}", class.name),
                name: class.name.clone(),
                problem: "duplicate rate class".into(),
            });
        }
    }
    let class_exists = |name: &str| contract.rate_classes.iter().any(|c| c.name == name);
    for resource in &contract.resources {
        if let Some(class) = &resource.rate_class {
            if !class_exists(class) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}", resource.name),
                    name: class.clone(),
                    problem: "references an undefined rate class".into(),
                });
            }
        }
        for action in &resource.actions {
            if let Some(class) = &action.rate_class {
                if !class_exists(class) {
                    violations.push(Violation::InvalidName {
                        scope: format!("resource {} action {}", resource.name, action.name),
                        name: class.clone(),
                        problem: "references an undefined rate class".into(),
                    });
                }
            }
        }
    }

    // GraphQL type names are schema-global; two surfaces landing on
    // the same effective type name would shadow each other.
    let mut type_names = std::collections::BTreeMap::new();
    for resource in &contract.resources {
        if let Some(previous) =
            type_names.insert(resource.graphql_type_name(), resource.name.clone())
        {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: resource.graphql_type_name(),
                problem: format!("collides with the GraphQL type name of resource {previous}"),
            });
        }
        let mut sub_names = std::collections::BTreeSet::new();
        for sub in &resource.sub_resources {
            validate_sub_resource(resource, sub, schema, &mut violations);
            if !sub_names.insert(sub.name.clone()) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}", resource.name),
                    name: sub.name.clone(),
                    problem: "duplicate sub-resource name".into(),
                });
            }
            let owner = format!("{}.{}", resource.name, sub.name);
            if let Some(previous) = type_names.insert(sub.graphql_type_name(resource), owner) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}.{}", resource.name, sub.name),
                    name: sub.graphql_type_name(resource),
                    problem: format!("collides with the GraphQL type name of {previous}"),
                });
            }
        }
    }
    // Queries: names sound and unique, rate classes resolvable, and
    // paths absolute. A query is a wire surface like any other, so it
    // answers to the same naming rules.
    let mut query_names = std::collections::BTreeSet::new();
    for query in &contract.queries {
        if !is_wire_ident(&query.name, false) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.name.clone(),
                problem: "must be lowercase snake case".into(),
            });
        }
        if !query_names.insert(query.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.name.clone(),
                problem: "duplicate query".into(),
            });
        }
        if !query.path.starts_with('/') {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.path.clone(),
                problem: "path must be absolute".into(),
            });
        }
        if let Some(class) = &query.rate_class {
            if !class_exists(class) {
                violations.push(Violation::InvalidName {
                    scope: format!("query {}", query.name),
                    name: class.clone(),
                    problem: "references an undefined rate class".into(),
                });
            }
        }
        for field in &query.input {
            if !is_wire_ident(&field.name, false) {
                violations.push(Violation::InvalidName {
                    scope: format!("query {} input", query.name),
                    name: field.name.clone(),
                    problem: "must be lowercase snake case".into(),
                });
            }
        }
    }

    violations
}

/// The index an author most likely mistook for coverage: one that holds
/// `column` and cannot order it, reported only when nothing that CAN
/// order it holds it at all.
///
/// The distinction matters because the two mistakes want different
/// repairs. Where a standard index does hold the column, the index type
/// is not what went wrong, and saying it was would send the author off
/// to define an index they already have; the prefix is the problem, and
/// [`Violation::UnindexedSort`] already explains prefixes.
fn misleading_index<'a>(table: &'a TableDefinition, column: &str) -> Option<&'a IndexDefinition> {
    let holds = |index: &IndexDefinition| index.columns.iter().any(|c| c == column);
    if table
        .indexes
        .iter()
        .any(|index| serves_ordering(index) && holds(index))
    {
        return None;
    }
    table.indexes.iter().find(|index| holds(index))
}

/// The index rulebook, applied identically to a resource and to a
/// sub-resource. Only the scope string and the set of server-bound
/// columns differ between them.
fn validate_listing(
    listing: &Listing<'_>,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    let column_exists = |column: &str| table.fields.iter().any(|f| f.name == column);
    let push_unknown = |column: &str, violations: &mut Vec<Violation>| {
        violations.push(Violation::UnknownColumn {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.to_owned(),
        });
    };

    if listing.fields.is_empty() {
        violations.push(Violation::NoFields {
            resource: listing.scope.clone(),
        });
    }

    let mut seen_api_names = std::collections::BTreeSet::new();
    for exposure in listing.fields {
        if !column_exists(&exposure.column) {
            push_unknown(&exposure.column, violations);
        }
        if !seen_api_names.insert(exposure.api_name().to_owned()) {
            violations.push(Violation::DuplicateApiName {
                resource: listing.scope.clone(),
                name: exposure.api_name().to_owned(),
            });
        }
    }

    let ordering = ordering_indexes(table);
    // An author who reached a claim through an index that turns out not
    // to serve it is in a different position from one who claimed
    // against nothing: they looked, they found something, and they were
    // right about everything except the one property that mattered. The
    // violation says which index they were looking at.
    let wrong_index = |column: &String, claim: &str, violations: &mut Vec<Violation>| {
        let Some(index) = misleading_index(table, column) else {
            return false;
        };
        violations.push(Violation::WrongIndexType {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
            claim: claim.to_owned(),
            index: index.name.clone(),
            index_type: index.index_type,
        });
        true
    };

    for column in listing.filterable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let covered = ordering
            .iter()
            .any(|index| index.columns.iter().any(|c| c == column));
        if covered {
            continue;
        }
        if wrong_index(column, "filterable", violations) {
            continue;
        }
        violations.push(Violation::UnindexedFilter {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
        });
    }

    for column in &listing.bound {
        if !column_exists(column) {
            push_unknown(column, violations);
        }
    }

    // A column earlier in an index than the sort column must be
    // equality-boundable, or the index cannot serve the ORDER BY.
    let boundable = |column: &str| {
        listing.bound.contains(&column) || listing.filterable.iter().any(|c| c == column)
    };
    for column in listing.sortable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let reachable = ordering.iter().any(|index| {
            index
                .columns
                .iter()
                .position(|c| c == column)
                .is_some_and(|k| index.columns[..k].iter().all(|earlier| boundable(earlier)))
        });
        if reachable {
            continue;
        }
        if wrong_index(column, "sortable", violations) {
            continue;
        }
        violations.push(Violation::UnindexedSort {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
        });
    }
}

/// Sub-resources: the same rulebook over the sub table, with
/// `parent_key` credited as server-bound, plus the name checks.
fn validate_sub_resource(
    parent: &Resource,
    sub: &SubResource,
    schema: &[TableDefinition],
    violations: &mut Vec<Violation>,
) {
    let scope = format!("{}.{}", parent.name, sub.name);
    if !is_wire_ident(&sub.name, true) {
        violations.push(Violation::InvalidName {
            scope: format!("resource {scope}"),
            name: sub.name.clone(),
            problem: "must be lowercase snake or kebab case".into(),
        });
    }
    for (label, name) in [
        (
            "graphql.type_name",
            sub.graphql.as_ref().and_then(|g| g.type_name.as_deref()),
        ),
        (
            "graphql.field",
            sub.graphql.as_ref().and_then(|g| g.field.as_deref()),
        ),
    ] {
        let Some(name) = name else { continue };
        if !is_graphql_name(name) || name.starts_with("__") {
            violations.push(Violation::InvalidName {
                scope: format!("resource {scope} {label}"),
                name: name.to_owned(),
                problem: "is not a valid GraphQL name".into(),
            });
        }
        if crate::reserved::is_reserved(name) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {scope} {label}"),
                name: name.to_owned(),
                problem: "collides with a SurrealDB v3 reserved name".into(),
            });
        }
    }

    let Some(table) = schema.iter().find(|t| t.name == sub.table) else {
        violations.push(Violation::UnknownTable {
            resource: scope,
            table: sub.table.clone(),
        });
        return;
    };
    validate_listing(
        &Listing {
            scope,
            table: &sub.table,
            fields: &sub.fields,
            bound: sub.bound_columns(),
            filterable: &sub.filterable,
            sortable: &sub.sortable,
        },
        table,
        violations,
    );
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
/// they reach; names that OVERRIDE something (field renames and
/// GraphQL overrides) additionally must never collide with a
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
        if let Some(guard) = &exposure.guard {
            if !is_wire_ident(guard, false) {
                push(
                    scope(""),
                    guard,
                    "guard names must be lowercase snake case".into(),
                );
            }
        }
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
            ("graphql.watch_field", g.watch_field.as_deref()),
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

    for required in &resource.reads_require {
        if !is_wire_ident(required, false) {
            push(
                scope(" reads_require"),
                required,
                "scope names must be lowercase snake case".into(),
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
        for required in &action.requires {
            if !is_wire_ident(required, false) {
                push(
                    action_scope(),
                    required,
                    "scope names must be lowercase snake case".into(),
                );
            }
        }
    }
}

/// Filter options describe columns a caller may narrow by, so a key
/// that is not filterable describes nothing and would render a menu
/// beside a filter the server refuses.
fn validate_filter_options(resource: &Resource, violations: &mut Vec<Violation>) {
    for (column, options) in &resource.filter_options {
        if !resource.filterable.iter().any(|f| f == column) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: column.clone(),
                problem: "filter options name a column that is not filterable".into(),
            });
        }
        if options.is_empty() {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: column.clone(),
                problem: "filter options are empty, which offers a menu of nothing".into(),
            });
        }
    }
}

fn validate_resource(
    resource: &Resource,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    validate_listing(
        &Listing {
            scope: resource.name.clone(),
            table: &resource.table,
            fields: &resource.fields,
            bound: resource.pinned.iter().map(String::as_str).collect(),
            filterable: &resource.filterable,
            sortable: &resource.sortable,
        },
        table,
        violations,
    );

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
            // A closed set of one repeated value refuses inputs by
            // accident, and a set on a field whose type cannot hold a
            // string never matches anything.
            let mut seen = std::collections::BTreeSet::new();
            for option in &field.options {
                if !seen.insert(option) {
                    problem(format!(
                        "input {:?} lists option {option:?} twice",
                        field.name,
                    ));
                }
            }
            if field.multiple && field.options.is_empty() {
                problem(format!(
                    "input {:?} takes several values but lists none",
                    field.name,
                ));
            }
            if !field.options.is_empty() && field.kind != crate::ir::TypeRef::String {
                problem(format!(
                    "input {:?} lists options but is not a string",
                    field.name,
                ));
            }
        }
    }
}

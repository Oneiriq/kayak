//! IR-level breaking-change detection.
//!
//! Diffing at the IR level catches changes the rendered documents
//! hide: a filter removed from an allowlist, a sort claim
//! dropped, an action's method changed, a required input added. The
//! rule of thumb: anything a deployed client could be relying on is
//! breaking; pure additions are compatible.

use crate::ir::{Action, Contract, Resource};

/// One observed change between two contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A deployed client could break.
    Breaking(String),
    /// Additive or otherwise safe.
    Compatible(String),
}

impl Change {
    pub fn is_breaking(&self) -> bool {
        matches!(self, Self::Breaking(_))
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Breaking(m) | Self::Compatible(m) => m,
        }
    }
}

/// Compare `old` to `new`, returning every observed change.
pub fn diff(old: &Contract, new: &Contract) -> Vec<Change> {
    let mut changes = Vec::new();

    // Ceilings: introducing or lowering one refuses operations that
    // used to run. Raising or removing one refuses nothing.
    let old_limits = old.limits.unwrap_or_default();
    let new_limits = new.limits.unwrap_or_default();
    for (label, before, after) in [
        ("max_depth", old_limits.max_depth, new_limits.max_depth),
        (
            "max_complexity",
            old_limits.max_complexity,
            new_limits.max_complexity,
        ),
    ] {
        match (before, after) {
            (None, Some(introduced)) => changes.push(Change::Breaking(format!(
                "limits: {label} introduced at {introduced}",
            ))),
            (Some(b), Some(a)) if a < b => changes.push(Change::Breaking(format!(
                "limits: {label} lowered {b} -> {a}",
            ))),
            (Some(b), Some(a)) if a > b => changes.push(Change::Compatible(format!(
                "limits: {label} raised {b} -> {a}",
            ))),
            (Some(removed), None) => changes.push(Change::Compatible(format!(
                "limits: {label} removed (was {removed})",
            ))),
            _ => {}
        }
    }

    for old_resource in &old.resources {
        match new.resources.iter().find(|r| r.name == old_resource.name) {
            None => changes.push(Change::Breaking(format!(
                "resource {} removed",
                old_resource.name,
            ))),
            Some(new_resource) => diff_resource(old_resource, new_resource, &mut changes),
        }
    }
    for new_resource in &new.resources {
        if !old.resources.iter().any(|r| r.name == new_resource.name) {
            changes.push(Change::Compatible(format!(
                "resource {} added",
                new_resource.name,
            )));
        }
    }
    changes
}

fn diff_resource(old: &Resource, new: &Resource, changes: &mut Vec<Change>) {
    let scope = &old.name;

    for exposure in &old.fields {
        let api = exposure.api_name();
        match new.fields.iter().find(|f| f.api_name() == api) {
            None => changes.push(Change::Breaking(format!("{scope}: field {api} removed",))),
            Some(current) if current.column != exposure.column => {
                // Same wire name over a different column: the value's
                // meaning (and possibly type) changed under the client.
                changes.push(Change::Breaking(format!(
                    "{scope}: field {api} now reads column {} (was {})",
                    current.column, exposure.column,
                )));
            }
            Some(_) => {}
        }
    }
    for exposure in &new.fields {
        if !old
            .fields
            .iter()
            .any(|f| f.api_name() == exposure.api_name())
        {
            changes.push(Change::Compatible(format!(
                "{scope}: field {} added",
                exposure.api_name(),
            )));
        }
    }

    for column in &old.filterable {
        if !new.filterable.contains(column) {
            changes.push(Change::Breaking(format!(
                "{scope}: filter {column} removed",
            )));
        }
    }
    for column in &old.sortable {
        if !new.sortable.contains(column) {
            changes.push(Change::Breaking(format!("{scope}: sort {column} removed",)));
        }
    }
    if new.max_page_size < old.max_page_size {
        changes.push(Change::Breaking(format!(
            "{scope}: max_page_size lowered {} -> {}",
            old.max_page_size, new.max_page_size,
        )));
    }

    // GraphQL names are part of a deployed schema's identity: fragments
    // name types, queries name fields. Effective names are compared so
    // an override equal to the derived default is a no-op.
    for (label, before, after) in [
        ("type", old.graphql_type_name(), new.graphql_type_name()),
        (
            "list field",
            old.graphql_list_field(),
            new.graphql_list_field(),
        ),
        (
            "get field",
            old.graphql_get_field(),
            new.graphql_get_field(),
        ),
    ] {
        if before != after {
            changes.push(Change::Breaking(format!(
                "{scope}: graphql {label} renamed {before} -> {after}",
            )));
        }
    }

    // Sub-collections: adding one is additive, removing one takes a
    // path and a GraphQL field away from deployed clients.
    for old_sub in &old.sub_resources {
        match new.sub_resources.iter().find(|s| s.name == old_sub.name) {
            None => changes.push(Change::Breaking(format!(
                "{scope}: sub-resource {} removed",
                old_sub.name,
            ))),
            Some(new_sub) => {
                for exposure in &old_sub.fields {
                    let api = exposure.api_name();
                    if !new_sub.fields.iter().any(|f| f.api_name() == api) {
                        changes.push(Change::Breaking(format!(
                            "{scope}.{}: field {api} removed",
                            old_sub.name,
                        )));
                    }
                }
                for column in &old_sub.filterable {
                    if !new_sub.filterable.contains(column) {
                        changes.push(Change::Breaking(format!(
                            "{scope}.{}: filter {column} removed",
                            old_sub.name,
                        )));
                    }
                }
                for column in &old_sub.sortable {
                    if !new_sub.sortable.contains(column) {
                        changes.push(Change::Breaking(format!(
                            "{scope}.{}: sort {column} removed",
                            old_sub.name,
                        )));
                    }
                }
                if new_sub.max_page_size < old_sub.max_page_size {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: max_page_size lowered {} -> {}",
                        old_sub.name, old_sub.max_page_size, new_sub.max_page_size,
                    )));
                }
                let before = old_sub.graphql_field();
                let after = new_sub.graphql_field();
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: graphql field renamed {before} -> {after}",
                        old_sub.name,
                    )));
                }
                let before = old_sub.graphql_type_name(old);
                let after = new_sub.graphql_type_name(new);
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: graphql type renamed {before} -> {after}",
                        old_sub.name,
                    )));
                }
            }
        }
    }
    for new_sub in &new.sub_resources {
        if !old.sub_resources.iter().any(|s| s.name == new_sub.name) {
            changes.push(Change::Compatible(format!(
                "{scope}: sub-resource {} added",
                new_sub.name,
            )));
        }
    }

    // Scopes: a new requirement refuses callers that used to pass.
    for required in &new.reads_require {
        if !old.reads_require.contains(required) {
            changes.push(Change::Breaking(format!(
                "{scope}: reads now require scope {required}",
            )));
        }
    }
    for required in &old.reads_require {
        if !new.reads_require.contains(required) {
            changes.push(Change::Compatible(format!(
                "{scope}: reads no longer require scope {required}",
            )));
        }
    }

    // Watching: opening one is additive, closing one strands every
    // deployed subscriber, and the field name matters only while the
    // subscription exists.
    match (old.watchable, new.watchable) {
        (false, true) => changes.push(Change::Compatible(format!("{scope}: became watchable"))),
        (true, false) => changes.push(Change::Breaking(format!("{scope}: no longer watchable",))),
        (true, true) => {
            let before = old.graphql_watch_field();
            let after = new.graphql_watch_field();
            if before != after {
                changes.push(Change::Breaking(format!(
                    "{scope}: graphql watch field renamed {before} -> {after}",
                )));
            }
        }
        (false, false) => {}
    }

    for old_action in &old.actions {
        match new.actions.iter().find(|a| a.name == old_action.name) {
            None => changes.push(Change::Breaking(format!(
                "{scope}: action {} removed",
                old_action.name,
            ))),
            Some(new_action) => {
                let before = old_action.graphql_field_name(old);
                let after = new_action.graphql_field_name(new);
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {} graphql field renamed {before} -> {after}",
                        old_action.name,
                    )));
                }
                diff_action(scope, old_action, new_action, changes);
            }
        }
    }
    for new_action in &new.actions {
        if !old.actions.iter().any(|a| a.name == new_action.name) {
            changes.push(Change::Compatible(format!(
                "{scope}: action {} added",
                new_action.name,
            )));
        }
    }
}

fn diff_action(scope: &str, old: &Action, new: &Action, changes: &mut Vec<Change>) {
    let name = &old.name;
    for required in &new.requires {
        if !old.requires.contains(required) {
            changes.push(Change::Breaking(format!(
                "{scope}: action {name} now requires scope {required}",
            )));
        }
    }
    for required in &old.requires {
        if !new.requires.contains(required) {
            changes.push(Change::Compatible(format!(
                "{scope}: action {name} no longer requires scope {required}",
            )));
        }
    }
    if old.method != new.method || old.path != new.path {
        changes.push(Change::Breaking(format!(
            "{scope}: action {name} moved ({} {} -> {} {})",
            old.method, old.path, new.method, new.path,
        )));
    }
    if old.output != new.output {
        changes.push(Change::Breaking(format!(
            "{scope}: action {name} output changed",
        )));
    }
    for field in &new.input {
        let previous = old.input.iter().find(|f| f.name == field.name);
        match previous {
            None if field.required => changes.push(Change::Breaking(format!(
                "{scope}: action {name} gained required input {}",
                field.name,
            ))),
            None => changes.push(Change::Compatible(format!(
                "{scope}: action {name} gained optional input {}",
                field.name,
            ))),
            Some(previous) => {
                if field.required && !previous.required {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {name} input {} became required",
                        field.name,
                    )));
                }
                if field.kind != previous.kind {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {name} input {} changed type",
                        field.name,
                    )));
                }
            }
        }
    }
    for field in &old.input {
        if !new.input.iter().any(|f| f.name == field.name) {
            // Removing an input a client may still send: servers should
            // ignore unknown fields, so this is compatible-with-note.
            changes.push(Change::Compatible(format!(
                "{scope}: action {name} input {} removed",
                field.name,
            )));
        }
    }
}

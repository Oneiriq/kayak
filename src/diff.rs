//! IR-level breaking-change detection.
//!
//! Diffing the IR — not the generated SDL or OpenAPI — catches changes
//! documents hide: a filter removed from an allowlist, a sort claim
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

    for old_action in &old.actions {
        match new.actions.iter().find(|a| a.name == old_action.name) {
            None => changes.push(Change::Breaking(format!(
                "{scope}: action {} removed",
                old_action.name,
            ))),
            Some(new_action) => diff_action(scope, old_action, new_action, changes),
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

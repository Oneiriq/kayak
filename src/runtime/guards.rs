//! Field guards: per-caller visibility, decided over the context.
//!
//! A guard is a named predicate the service registers. The contract
//! references it from a [`FieldExposure`](crate::ir::FieldExposure);
//! the dispatcher applies it as projection on every row a resolver
//! returns, on every face, so a guarded value cannot leave the
//! process through a forgotten path.
//!
//! Guards are synchronous on purpose. They run per field per row, so
//! a guard that performed IO would turn one listing into hundreds of
//! queries; a guard reads the context (the principal, whatever the
//! middleware seeded) and answers. Anything that needs IO belongs in
//! an auth middleware that resolves ONCE into the context, where a
//! guard can then see it.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ir::{Contract, FieldExposure};
use crate::runtime::context::JanusContext;

/// One visibility decision: does this caller see this field?
pub(crate) type FieldGuard = Arc<dyn Fn(&JanusContext) -> bool + Send + Sync>;

/// Registered guards, keyed by the name the contract references.
#[derive(Default, Clone)]
pub struct Guards {
    pub(crate) map: BTreeMap<String, FieldGuard>,
}

impl Guards {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the guard for `name`. Returning `true` shows the
    /// field; `false` omits it from the row.
    pub fn guard<F>(mut self, name: &str, decide: F) -> Self
    where
        F: Fn(&JanusContext) -> bool + Send + Sync + 'static,
    {
        self.map.insert(name.to_owned(), Arc::new(decide));
        self
    }
}

impl std::fmt::Debug for Guards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guards")
            .field("names", &self.map.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// One field the current caller may not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenField {
    /// The wire name a row carries, which projection removes.
    pub api_name: String,
    /// The backing column, which filter and sort refusals compare.
    pub column: String,
}

/// The fields in `fields` whose guards deny this caller. The
/// dispatcher computes this for every operation it runs; a
/// hand-written face calls it so both faces redact from the same
/// declarations instead of drifting apart.
pub fn hidden_in(
    fields: &[FieldExposure],
    guards: &Guards,
    ctx: &JanusContext,
) -> Vec<HiddenField> {
    fields
        .iter()
        .filter_map(|exposure| {
            let guard = exposure.guard.as_deref()?;
            let decide = guards.map.get(guard)?;
            if decide(ctx) {
                None
            } else {
                Some(HiddenField {
                    api_name: exposure.api_name().to_owned(),
                    column: exposure.column.clone(),
                })
            }
        })
        .collect()
}

/// [`hidden_in`] over a contract resource, or one of its
/// sub-collections when `sub` names one. Unknown names hide nothing,
/// matching the dispatcher: a name the contract does not know cannot
/// have declared guards.
pub fn hidden_fields(
    contract: &Contract,
    resource: &str,
    sub: Option<&str>,
    guards: &Guards,
    ctx: &JanusContext,
) -> Vec<HiddenField> {
    let Some(resource) = contract.resources.iter().find(|r| r.name == resource) else {
        return Vec::new();
    };
    let fields = match sub {
        Some(name) => resource
            .sub_resources
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.fields.as_slice())
            .unwrap_or(&[]),
        None => &resource.fields,
    };
    hidden_in(fields, guards, ctx)
}

/// Remove every hidden field from one wire row, in place. The row
/// omits the keys rather than nulling them, exactly as the dispatcher
/// projects.
pub fn strip_hidden(row: &mut serde_json::Value, hidden: &[HiddenField]) {
    if let Some(object) = row.as_object_mut() {
        for field in hidden {
            object.remove(&field.api_name);
        }
    }
}

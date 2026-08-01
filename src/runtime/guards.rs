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

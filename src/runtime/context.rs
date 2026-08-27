//! Per-request context: typed values flowing from the protocol layer
//! through middleware into resolvers.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A typed extension map. The protocol layer seeds it (a tenant header,
/// a bearer principal), middleware may add to it (an authenticated
/// identity), and resolvers read it. Values are `Arc`-shared, so
/// cloning the context is cheap and clones observe the same values.
#[derive(Default, Clone)]
pub struct KayakContext {
    values: BTreeMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl KayakContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store one value of type `T`, replacing any previous `T`.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> &mut Self {
        self.values.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }

    /// Builder-style [`KayakContext::insert`].
    pub fn with<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.insert(value);
        self
    }

    /// Read the stored `T`, if any.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.values
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
    }
}

impl std::fmt::Debug for KayakContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KayakContext")
            .field("values", &self.values.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Tenant(String);

    #[test]
    fn typed_values_round_trip_and_clones_share() {
        let mut ctx = KayakContext::new();
        ctx.insert(Tenant("acme".into()));
        let cloned = ctx.clone();
        assert_eq!(cloned.get::<Tenant>(), Some(&Tenant("acme".into())));
        assert!(cloned.get::<u32>().is_none());
    }
}

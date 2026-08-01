//! The principal: who is acting, below the tenant.
//!
//! A tenant says whose data a request touches. A principal says what
//! this particular caller may do with it: an API key with a read
//! scope, a service account with write, an operator with admin. The
//! protocol layer or an auth middleware seeds one into the
//! [`JanusContext`](crate::runtime::JanusContext); the dispatcher
//! checks it against the scopes the contract declares, after the
//! middleware chain and before the resolver, so an auth layer that
//! resolves identity mid-chain still counts.
//!
//! Absence fails closed. A resource that requires a scope refuses an
//! anonymous request with `Unauthorized`; an identified caller
//! missing the scope refuses with `Forbidden`, naming the scope. A
//! contract that declares no scopes checks nothing, which keeps
//! existing deployments exactly as open as they were.

use std::collections::BTreeSet;

/// An identified caller and what it is allowed to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Who this is, for audit trails: a key id, a service name. Never
    /// interpreted by the dispatcher.
    pub subject: String,
    /// The scopes this caller holds. Checked as exact strings; the
    /// contract's vocabulary is the service's to choose.
    pub scopes: BTreeSet<String>,
}

impl Principal {
    /// A principal holding the given scopes.
    pub fn new(subject: impl Into<String>, scopes: impl IntoIterator<Item = String>) -> Self {
        Self {
            subject: subject.into(),
            scopes: scopes.into_iter().collect(),
        }
    }

    /// Whether this caller holds `scope`.
    pub fn has(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }
}

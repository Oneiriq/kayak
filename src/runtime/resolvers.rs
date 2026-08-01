//! The resolver registry: where a service plugs its own data access in.
//!
//! Janus never talks to a database. A resolver is any async closure the
//! service registers (over surql-rs repositories or another
//! service), and the dispatcher guarantees it only ever sees
//! contract-validated arguments.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;

use crate::runtime::args::{ActionArgs, GetArgs, ListArgs, ListOutput, WatchArgs};
use crate::runtime::context::JanusContext;
use crate::runtime::error::JanusError;

/// The boxed future every resolver and middleware returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a watch resolver returns: rows in the same wire shape list and
/// get produce, arriving until the subscriber drops the stream. An
/// `Err` item ends the subscription with that error; a resolver that
/// loses its source should yield one rather than closing silently.
pub type RowStream = Pin<Box<dyn Stream<Item = Result<serde_json::Value, JanusError>> + Send>>;

pub(crate) type ListResolver = Arc<
    dyn Fn(JanusContext, ListArgs) -> BoxFuture<'static, Result<ListOutput, JanusError>>
        + Send
        + Sync,
>;
pub(crate) type GetResolver = Arc<
    dyn Fn(
            JanusContext,
            GetArgs,
        ) -> BoxFuture<'static, Result<Option<serde_json::Value>, JanusError>>
        + Send
        + Sync,
>;
pub(crate) type ActionResolver = Arc<
    dyn Fn(
            JanusContext,
            ActionArgs,
        ) -> BoxFuture<'static, Result<Option<serde_json::Value>, JanusError>>
        + Send
        + Sync,
>;

pub(crate) type WatchResolver = Arc<
    dyn Fn(JanusContext, WatchArgs) -> BoxFuture<'static, Result<RowStream, JanusError>>
        + Send
        + Sync,
>;

/// Registered resolvers, keyed by resource (and action) name.
#[derive(Default, Clone)]
pub struct Resolvers {
    pub(crate) list: BTreeMap<String, ListResolver>,
    pub(crate) get: BTreeMap<String, GetResolver>,
    pub(crate) action: BTreeMap<(String, String), ActionResolver>,
    pub(crate) watch: BTreeMap<String, WatchResolver>,
}

impl Resolvers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the list resolver for `resource`. The closure receives
    /// clamped, allowlist-checked [`ListArgs`] and returns one page in
    /// wire shape.
    pub fn list<F, Fut>(mut self, resource: &str, f: F) -> Self
    where
        F: Fn(JanusContext, ListArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ListOutput, JanusError>> + Send + 'static,
    {
        self.list.insert(
            resource.to_owned(),
            Arc::new(move |ctx, args| Box::pin(f(ctx, args))),
        );
        self
    }

    /// Register the get resolver for `resource`. Returning `Ok(None)`
    /// means not found (protocols render null / 404).
    pub fn get<F, Fut>(mut self, resource: &str, f: F) -> Self
    where
        F: Fn(JanusContext, GetArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<serde_json::Value>, JanusError>> + Send + 'static,
    {
        self.get.insert(
            resource.to_owned(),
            Arc::new(move |ctx, args| Box::pin(f(ctx, args))),
        );
        self
    }

    /// Register the resolver for one action on `resource`. The closure
    /// receives type-checked inputs; its `Ok` value must match the
    /// action's declared output (`Some` row for `Resource`, `Some`
    /// value for `Json`, `None` for `None`).
    pub fn action<F, Fut>(mut self, resource: &str, action: &str, f: F) -> Self
    where
        F: Fn(JanusContext, ActionArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<serde_json::Value>, JanusError>> + Send + 'static,
    {
        self.action.insert(
            (resource.to_owned(), action.to_owned()),
            Arc::new(move |ctx, args| Box::pin(f(ctx, args))),
        );
        self
    }

    /// Register the watch resolver for `resource`, required of every
    /// resource the contract marks watchable.
    ///
    /// The closure is awaited once, when a subscription opens, and the
    /// stream it returns runs until the subscriber drops it. The
    /// middleware chain runs around that opening call only: a
    /// subscription is authorized when it starts, so a resolver whose
    /// stream must stop on a revoked credential has to enforce that
    /// itself, per row.
    pub fn watch<F, Fut>(mut self, resource: &str, f: F) -> Self
    where
        F: Fn(JanusContext, WatchArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<RowStream, JanusError>> + Send + 'static,
    {
        self.watch.insert(
            resource.to_owned(),
            Arc::new(move |ctx, args| Box::pin(f(ctx, args))),
        );
        self
    }
}

impl std::fmt::Debug for Resolvers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolvers")
            .field("list", &self.list.keys().collect::<Vec<_>>())
            .field("get", &self.get.keys().collect::<Vec<_>>())
            .field("action", &self.action.keys().collect::<Vec<_>>())
            .field("watch", &self.watch.keys().collect::<Vec<_>>())
            .finish()
    }
}

//! The middleware chain: protocol-agnostic policy around every
//! operation.
//!
//! Middleware wraps dispatch itself, so the same chain runs whether an
//! operation arrived over GraphQL or REST, which is the point: auth,
//! auditing, and metrics are written once. Each layer receives the
//! operation identity, the context, the validated payload, and a
//! [`Next`]; it can short-circuit (return an error), enrich the context
//! (insert an authenticated principal), observe, or transform the
//! outcome on the way back out.

use std::sync::Arc;

use crate::runtime::args::{
    ActionArgs, GetArgs, ListArgs, ListOutput, QueryArgs, SubListArgs, WatchArgs,
};
use crate::runtime::context::JanusContext;
use crate::runtime::error::JanusError;
use crate::runtime::resolvers::{BoxFuture, RowStream};

/// Which kind of operation is being dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    List,
    Get,
    Action,
    /// Listing a collection that hangs off one parent instance.
    SubList,
    /// Opening a subscription. The chain runs once, at open; the rows
    /// that follow do not pass through it.
    Watch,
    /// A contract query: a read that answers a question rather than
    /// paging a collection. It names no resource, so scope and rate
    /// declarations come from the query itself.
    Query,
}

/// The identity of one dispatched operation.
#[derive(Debug, Clone)]
pub struct Operation {
    /// The resource this operation belongs to, or the query name when
    /// `kind` is [`OperationKind::Query`], which belongs to none.
    pub resource: String,
    pub kind: OperationKind,
    /// Set when `kind` is [`OperationKind::Action`].
    pub action: Option<String>,
    /// Set when `kind` is [`OperationKind::SubList`]: which collection
    /// of the parent is being listed.
    pub sub: Option<String>,
}

/// The validated arguments travelling through the chain.
#[derive(Debug, Clone)]
pub enum Payload {
    List(ListArgs),
    Get(GetArgs),
    Action(ActionArgs),
    SubList(SubListArgs),
    Watch(WatchArgs),
    /// A contract query's parameters.
    Query(QueryArgs),
}

/// What came back from the resolver.
pub enum Outcome {
    List(ListOutput),
    Get(Option<serde_json::Value>),
    Action(Option<serde_json::Value>),
    /// The opened stream. A middleware may replace it (to meter or
    /// bound it) but cannot read it without consuming rows.
    Watch(RowStream),
    /// A contract query's answer.
    Query(serde_json::Value),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::List(output) => f.debug_tuple("List").field(output).finish(),
            Self::Get(row) => f.debug_tuple("Get").field(row).finish(),
            Self::Action(value) => f.debug_tuple("Action").field(value).finish(),
            Self::Query(value) => f.debug_tuple("Query").field(value).finish(),
            Self::Watch(_) => f.debug_tuple("Watch").field(&"<stream>").finish(),
        }
    }
}

/// One layer of the chain.
pub trait Middleware: Send + Sync {
    /// Handle the operation. Call `next.run(...)` to continue inward;
    /// skip it to short-circuit.
    fn handle<'a>(
        &'a self,
        operation: Operation,
        ctx: JanusContext,
        payload: Payload,
        next: Next,
    ) -> BoxFuture<'a, Result<Outcome, JanusError>>;
}

pub(crate) type Terminal = Arc<
    dyn Fn(Operation, JanusContext, Payload) -> BoxFuture<'static, Result<Outcome, JanusError>>
        + Send
        + Sync,
>;

/// The remainder of the chain, ending at the resolver.
pub struct Next {
    pub(crate) chain: Arc<[Arc<dyn Middleware>]>,
    pub(crate) index: usize,
    pub(crate) terminal: Terminal,
}

impl Next {
    /// Continue to the next layer (or the resolver, when the chain is
    /// exhausted).
    pub fn run(
        mut self,
        operation: Operation,
        ctx: JanusContext,
        payload: Payload,
    ) -> BoxFuture<'static, Result<Outcome, JanusError>> {
        match self.chain.get(self.index).cloned() {
            Some(layer) => {
                self.index += 1;
                Box::pin(async move { layer.handle(operation, ctx, payload, self).await })
            }
            None => (self.terminal)(operation, ctx, payload),
        }
    }
}

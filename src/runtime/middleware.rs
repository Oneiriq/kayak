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

use crate::runtime::args::{ActionArgs, GetArgs, ListArgs, ListOutput};
use crate::runtime::context::JanusContext;
use crate::runtime::error::JanusError;
use crate::runtime::resolvers::BoxFuture;

/// Which kind of operation is being dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    List,
    Get,
    Action,
}

/// The identity of one dispatched operation.
#[derive(Debug, Clone)]
pub struct Operation {
    pub resource: String,
    pub kind: OperationKind,
    /// Set when `kind` is [`OperationKind::Action`].
    pub action: Option<String>,
}

/// The validated arguments travelling through the chain.
#[derive(Debug, Clone)]
pub enum Payload {
    List(ListArgs),
    Get(GetArgs),
    Action(ActionArgs),
}

/// What came back from the resolver.
#[derive(Debug, Clone)]
pub enum Outcome {
    List(ListOutput),
    Get(Option<serde_json::Value>),
    Action(Option<serde_json::Value>),
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

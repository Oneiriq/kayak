//! The Janus runtime: the contract, executed.
//!
//! Generators compile the contract into artifacts; the runtime executes
//! it. A service registers a [`resolvers::Resolvers`] set (its own
//! data-access closures), stacks [`middleware::Middleware`] around them
//! (auth, auditing; all protocol-agnostic), and a
//! [`dispatch::Dispatcher`] enforces the contract before any resolver
//! runs: page limits clamped, filters and sorts checked against the
//! allowlists, action inputs type-checked. Protocol layers (the
//! `graphql` feature's dynamic schema; a service's own REST handlers)
//! all funnel through the same dispatcher, so policy lives in exactly
//! one place.
//!
//! Watching is the one long-lived operation. A resource the contract
//! marks watchable registers a resolver returning a [`RowStream`], and
//! the chain runs once, when the subscription opens. Rows travel from
//! the resolver to the subscriber without passing through middleware
//! again, so a stream that must end on a revoked credential has to
//! check that per row inside the resolver.

pub mod args;
pub mod context;
pub mod dispatch;
pub mod error;
pub mod guards;
pub mod middleware;
pub mod principal;
pub mod rate;
pub mod resolvers;
pub mod rest;

#[cfg(feature = "graphql")]
pub mod graphql;

pub use args::{
    ActionArgs, GetArgs, ListArgs, ListOutput, QueryArgs, SortDirection, SubListArgs, WatchArgs,
};
pub use context::JanusContext;
pub use dispatch::{Dispatcher, RuntimeBuildError};
pub use error::JanusError;
pub use guards::{
    guarded_fields, hidden_fields, strip_guarded, strip_hidden, GuardedField, Guards, HiddenField,
};
pub use middleware::{Middleware, Next, Operation, OperationKind, Outcome, Payload};
pub use principal::Principal;
pub use rate::{MemoryRateStore, RateStore};
pub use resolvers::{BoxFuture, Resolvers, RowStream};
pub use rest::{RestAnswer, RestRouter};

//! The Janus runtime: the contract, executed.
//!
//! Generators compile the contract into artifacts; the runtime executes
//! it. A service registers a [`resolvers::Resolvers`] set (its own
//! data-access closures), stacks [`middleware::Middleware`] around them
//! (auth, audit, metrics — protocol-agnostic), and a
//! [`dispatch::Dispatcher`] enforces the contract before any resolver
//! runs: page limits clamped, filters and sorts checked against the
//! allowlists, action inputs type-checked. Protocol layers (the
//! `graphql` feature's dynamic schema; a service's own REST handlers)
//! all funnel through the same dispatcher, so policy lives in exactly
//! one place.

pub mod args;
pub mod context;
pub mod dispatch;
pub mod error;
pub mod middleware;
pub mod resolvers;

#[cfg(feature = "graphql")]
pub mod graphql;

pub use args::{ActionArgs, GetArgs, ListArgs, ListOutput, SortDirection};
pub use context::JanusContext;
pub use dispatch::{Dispatcher, RuntimeBuildError};
pub use error::JanusError;
pub use middleware::{Middleware, Next, Operation, OperationKind, Outcome, Payload};
pub use resolvers::{BoxFuture, Resolvers};

//! Janus: one contract, every face.
//!
//! A serializable contract IR authored over `surql-rs` schema
//! definitions, validated against the schema's real indexes, and
//! compiled into API surfaces. The IR never restates the database
//! schema; it references tables and columns by name and resolves them
//! against the authoritative [`surql::schema::TableDefinition`]s, so a
//! contract cannot drift from the schema without failing generation.

pub mod clients;
pub mod diff;
pub mod generate;
pub mod ir;
mod naming;
pub mod openapi;
pub mod reserved;
pub mod sdl;
pub mod validate;

#[cfg(feature = "runtime")]
pub mod runtime;

pub use diff::{diff, Change};
pub use generate::generate_all;
pub use ir::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, GraphqlNames, Resource,
    SubGraphqlNames, SubResource, TypeRef,
};
pub use openapi::{generate_openapi, GenerateError};
pub use reserved::is_reserved;
pub use sdl::generate_sdl;
pub use validate::{validate, Violation};

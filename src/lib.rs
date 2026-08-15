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
mod emit;
pub mod generate;
mod indexes;
pub mod ir;
pub mod mcp;
mod methods;
mod naming;
pub mod openapi;
pub mod policy;
pub mod reserved;
pub mod scaffold;
pub mod sdl;
pub mod validate;

#[cfg(feature = "runtime")]
pub mod runtime;

#[cfg(feature = "verify")]
pub mod verify;

pub use diff::{diff, Change};
pub use generate::generate_all;
pub use ir::{
    Action, ActionField, ActionOutput, AuthScheme, ContentFaces, Contract, ContractLimits,
    FieldExposure, GraphqlNames, Query, RateClass, Resource, ResourceFaces, SearchBacking,
    SearchKind, SubGraphqlNames, SubResource, TypeRef,
};
pub use mcp::generate_mcp_tools;
pub use openapi::{generate_openapi, GenerateError};
pub use policy::{derive_policy, ClaimVocabulary, EnginePolicy, PolicyError};
pub use reserved::is_reserved;
pub use sdl::generate_sdl;
pub use validate::{validate, Violation};

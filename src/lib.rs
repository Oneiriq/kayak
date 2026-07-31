//! Janus: one contract, every face.
//!
//! A serializable contract IR authored over `surql-rs` schema
//! definitions, validated against the schema's real indexes, and
//! compiled into API surfaces. The IR never restates the database
//! schema — it references tables and columns by name and resolves them
//! against the authoritative [`surql::schema::TableDefinition`]s, so a
//! contract cannot drift from the schema without failing generation.

pub mod ir;
pub mod openapi;
pub mod validate;

pub use ir::{Contract, FieldExposure, Resource};
pub use openapi::{generate_openapi, GenerateError};
pub use validate::{validate, Violation};

//! Typed client generation: Rust, TypeScript, Python, Go.
//!
//! Each generator emits ONE self-contained source file: resource
//! types with the contract's renames and nullability, a client struct
//! carrying base URL and tenant, list/get per resource, and a method
//! per action. Dependency policy per language: Rust declares reqwest +
//! serde (stated in the file header); TypeScript uses `fetch`; Python
//! uses the standard library only; Go uses net/http only.
//!
//! This module is why the four SDKs stop being hand-maintained: one
//! contract, four clients, regenerated instead of ported.
//!
//! One module per language, because the file outgrew the 1000-line
//! budget and a second Rust flavour was about to make that worse. What
//! lives HERE is only what more than one language needs: resolving the
//! contract against the schema, and the naming and shape helpers that
//! every generator reads the same way. A helper used by exactly one
//! language belongs in that language's module.

mod go;
mod python;
mod rust;
mod typescript;

pub use go::generate_client_go;
pub use python::generate_client_py;
pub use rust::generate_client_rs;
pub use typescript::generate_client_ts;

use surql::schema::{FieldDefinition, TableDefinition};

use crate::ir::{Contract, Query, TypeRef};
use crate::naming::{snake, type_name};
use crate::openapi::GenerateError;
use crate::validate::validate;

pub(super) fn checked<'a>(
    contract: &'a Contract,
    schema: &'a [TableDefinition],
) -> Result<Vec<(&'a crate::ir::Resource, &'a TableDefinition)>, GenerateError> {
    let violations = validate(contract, schema);
    if !violations.is_empty() {
        return Err(GenerateError::Invalid(violations));
    }
    Ok(contract
        .resources
        .iter()
        .map(|resource| {
            let table = schema
                .iter()
                .find(|t| t.name == resource.table)
                .expect("validated: table exists");
            (resource, table)
        })
        .collect())
}

pub(super) fn column<'a>(table: &'a TableDefinition, name: &str) -> &'a FieldDefinition {
    table
        .fields
        .iter()
        .find(|f| f.name == name)
        .expect("validated: column exists")
}

pub(super) fn query_takes_id(query: &Query) -> bool {
    query.path.contains("{id}")
}

/// The inputs that travel as query-string values: everything except
/// the one the path already carries.
pub(super) fn query_params(query: &Query) -> Vec<&crate::ir::ActionField> {
    let mut params: Vec<&crate::ir::ActionField> = query
        .input
        .iter()
        .filter(|field| !(query_takes_id(query) && field.name == "id"))
        .collect();
    // TypeScript and Python both refuse a required parameter after an
    // optional one, so the required ones lead. A stable sort keeps the
    // contract's order within each group, which keeps goldens stable.
    params.sort_by_key(|field| !field.required);
    params
}

/// A query answers with whatever its resolver decided, and the IR
/// declares no shape for it, so every generated method returns the
/// language's open JSON type. Inventing a struct here would be
/// inventing a promise the contract does not make.
pub(super) fn query_kind(kind: TypeRef, language: &str) -> &'static str {
    match (language, kind) {
        ("rs", TypeRef::Int) => "i64",
        ("rs", TypeRef::Bool) => "bool",
        ("rs", TypeRef::Json) => "Value",
        ("rs", _) => "&str",
        ("ts", TypeRef::Int) => "number",
        ("ts", TypeRef::Bool) => "boolean",
        ("ts", TypeRef::Json) => "unknown",
        ("ts", _) => "string",
        ("py", TypeRef::Int) => "int",
        ("py", TypeRef::Bool) => "bool",
        ("py", TypeRef::Json) => "Any",
        ("py", _) => "str",
        ("go", TypeRef::Int) => "int64",
        ("go", TypeRef::Bool) => "bool",
        ("go", TypeRef::Json) => "any",
        _ => "string",
    }
}

/// The schema table a sub-resource reads, resolved after validation
pub(super) fn sub_table<'a>(
    schema: &'a [TableDefinition],
    sub: &crate::ir::SubResource,
) -> &'a TableDefinition {
    schema
        .iter()
        .find(|t| t.name == sub.table)
        .expect("validated: table exists")
}

/// The generated type name for a sub-resource, composed with the
/// parent so two parents may each carry a `versions` collection.
pub(super) fn sub_type_name(parent: &crate::ir::Resource, sub: &crate::ir::SubResource) -> String {
    format!("{}{}", type_name(&parent.name), type_name(&sub.name))
}

/// The method stem that reaches a sub-collection: `versions` under
/// `files` becomes `list_versions_files`, matching the OpenAPI
/// operationId.
pub(super) fn sub_method_stem(
    parent: &crate::ir::Resource,
    sub: &crate::ir::SubResource,
) -> String {
    format!("list_{}_{}", snake(&sub.name), snake(&parent.name))
}

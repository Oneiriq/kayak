//! Looking up what validation already proved is there.
//!
//! Every generator resolves the same two things: the table a resource
//! reads, and the column an exposure names. Validation refuses a
//! contract naming either wrongly, so by the time a generator runs the
//! lookups cannot fail -- and each site said so with an
//! `.expect("validated: table exists")`.
//!
//! Fifteen of those is fifteen places a reader auditing panics has to
//! reconstruct the same argument, and the argument is only as good as
//! the coupling between two modules that do not reference each other.
//! If validation and generation ever disagree, a library taking the
//! process down is the worst of the available behaviors: the caller
//! cannot catch it, and the message names a line rather than a
//! contract.
//!
//! So they return errors. The variant says plainly that reaching it is
//! a kayak bug rather than a contract's fault, which is the honest
//! thing to tell whoever sees it.

use surql::schema::{FieldDefinition, TableDefinition};

use crate::openapi::GenerateError;

/// The schema table named by `name`.
///
/// # Errors
/// [`GenerateError::Unresolved`] if the schema has no such table,
/// which validation should already have refused.
pub(crate) fn table<'a>(
    schema: &'a [TableDefinition],
    name: &str,
) -> Result<&'a TableDefinition, GenerateError> {
    schema
        .iter()
        .find(|table| table.name == name)
        .ok_or_else(|| GenerateError::Unresolved {
            what: "table",
            name: name.to_owned(),
        })
}

/// The column named by `name` on `table`.
///
/// # Errors
/// [`GenerateError::Unresolved`] if the table has no such column,
/// which validation should already have refused.
pub(crate) fn column<'a>(
    table: &'a TableDefinition,
    name: &str,
) -> Result<&'a FieldDefinition, GenerateError> {
    table
        .fields
        .iter()
        .find(|field| field.name == name)
        .ok_or_else(|| GenerateError::Unresolved {
            what: "column",
            name: name.to_owned(),
        })
}

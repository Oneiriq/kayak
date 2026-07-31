//! The contract intermediate representation.
//!
//! A contract is data: serializable, versioned, diffable, checked in.
//! It deliberately does not restate the database schema — resources
//! reference tables and columns by name, and validation resolves those
//! references against the authoritative `surql-rs` definitions. What
//! lives here is exclusively API-side: exposure, renaming, filter and
//! sort allowlists, and pagination bounds.

use serde::{Deserialize, Serialize};

/// A complete API contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    /// Contract name; becomes the OpenAPI title.
    pub name: String,
    /// Semantic version of the contract itself (not the service).
    pub version: String,
    /// IR schema revision, for forward-compatible tooling.
    #[serde(default = "default_ir_revision")]
    pub ir_revision: u32,
    /// Exposed resources.
    pub resources: Vec<Resource>,
}

fn default_ir_revision() -> u32 {
    1
}

/// One exposed resource over one table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// API-facing name (plural, kebab/snake as the API prefers).
    pub name: String,
    /// Backing table in the schema.
    pub table: String,
    /// Projected fields. Nothing is exposed that is not listed.
    pub fields: Vec<FieldExposure>,
    /// Columns the SERVER always equality-binds before any caller input
    /// — tenant scoping, soft-delete filters. Never exposed as API
    /// parameters; they exist so index-prefix validation can credit
    /// them: an index `(tenant_id, state, created_at)` serves a
    /// `created_at` sort because `tenant_id` is pinned and `state` is
    /// filterable.
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Columns callers may filter on. Validated against indexes.
    #[serde(default)]
    pub filterable: Vec<String>,
    /// Columns callers may sort on. Validated against index prefixes.
    #[serde(default)]
    pub sortable: Vec<String>,
    /// Page-size ceiling for list endpoints.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u32,
}

fn default_max_page_size() -> u32 {
    100
}

/// One projected column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldExposure {
    /// Column name in the table.
    pub column: String,
    /// API-facing name; defaults to the column name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
}

impl FieldExposure {
    /// Expose a column under its own name.
    pub fn column(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: None,
        }
    }

    /// Expose a column under an API-facing name.
    pub fn renamed(column: impl Into<String>, rename: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: Some(rename.into()),
        }
    }

    /// The name the API surface uses.
    pub fn api_name(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_round_trips_as_data() {
        let contract = Contract {
            name: "copal".into(),
            version: "1.0.0".into(),
            ir_revision: 1,
            resources: vec![Resource {
                name: "files".into(),
                table: "file".into(),
                fields: vec![
                    FieldExposure::column("path"),
                    FieldExposure::renamed("size_bytes", "size"),
                ],
                pinned: vec!["tenant_id".into()],
                filterable: vec!["state".into()],
                sortable: vec!["created_at".into()],
                max_page_size: 100,
            }],
        };
        let json = serde_json::to_string_pretty(&contract).unwrap();
        let back: Contract = serde_json::from_str(&json).unwrap();
        assert_eq!(back, contract);
        assert_eq!(back.resources[0].fields[1].api_name(), "size");
    }
}

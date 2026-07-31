//! The contract intermediate representation.
//!
//! A contract is data: serializable, versioned, diffable, checked in.
//! It never restates the database schema: resources
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
    /// (tenant scoping, soft-delete filters). Never exposed as API
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
    /// Verbs beyond list/get: uploads, grants, deletions, workflow
    /// starts. An action binds to a domain use-case on the server; the
    /// contract only describes its wire shape.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// GraphQL-scoped name overrides. REST paths and generated clients
    /// never see these; they exist because GraphQL names are part of a
    /// deployed schema's identity (fragments name types, queries name
    /// fields) and sometimes must differ from the derived defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<GraphqlNames>,
}

/// GraphQL-side name overrides for one resource. Every member is
/// optional; absent members fall back to the derived names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphqlNames {
    /// Object type name (default: PascalCase singular of the resource).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Query field returning a page (default: camelCase resource name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_field: Option<String>,
    /// Query field returning one instance (default: camelCase singular).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub get_field: Option<String>,
}

/// One verb on a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Snake-case action name; generators derive per-language method
    /// and mutation names from it.
    pub name: String,
    /// HTTP method: POST, PUT, DELETE, or PATCH.
    pub method: String,
    /// Path suffix under the resource (`"/{id}/url"`, `"/{id}"`, or
    /// `""` for the collection itself). A literal `{id}` marks an
    /// instance action and becomes a required id parameter everywhere.
    #[serde(default)]
    pub path: String,
    /// Request-body fields.
    #[serde(default)]
    pub input: Vec<ActionField>,
    /// What the action returns.
    #[serde(default)]
    pub output: ActionOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL mutation field name override (default: camelCase
    /// singular resource + PascalCase action, e.g. `fileIssueUrl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql_field: Option<String>,
}

impl Action {
    /// Whether this action targets one instance (path carries `{id}`).
    pub fn takes_id(&self) -> bool {
        self.path.contains("{id}")
    }

    /// The GraphQL mutation field name: the override, or camelCase
    /// singular resource + PascalCase action (`fileIssueUrl`).
    pub fn graphql_field_name(&self, resource: &Resource) -> String {
        self.graphql_field.clone().unwrap_or_else(|| {
            format!(
                "{}{}",
                crate::naming::camel(crate::naming::singular(&resource.name)),
                crate::naming::pascal(&self.name),
            )
        })
    }
}

impl Resource {
    /// The GraphQL object type name: the override, or PascalCase
    /// singular of the resource name (`files` -> `File`).
    pub fn graphql_type_name(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.type_name.clone())
            .unwrap_or_else(|| crate::naming::type_name(&self.name))
    }

    /// The Query field returning a page: the override, or camelCase
    /// resource name (`files`).
    pub fn graphql_list_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.list_field.clone())
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }

    /// The Query field returning one instance: the override, or
    /// camelCase singular (`file`).
    pub fn graphql_get_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.get_field.clone())
            .unwrap_or_else(|| crate::naming::camel(crate::naming::singular(&self.name)))
    }
}

/// One request-body field of an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionField {
    pub name: String,
    pub kind: TypeRef,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Wire types for action inputs, kept small; anything richer
/// is `Json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeRef {
    String,
    Int,
    Bool,
    Json,
}

/// What an action returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutput {
    /// The parent resource's object schema.
    Resource,
    /// A free-form JSON object.
    #[default]
    Json,
    /// Nothing (HTTP 204).
    None,
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
                actions: vec![Action {
                    name: "issue_url".into(),
                    method: "POST".into(),
                    path: "/{id}/url".into(),
                    input: vec![ActionField {
                        name: "ttl_secs".into(),
                        kind: TypeRef::Int,
                        required: false,
                        description: None,
                    }],
                    output: ActionOutput::Json,
                    description: Some("Issue a signed URL.".into()),
                    graphql_field: None,
                }],
                graphql: None,
            }],
        };
        let json = serde_json::to_string_pretty(&contract).unwrap();
        let back: Contract = serde_json::from_str(&json).unwrap();
        assert_eq!(back, contract);
        assert_eq!(back.resources[0].fields[1].api_name(), "size");
    }
}

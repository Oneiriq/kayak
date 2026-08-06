//! The contract intermediate representation.
//!
//! A contract is data: serializable, versioned, diffable, checked in.
//! It never restates the database schema: resources
//! reference tables and columns by name, and validation resolves those
//! references against the authoritative `surql-rs` definitions. What
//! lives here is exclusively API-side: exposure, renaming, filter and
//! sort allowlists, and pagination bounds.

use std::collections::BTreeMap;

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
    /// Named consumption budgets. A resource or action that names one
    /// is metered against it; introducing or shrinking a budget is a
    /// breaking change the differ names. Defined here so the budgets
    /// are review-visible beside the operations they bound.
    #[serde(default)]
    pub rate_classes: Vec<RateClass>,
    /// Request-cost ceilings, declared here so they appear in the
    /// artifacts and the differ tracks them. A ceiling hand-wired at
    /// the protocol layer is policy the contract does not know about:
    /// invisible in review, silent when it tightens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ContractLimits>,
    /// Exposed resources.
    pub resources: Vec<Resource>,
    /// Reads that are not listings: a question with typed inputs and
    /// an answer, ordered by whatever the resolver decides. Search is
    /// the shape that motivated them, and a listing cannot express
    /// one: relevance is not a sort column and a query string is not
    /// a filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<Query>,
}

/// One named read that answers a question rather than paging a
/// collection.
///
/// Queries render as GraphQL query fields and REST `GET`s, carry the
/// same scope and rate declarations resources and actions carry, and
/// the differ treats them the way it treats actions: adding one is
/// additive, removing or renaming one breaks, and tightening what it
/// requires breaks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Snake-case name; generators derive per-language method and
    /// GraphQL field names from it.
    pub name: String,
    /// REST path, absolute under the API root (`"/v1/search"`).
    pub path: String,
    /// Parameters, which arrive as query-string values on REST and
    /// as field arguments on GraphQL.
    #[serde(default)]
    pub input: Vec<ActionField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL field name override (default: camelCase of the name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql_field: Option<String>,
    /// Scopes a caller must hold. Empty means open.
    #[serde(default)]
    pub requires: Vec<String>,
    /// The rate class metering this query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
}

impl Query {
    /// The GraphQL field name: the override, or camelCase of the
    /// query name (`file_text` -> `fileText`).
    pub fn graphql_field_name(&self) -> String {
        self.graphql_field
            .clone()
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }
}

/// One named consumption budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateClass {
    /// Snake-case name resources and actions reference.
    pub name: String,
    /// Units allowed per caller per minute. A listing costs its row
    /// limit; every other operation costs one.
    pub units_per_minute: u64,
}

/// Ceilings on what one operation may cost. The served GraphQL schema
/// enforces both before any resolver runs; REST operations have fixed
/// shape and depth, so the ceilings exist for the face where cost is
/// caller-controlled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractLimits {
    /// Maximum selection depth of one GraphQL operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum field-selection count of one GraphQL operation, which
    /// is what alias amplification multiplies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_complexity: Option<u32>,
    /// Maximum concurrently open subscriptions per principal. A
    /// subscription holds server resources for as long as the client
    /// stays; without a ceiling, one caller can hold every live query
    /// the deployment will ever serve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_watches_per_principal: Option<u32>,
}

pub(crate) fn default_ir_revision() -> u32 {
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
    /// The values a filterable column accepts, for the columns whose
    /// values are a closed set. Keyed by column name; a column absent
    /// here takes anything. Same reasoning as an action field's
    /// `options`, applied to the other place a caller supplies a
    /// value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter_options: BTreeMap<String, Vec<String>>,
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
    /// The resource's byte faces, when it has content: an upload
    /// path, a download path, or both. Bytes are streams rather than
    /// JSON, so these render into the OpenAPI document as
    /// octet-stream operations instead of becoming GraphQL or MCP
    /// shapes; the grant actions are the agent path to the same
    /// bytes. Declaring them here puts the write half of a service
    /// under the differ: removing a face is breaking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ContentFaces>,
    /// Collections that hang off ONE instance of this resource: a
    /// file's versions, an endpoint's deliveries. They read like a
    /// resource and are reachable only through a parent id, which is
    /// why they are not resources of their own.
    #[serde(default)]
    pub sub_resources: Vec<SubResource>,
    /// The rate class metering reads of this resource: list, get,
    /// sub-collections, and watch opens. Absent means unmetered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
    /// Scopes a caller must hold to READ this resource: list, get,
    /// sub-collections, and watching all check them. Empty means open
    /// to any caller the middleware admits, which is every existing
    /// contract's behavior. A sub-collection is read under its
    /// parent's requirement, because it is reached through the parent.
    #[serde(default)]
    pub reads_require: Vec<String>,
    /// Whether callers may watch this resource for changes. A watchable
    /// resource gains a GraphQL Subscription field and requires a watch
    /// resolver; it changes nothing about REST, which has no long-lived
    /// shape here. Watchers narrow the stream with the same `filterable`
    /// columns list callers use, so a resource has ONE filter vocabulary
    /// whichever operation reads it.
    #[serde(default)]
    pub watchable: bool,
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
    /// Subscription field delivering rows as they change (default:
    /// camelCase singular + `Changed`). Read only when the resource is
    /// watchable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_field: Option<String>,
}

/// A collection belonging to one instance of a parent resource.
///
/// It lists and pages like a resource, but it has no id-addressable
/// form of its own and no actions: everything about it is reached
/// through the parent. `GET /v1/files/{id}/versions` on REST, a field
/// on the parent object type in GraphQL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubResource {
    /// API-facing name, plural. Becomes the path segment and the
    /// GraphQL field on the parent.
    pub name: String,
    /// Backing table in the schema.
    pub table: String,
    /// Column on `table` holding the parent's id. The server always
    /// equality-binds it, so index validation credits it the way it
    /// credits `pinned`.
    pub parent_key: String,
    /// Projected fields. Nothing is exposed that is not listed.
    pub fields: Vec<FieldExposure>,
    /// Further server-bound columns, beyond `parent_key`.
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Columns callers may filter on. Validated against indexes.
    #[serde(default)]
    pub filterable: Vec<String>,
    /// Columns callers may sort on. Validated against index prefixes.
    #[serde(default)]
    pub sortable: Vec<String>,
    /// Page-size ceiling.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// GraphQL-scoped name overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphql: Option<SubGraphqlNames>,
}

/// GraphQL name overrides for one sub-resource.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubGraphqlNames {
    /// Object type name (default: parent singular + sub singular, e.g.
    /// `FileVersion`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Field on the parent object (default: camelCase sub name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl SubResource {
    /// The GraphQL object type name: the override, or PascalCase
    /// singular of the parent and of this name (`files` + `versions`
    /// -> `FileVersion`). Composing with the parent is what keeps two
    /// parents with a `versions` collection from colliding.
    pub fn graphql_type_name(&self, parent: &Resource) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.type_name.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}{}",
                    crate::naming::type_name(&parent.name),
                    crate::naming::type_name(&self.name),
                )
            })
    }

    /// The field on the parent object type: the override, or camelCase
    /// of this name (`versions`).
    pub fn graphql_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.field.clone())
            .unwrap_or_else(|| crate::naming::camel(&self.name))
    }

    /// Columns the server equality-binds before any caller input:
    /// `parent_key` always, plus anything explicitly pinned.
    pub fn bound_columns(&self) -> Vec<&str> {
        std::iter::once(self.parent_key.as_str())
            .chain(self.pinned.iter().map(String::as_str))
            .collect()
    }
}

/// A resource's byte faces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFaces {
    /// `PUT /v1/{resource}/{id}/content` accepts the bytes.
    #[serde(default)]
    pub upload: bool,
    /// `GET /v1/{resource}/{id}/content` serves them.
    #[serde(default)]
    pub download: bool,
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
    /// Scopes a caller must hold to invoke this action. Empty means
    /// open, which is every existing contract's behavior.
    #[serde(default)]
    pub requires: Vec<String>,
    /// The rate class metering this action. Absent means unmetered;
    /// there is no fallback to the resource's class, because an
    /// action's cost profile rarely matches its resource's reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_class: Option<String>,
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
                crate::naming::camel(&crate::naming::singular(&resource.name)),
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
            .unwrap_or_else(|| crate::naming::camel(&crate::naming::singular(&self.name)))
    }

    /// The Subscription field delivering rows as they change: the
    /// override, or camelCase singular + `Changed` (`fileChanged`).
    pub fn graphql_watch_field(&self) -> String {
        self.graphql
            .as_ref()
            .and_then(|g| g.watch_field.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}Changed",
                    crate::naming::camel(&crate::naming::singular(&self.name)),
                )
            })
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
    /// Whether the caller may name several of `options` at once.
    ///
    /// The value travels as one comma-separated string, which is what
    /// a query parameter can carry without ceremony. Meaningless
    /// without `options`, since a set is what there is to choose
    /// several of.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
    /// The values this input accepts, when they are a closed set.
    ///
    /// Empty means anything the type allows. A non-empty list is a
    /// constraint the dispatcher enforces, so it cannot drift from
    /// what the resolver does: a value outside it is refused before
    /// the resolver runs. It also reaches every face, as an `enum` in
    /// the OpenAPI document and the MCP manifest, and as a menu in the
    /// console instead of a box a caller has to guess into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
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
    /// Named guard deciding, per caller, whether this field is
    /// visible. A guarded field renders nullable on every generated
    /// surface and is OMITTED from rows the guard denies; a read is
    /// never an error. Absent means visible to every caller the
    /// operation admits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

impl FieldExposure {
    /// Expose a column under its own name.
    pub fn column(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: None,
            guard: None,
        }
    }

    /// Expose a column under an API-facing name.
    pub fn renamed(column: impl Into<String>, rename: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            rename: Some(rename.into()),
            guard: None,
        }
    }

    /// Guard this exposure with the named policy.
    pub fn with_guard(mut self, guard: impl Into<String>) -> Self {
        self.guard = Some(guard.into());
        self
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
            limits: None,
            rate_classes: vec![],
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
                        multiple: false,
                        description: None,
                        options: Vec::new(),
                    }],
                    output: ActionOutput::Json,
                    description: Some("Issue a signed URL.".into()),
                    graphql_field: None,
                    requires: vec![],
                    rate_class: None,
                }],
                graphql: None,
                watchable: false,
                reads_require: vec![],
                rate_class: None,
                sub_resources: vec![],
                content: None,
                filter_options: Default::default(),
            }],
            queries: vec![],
        };
        let json = serde_json::to_string_pretty(&contract).unwrap();
        let back: Contract = serde_json::from_str(&json).unwrap();
        assert_eq!(back, contract);
        assert_eq!(back.resources[0].fields[1].api_name(), "size");
    }
}

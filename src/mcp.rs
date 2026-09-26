//! The MCP tool manifest, generated from the contract.
//!
//! Agents reach tools through MCP, and a hand-written wrapper drifts
//! from the surface it wraps. This manifest derives from the same
//! declaration that produces the OpenAPI document, the GraphQL
//! schema, and the engine's row security, so the agent surface is
//! governed by the same differ as every other face: a removed
//! resource, a tightened scope, or a renamed action changes this
//! artifact, and the drift gate refuses until the change is blessed.
//!
//! Tool names are deterministic: `{resource}_list`, `{singular}_get`,
//! `{singular}_{sub}_list` for a sub-collection, the action's snake
//! form, and a query's own name. Scope and rate declarations ride each
//! tool as annotations, so a serving runtime can enforce them and an
//! agent can read what a call will cost before making it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::ir::{ActionOutput, Contract, Query, Resource, SubResource, TypeRef};
use crate::naming::singular;
use crate::openapi::{addressed_identity, GenerateError};

/// The manifest: every tool the contract implies, in MCP's
/// `tools/list` shape.
///
/// It reads only the contract and so takes no schema to validate
/// against. Validate first with [`crate::validate()`], or generate
/// through [`crate::generate_all`], which does.
pub fn generate_mcp_tools(contract: &Contract) -> Result<Value, GenerateError> {
    let mut tools = Vec::new();
    for resource in &contract.resources {
        if resource.faces.list {
            tools.push(list_tool(resource));
        }
        if resource.faces.get {
            tools.push(get_tool(resource)?);
        }
        for sub in &resource.sub_resources {
            tools.push(sub_list_tool(resource, sub)?);
        }
        for action in &resource.actions {
            tools.push(action_tool(resource, action));
        }
    }
    for query in &contract.queries {
        tools.push(query_tool(query));
    }
    Ok(json!({
        "tools": tools,
    }))
}

fn annotations(requires: &[String], rate_class: Option<&str>) -> Value {
    let mut map = Map::new();
    if !requires.is_empty() {
        map.insert("requiredScopes".to_owned(), json!(requires));
    }
    if let Some(class) = rate_class {
        map.insert("rateClass".to_owned(), json!(class));
    }
    Value::Object(map)
}

fn type_schema(kind: TypeRef) -> Value {
    match kind {
        TypeRef::String => json!({ "type": "string" }),
        TypeRef::Int => json!({ "type": "integer" }),
        TypeRef::Bool => json!({ "type": "boolean" }),
        TypeRef::Json => json!({ "type": "object" }),
    }
}

/// The paging, filter, and sort inputs a listing takes, added after
/// whatever `properties` already holds.
fn page_properties(
    properties: &mut Map<String, Value>,
    max_page_size: u32,
    filterable: &[String],
    filter_options: Option<&BTreeMap<String, Vec<String>>>,
    sortable: &[String],
) {
    properties.insert(
        "limit".to_owned(),
        json!({
            "type": "integer",
            "description": format!("Rows per page, at most {max_page_size}."),
        }),
    );
    properties.insert(
        "cursor".to_owned(),
        json!({ "type": "string", "description": "Resume from a previous page." }),
    );
    for column in filterable {
        let mut schema = json!({ "type": "string", "description": format!("Filter by {column}.") });
        if let Some(options) = filter_options.and_then(|options| options.get(column)) {
            schema["enum"] = json!(options);
        }
        properties.insert(column.clone(), schema);
    }
    if !sortable.is_empty() {
        properties.insert(
            "sort".to_owned(),
            json!({
                "type": "string",
                "description": format!(
                    "Sort column, one of: {}. Suffix with :desc for newest first.",
                    sortable.join(", "),
                ),
            }),
        );
    }
}

fn list_tool(resource: &Resource) -> Value {
    let mut properties = Map::new();
    page_properties(
        &mut properties,
        resource.max_page_size,
        &resource.filterable,
        Some(&resource.filter_options),
        &resource.sortable,
    );
    json!({
        "name": format!("{}_list", resource.name),
        "description": format!("List {} for the authenticated tenant.", resource.name),
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "additionalProperties": false,
        },
        "annotations": annotations(&resource.reads_require, resource.rate_class.as_deref()),
    })
}

fn get_tool(resource: &Resource) -> Result<Value, GenerateError> {
    let identity = addressed_identity(resource)?;
    Ok(json!({
        "name": format!("{}_get", singular(&resource.name)),
        "description": format!("Fetch one of {} by {}.", resource.name, identity),
        "inputSchema": {
            "type": "object",
            "properties": { identity: { "type": "string" } },
            "required": [identity],
            "additionalProperties": false,
        },
        "annotations": annotations(&resource.reads_require, resource.rate_class.as_deref()),
    }))
}

/// A sub-collection pages like a listing, reached through one parent:
/// the parent's identity is the one required input. It is read under
/// the parent's scopes and metered by the parent's rate class, as the
/// dispatcher reads it.
fn sub_list_tool(resource: &Resource, sub: &SubResource) -> Result<Value, GenerateError> {
    let identity = addressed_identity(resource)?;
    let one = singular(&resource.name);
    let mut properties = Map::new();
    properties.insert(
        identity.to_owned(),
        json!({
            "type": "string",
            "description": format!("The {one} whose {} to list.", sub.name),
        }),
    );
    page_properties(
        &mut properties,
        sub.max_page_size,
        &sub.filterable,
        None,
        &sub.sortable,
    );
    Ok(json!({
        "name": format!("{one}_{}_list", sub.name),
        "description": sub
            .description
            .clone()
            .unwrap_or_else(|| format!("List the {} of one of {}.", sub.name, resource.name)),
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": [identity],
            "additionalProperties": false,
        },
        "annotations": annotations(&resource.reads_require, resource.rate_class.as_deref()),
    }))
}

fn action_tool(resource: &Resource, action: &crate::ir::Action) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    if action.takes_id() {
        properties.insert("id".to_owned(), json!({ "type": "string" }));
        required.push(json!("id"));
    }
    for field in &action.input {
        let mut schema = type_schema(field.kind);
        if !field.options.is_empty() {
            schema["enum"] = json!(field.options);
        }
        if field.multiple {
            schema = json!({ "type": "array", "items": schema });
        }
        if let (Some(description), Some(object)) = (&field.description, schema.as_object_mut()) {
            object.insert("description".to_owned(), json!(description));
        }
        properties.insert(field.name.clone(), schema);
        if field.required {
            required.push(json!(field.name));
        }
    }
    let returns = match action.output {
        ActionOutput::Resource => "the updated row",
        ActionOutput::Json => "a JSON answer",
        ActionOutput::None => "nothing on success",
    };
    json!({
        "name": format!("{}_{}", singular(&resource.name), action.name),
        "description": action
            .description
            .clone()
            .unwrap_or_else(|| format!("{} on {}; returns {returns}.", action.name, resource.name)),
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": annotations(&action.requires, action.rate_class.as_deref()),
    })
}

fn query_tool(query: &Query) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &query.input {
        let mut schema = type_schema(field.kind);
        if !field.options.is_empty() {
            schema["enum"] = json!(field.options);
        }
        if field.multiple {
            schema = json!({ "type": "array", "items": schema });
        }
        if let (Some(description), Some(object)) = (&field.description, schema.as_object_mut()) {
            object.insert("description".to_owned(), json!(description));
        }
        properties.insert(field.name.clone(), schema);
        if field.required {
            required.push(json!(field.name));
        }
    }
    // The backing rides the annotations the way scope and rate do:
    // capacity metadata an agent can read before calling, on the face
    // where an agent reads, without touching the input schema that
    // decides what a call looks like.
    let mut notes = annotations(&query.requires, query.rate_class.as_deref());
    if !query.backing.is_empty() {
        notes["backing"] = json!(query.backing);
    }
    json!({
        "name": query.name,
        "description": query
            .description
            .clone()
            .unwrap_or_else(|| format!("The {} query.", query.name)),
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ActionField, FieldExposure};

    fn contract() -> Contract {
        Contract {
            name: "copal".into(),
            version: "0.1.0".into(),
            ir_revision: 1,
            api_prefix: "/v1".into(),
            limits: None,
            rate_classes: vec![],
            auth: Default::default(),
            resources: vec![Resource {
                name: "files".into(),
                table: "file".into(),
                identity: Default::default(),
                fields: vec![FieldExposure::column("path")],
                pinned: vec![],
                pinned_either: vec![],
                filterable: vec!["state".into()],
                sortable: vec!["created_at".into()],
                max_page_size: 100,
                graphql: None,
                watchable: false,
                reads_require: vec!["read".into()],
                rate_class: Some("reads".into()),
                sub_resources: vec![],
                actions: vec![crate::ir::Action {
                    name: "remove".into(),
                    method: "DELETE".into(),
                    path: "/{id}".into(),
                    input: vec![],
                    output: ActionOutput::None,
                    description: None,
                    graphql_field: None,
                    requires: vec!["write".into()],
                    rate_class: Some("mutations".into()),
                }],
                content: None,
                filter_options: Default::default(),
                faces: Default::default(),
            }],
            queries: vec![Query {
                name: "search".into(),
                path: "/v1/search".into(),
                input: vec![ActionField {
                    name: "q".into(),
                    kind: TypeRef::String,
                    required: true,
                    multiple: false,
                    description: None,
                    options: Vec::new(),
                }],
                description: None,
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: Some("reads".into()),
                searches: vec![crate::ir::SearchKind::Lexical],
                backing: vec![crate::ir::SearchBacking {
                    table: "text_chunk".into(),
                    column: "body".into(),
                    index: "idx_chunk_body".into(),
                    kind: crate::ir::SearchKind::Lexical,
                    dimension: None,
                    optional: false,
                }],
            }],
        }
    }

    #[test]
    fn every_face_of_the_contract_becomes_a_tool() {
        let manifest = generate_mcp_tools(&contract()).unwrap();
        let names: Vec<&str> = manifest["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["files_list", "file_get", "file_remove", "search"]);
    }

    /// A sub-collection is a tool beside its parent's listing and
    /// getter, as it is a path in the OpenAPI document and a method in
    /// every client. It takes the parent's identity and pages under
    /// the parent's scopes and rate class.
    #[test]
    fn a_sub_collection_becomes_a_tool() {
        let mut contract = contract();
        contract.resources[0].identity = crate::ir::Identity::Column("key".into());
        contract.resources[0].sub_resources = vec![SubResource {
            name: "versions".into(),
            table: "file_version".into(),
            parent_key: "file".into(),
            identity: Default::default(),
            fields: vec![FieldExposure::column("number")],
            pinned: vec![],
            filterable: vec!["state".into()],
            sortable: vec!["created_at".into()],
            max_page_size: 20,
            description: None,
            graphql: None,
        }];
        let manifest = generate_mcp_tools(&contract).unwrap();
        let tools = manifest["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "files_list",
                "file_get",
                "file_versions_list",
                "file_remove",
                "search"
            ],
        );
        let versions = &tools[2];
        let properties: Vec<&String> = versions["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(properties, ["key", "limit", "cursor", "state", "sort"]);
        assert_eq!(versions["inputSchema"]["required"], json!(["key"]));
        assert_eq!(versions["annotations"]["requiredScopes"], json!(["read"]));
        assert_eq!(versions["annotations"]["rateClass"], "reads");
    }

    #[test]
    fn declarations_ride_the_tools() {
        let manifest = generate_mcp_tools(&contract()).unwrap();
        let tools = manifest["tools"].as_array().unwrap();
        let remove = tools.iter().find(|t| t["name"] == "file_remove").unwrap();
        assert_eq!(remove["annotations"]["requiredScopes"], json!(["write"]));
        assert_eq!(remove["annotations"]["rateClass"], "mutations");
        assert_eq!(remove["inputSchema"]["required"], json!(["id"]));

        let search = tools.iter().find(|t| t["name"] == "search").unwrap();
        assert_eq!(search["inputSchema"]["required"], json!(["q"]));
        assert_eq!(search["annotations"]["requiredScopes"], json!(["read"]));
        // The backing rides beside scope and rate, and stays off the
        // input schema: it says what answers the call, not what the
        // call looks like.
        assert_eq!(
            search["annotations"]["backing"],
            json!([{
                "table": "text_chunk",
                "column": "body",
                "index": "idx_chunk_body",
                "kind": "lexical",
            }]),
        );
        assert!(search["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| k == "q"));
    }
}

//! OpenAPI 3.1 generation.
//!
//! Emits a deterministic document (sorted keys via `BTreeMap`-backed
//! `serde_json::Value` construction) so the checked-in artifact diffs
//! cleanly. Types come from the schema's `FieldDefinition`s: the
//! contract never restates a column type, so drift between database and
//! API document is structurally impossible.
//!
//! Generation refuses to run on an invalid contract; the validation
//! gate is not advisory.

use serde_json::{json, Map, Value};

use surql::schema::{FieldDefinition, FieldType, TableDefinition};

use crate::ir::{Action, ActionOutput, Contract, Resource, TypeRef};
use crate::validate::{validate, Violation};

/// Errors from generation.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// The contract does not match the schema; every violation listed.
    #[error("contract failed validation:\n{}", format_violations(.0))]
    Invalid(Vec<Violation>),
    /// A target name the orchestrator does not recognize.
    #[error("unknown generation target {0:?}")]
    UnknownTarget(String),
    /// The engine policy could not be rendered.
    #[error("engine policy: {0}")]
    Policy(#[from] crate::policy::PolicyError),
}

fn format_violations(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("  - {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate the OpenAPI 3.1 document for `contract` over `schema`.
pub fn generate_openapi(
    contract: &Contract,
    schema: &[TableDefinition],
) -> Result<Value, GenerateError> {
    let violations = validate(contract, schema);
    if !violations.is_empty() {
        return Err(GenerateError::Invalid(violations));
    }

    let mut paths = Map::new();
    let mut schemas = Map::new();
    for resource in &contract.resources {
        let table = schema
            .iter()
            .find(|t| t.name == resource.table)
            .expect("validated: table exists");
        let schema_name = component_name(&resource.name);
        schemas.insert(schema_name.clone(), resource_schema(resource, table));
        schemas.insert(format!("{schema_name}Page"), page_schema(&schema_name));
        paths.insert(
            format!("/v1/{}", resource.name),
            list_path(resource, &schema_name),
        );
        paths.insert(
            format!("/v1/{}/{{id}}", resource.name),
            get_path(resource, &schema_name),
        );
        for sub in &resource.sub_resources {
            let sub_table = schema
                .iter()
                .find(|t| t.name == sub.table)
                .expect("validated: table exists");
            let sub_schema = format!("{schema_name}{}", component_name(&sub.name));
            schemas.insert(sub_schema.clone(), sub_resource_schema(sub, sub_table));
            schemas.insert(format!("{sub_schema}Page"), page_schema(&sub_schema));
            paths.insert(
                format!("/v1/{}/{{id}}/{}", resource.name, sub.name),
                sub_list_path(resource, sub, &sub_schema),
            );
        }
        if let Some(content) = &resource.content {
            let mut operations = Map::new();
            if content.upload {
                operations.insert(
                    "put".to_owned(),
                    json!({
                        "operationId": format!("{}_upload_content", crate::naming::singular(&resource.name)),
                        "summary": "Upload the content bytes; the stored digest becomes the ETag.",
                        "parameters": [ {
                            "name": "id", "in": "path", "required": true,
                            "schema": { "type": "string" },
                        } ],
                        "requestBody": {
                            "required": true,
                            "content": { "application/octet-stream": {
                                "schema": { "type": "string", "format": "binary" },
                            } },
                        },
                        "responses": { "200": { "description": "stored" } },
                    }),
                );
            }
            if content.download {
                operations.insert(
                    "get".to_owned(),
                    json!({
                        "operationId": format!("{}_download_content", crate::naming::singular(&resource.name)),
                        "summary": "Serve the content bytes, with ETag, Range, and conditional requests.",
                        "parameters": [ {
                            "name": "id", "in": "path", "required": true,
                            "schema": { "type": "string" },
                        } ],
                        "responses": { "200": {
                            "description": "the bytes",
                            "content": { "application/octet-stream": {
                                "schema": { "type": "string", "format": "binary" },
                            } },
                        } },
                    }),
                );
            }
            if !operations.is_empty() {
                paths.insert(
                    format!("/v1/{}/{{id}}/content", resource.name),
                    Value::Object(operations),
                );
            }
        }
        for action in &resource.actions {
            let path = format!("/v1/{}{}", resource.name, action.path);
            let entry = paths
                .entry(path)
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(object) = entry.as_object_mut() {
                object.insert(
                    action.method.to_ascii_lowercase(),
                    action_operation(resource, action, &schema_name),
                );
            }
        }
    }

    // Contract queries are REST surfaces too: the path they declare,
    // their parameters as query strings, and a free-form JSON answer.
    for query in &contract.queries {
        let parameters: Vec<Value> = query
            .input
            .iter()
            .filter(|field| !query.path.contains(&format!("{{{}}}", field.name)))
            .map(|field| {
                let mut schema = json!({ "type": type_name(field.kind) });
                if !field.options.is_empty() {
                    schema["enum"] = json!(field.options);
                }
                if field.multiple {
                    // `?facets=a,b`, which is form style without
                    // explode, so the array arrives as one parameter.
                    schema = json!({ "type": "array", "items": schema });
                }
                json!({
                    "name": field.name,
                    "in": "query",
                    "required": field.required,
                    "description": field.description,
                    "schema": schema,
                })
            })
            .collect();
        let mut path_parameters: Vec<Value> = query
            .input
            .iter()
            .filter(|field| query.path.contains(&format!("{{{}}}", field.name)))
            .map(|field| {
                json!({
                    "name": field.name,
                    "in": "path",
                    "required": true,
                    "description": field.description,
                    "schema": { "type": type_name(field.kind) },
                })
            })
            .collect();
        path_parameters.extend(parameters);
        let mut operation = json!({
            "operationId": query.name,
            "summary": query.description,
            "parameters": path_parameters,
            "responses": {
                "200": {
                    "description": "the answer",
                    "content": { "application/json": { "schema": { "type": "object" } } },
                },
            },
        });
        // The backing is capacity metadata, not wire shape: the path,
        // the parameters, and the answer stay exactly what they were,
        // and the declaration lands in the operation description,
        // which is where a reader of the document learns what a call
        // costs.
        if !query.backing.is_empty() {
            let described: Vec<String> = query
                .backing
                .iter()
                .map(|b| format!("{} via {} over {}.{}", b.kind, b.index, b.table, b.column))
                .collect();
            operation["description"] = json!(format!("Search backing: {}.", described.join("; ")));
        }
        let entry = paths
            .entry(query.path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(object) = entry.as_object_mut() {
            object.insert("get".to_owned(), operation);
        }
    }

    let mut document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": contract.name,
            "version": contract.version,
        },
        "paths": Value::Object(paths),
        "components": { "schemas": Value::Object(schemas) },
    });
    // Declared ceilings ride the document as a root extension, so
    // policy that refuses requests is visible where the API is read.
    if let Some(limits) = &contract.limits {
        let mut rendered = Map::new();
        if let Some(depth) = limits.max_depth {
            rendered.insert("max_depth".into(), json!(depth));
        }
        if let Some(complexity) = limits.max_complexity {
            rendered.insert("max_complexity".into(), json!(complexity));
        }
        if let Some(watches) = limits.max_watches_per_principal {
            rendered.insert("max_watches_per_principal".into(), json!(watches));
        }
        document["x-limits"] = Value::Object(rendered);
    }
    Ok(canonical(document))
}

/// Rebuild every object with keys in sorted order, so the emitted
/// document is byte-identical whether `serde_json` was compiled with
/// `preserve_order` (any dependent may unify that feature in) or not.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (key, inner) in entries {
                out.insert(key, canonical(inner));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// `files` -> `File`; `file-versions` -> `FileVersion`.
fn component_name(resource: &str) -> String {
    let singular = resource.strip_suffix('s').unwrap_or(resource);
    singular
        .split(['-', '_'])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The object schema for a sub-resource, identical in shape to a
/// resource's: an opaque id plus the exposed columns.
fn sub_resource_schema(sub: &crate::ir::SubResource, table: &TableDefinition) -> Value {
    let mut properties = Map::new();
    let mut required = vec![json!("id")];
    properties.insert("id".into(), json!({"type": "string"}));
    for exposure in &sub.fields {
        let field = table
            .fields
            .iter()
            .find(|f| f.name == exposure.column)
            .expect("validated: column exists");
        let mut schema = field_schema(field);
        if let Some(guard) = &exposure.guard {
            schema["x-guard"] = json!(guard);
        }
        properties.insert(exposure.api_name().to_owned(), schema);
        if !field.nullable && exposure.guard.is_none() {
            required.push(json!(exposure.api_name()));
        }
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    })
}

/// `GET /v1/{parent}/{id}/{sub}`: the parent id is a path parameter,
/// and the rest mirrors a list endpoint.
fn sub_list_path(parent: &Resource, sub: &crate::ir::SubResource, schema_name: &str) -> Value {
    let mut parameters = vec![
        json!({
            "name": "id",
            "in": "path",
            "required": true,
            "schema": {"type": "string"},
            "description": format!("Identifier of the parent {}.", parent.name),
        }),
        json!({
            "name": "limit",
            "in": "query",
            "schema": {
                "type": "integer",
                "minimum": 1,
                "maximum": sub.max_page_size,
                "default": sub.max_page_size,
            },
        }),
        json!({
            "name": "cursor",
            "in": "query",
            "schema": {"type": "string"},
            "description": "Opaque cursor from a previous page's next_cursor.",
        }),
    ];
    for column in &sub.filterable {
        parameters.push(json!({
            "name": column,
            "in": "query",
            "schema": {"type": "string"},
            "description": format!("Filter by {column} (indexed)."),
        }));
    }
    if !sub.sortable.is_empty() {
        let values: Vec<Value> = sub
            .sortable
            .iter()
            .flat_map(|column| {
                [
                    json!(format!("{column}:asc")),
                    json!(format!("{column}:desc")),
                ]
            })
            .collect();
        parameters.push(json!({
            "name": "sort",
            "in": "query",
            "schema": {"type": "string", "enum": values},
            "description": "Sort order; every value is backed by an index.",
        }));
    }
    let mut operation = json!({
        "operationId": format!(
            "list_{}_{}",
            sub.name.replace('-', "_"),
            parent.name.replace('-', "_"),
        ),
        "parameters": parameters,
        "responses": {
            "200": {
                "description": "Page of sub-resources.",
                "content": {"application/json": {"schema": {
                    "$ref": format!("#/components/schemas/{schema_name}Page"),
                }}},
            },
        },
    });
    if let Some(description) = &sub.description {
        operation["description"] = json!(description);
    }
    attach_scopes(&mut operation, &parent.reads_require);
    json!({ "get": operation })
}

fn resource_schema(resource: &Resource, table: &TableDefinition) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    // Every resource carries an opaque id.
    properties.insert("id".into(), json!({"type": "string"}));
    required.push(json!("id"));
    for exposure in &resource.fields {
        let field = table
            .fields
            .iter()
            .find(|f| f.name == exposure.column)
            .expect("validated: column exists");
        let mut schema = field_schema(field);
        if let Some(guard) = &exposure.guard {
            // Visible where the API is read: this field may be absent,
            // and here is the policy that decides.
            schema["x-guard"] = json!(guard);
        }
        properties.insert(exposure.api_name().to_owned(), schema);
        if !field.nullable && exposure.guard.is_none() {
            required.push(json!(exposure.api_name()));
        }
    }
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    })
}

/// Map a schema field to a JSON Schema fragment. Nullable columns
/// (`option<...>`) become type unions with `"null"`, the 3.1 idiom.
fn field_schema(field: &FieldDefinition) -> Value {
    let base: Value = match field.field_type {
        FieldType::String | FieldType::Record | FieldType::File => json!({"type": "string"}),
        FieldType::Int => json!({"type": "integer"}),
        FieldType::Float | FieldType::Decimal | FieldType::Number => json!({"type": "number"}),
        FieldType::Bool => json!({"type": "boolean"}),
        FieldType::Datetime => json!({"type": "string", "format": "date-time"}),
        FieldType::Duration => json!({"type": "string", "format": "duration"}),
        FieldType::Object | FieldType::Geometry => json!({"type": "object"}),
        FieldType::Array => json!({"type": "array"}),
        FieldType::Bytes => json!({"type": "string", "format": "byte"}),
        FieldType::Any => json!({}),
    };
    if !field.nullable {
        return base;
    }
    let mut wrapped = base;
    if let Some(t) = wrapped.get("type").cloned() {
        wrapped["type"] = json!([t, "null"]);
    }
    wrapped
}

/// The page envelope every list endpoint actually returns, the same
/// shape the SDL's `{Type}Page` and every generated client use.
fn page_schema(schema_name: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {"$ref": format!("#/components/schemas/{schema_name}")},
            },
            "next_cursor": {
                "type": ["string", "null"],
                "description": "Opaque keyset cursor for the next page; null when drained.",
            },
        },
        "required": ["items", "next_cursor"],
    })
}

fn list_path(resource: &Resource, schema_name: &str) -> Value {
    let mut parameters = vec![
        json!({
            "name": "limit",
            "in": "query",
            "schema": {
                "type": "integer",
                "minimum": 1,
                "maximum": resource.max_page_size,
                "default": resource.max_page_size,
            },
        }),
        json!({
            "name": "cursor",
            "in": "query",
            "schema": {"type": "string"},
            "description": "Opaque cursor from a previous page's next_cursor.",
        }),
    ];
    for column in &resource.filterable {
        parameters.push(json!({
            "name": column,
            "in": "query",
            "schema": {"type": "string"},
            "description": format!("Filter by {column} (indexed)."),
        }));
    }
    if !resource.sortable.is_empty() {
        parameters.push(json!({
            "name": "sort",
            "in": "query",
            "schema": {"type": "string", "enum": sort_values(resource)},
            "description": "Sort order; every value is backed by an index.",
        }));
    }
    let mut operation = json!({
        "operationId": format!("list_{}", resource.name.replace('-', "_")),
        "parameters": parameters,
        "responses": {
            "200": {
                "description": "Page of resources.",
                "content": {"application/json": {"schema": {
                    "$ref": format!("#/components/schemas/{schema_name}Page"),
                }}},
            },
        },
    });
    attach_scopes(&mut operation, &resource.reads_require);
    json!({ "get": operation })
}

/// Stamp the scopes an operation demands, when it demands any, so
/// authorization policy is visible where the API is read.
fn attach_scopes(operation: &mut Value, scopes: &[String]) {
    if !scopes.is_empty() {
        operation["x-requires-scopes"] = json!(scopes);
    }
}

fn sort_values(resource: &Resource) -> Vec<String> {
    resource
        .sortable
        .iter()
        .flat_map(|c| [c.clone(), format!("-{c}")])
        .collect()
}

fn type_ref_schema(kind: TypeRef) -> Value {
    match kind {
        TypeRef::String => json!({"type": "string"}),
        TypeRef::Int => json!({"type": "integer"}),
        TypeRef::Bool => json!({"type": "boolean"}),
        TypeRef::Json => json!({"type": "object"}),
    }
}

fn action_operation(resource: &Resource, action: &Action, schema_name: &str) -> Value {
    let mut operation = Map::new();
    operation.insert(
        "operationId".into(),
        json!(format!(
            "{}_{}",
            action.name,
            resource.name.replace('-', "_")
        )),
    );
    if let Some(description) = &action.description {
        operation.insert("description".into(), json!(description));
    }
    if action.takes_id() {
        operation.insert(
            "parameters".into(),
            json!([{
                "name": "id",
                "in": "path",
                "required": true,
                "schema": {"type": "string"},
            }]),
        );
    }
    if !action.input.is_empty() {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for field in &action.input {
            let mut schema = type_ref_schema(field.kind);
            if !field.options.is_empty() {
                schema["enum"] = json!(field.options);
            }
            if field.multiple {
                schema = json!({ "type": "array", "items": schema });
            }
            properties.insert(field.name.clone(), schema);
            if field.required {
                required.push(json!(field.name));
            }
        }
        operation.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": {"application/json": {"schema": {
                    "type": "object",
                    "properties": Value::Object(properties),
                    "required": required,
                }}},
            }),
        );
    }
    let responses = match action.output {
        ActionOutput::Resource => json!({
            "200": {
                "description": "The resource.",
                "content": {"application/json": {"schema": {
                    "$ref": format!("#/components/schemas/{schema_name}"),
                }}},
            },
        }),
        ActionOutput::Json => json!({
            "200": {
                "description": "Action result.",
                "content": {"application/json": {"schema": {"type": "object"}}},
            },
        }),
        ActionOutput::None => json!({
            "204": {"description": "No content."},
        }),
    };
    operation.insert("responses".into(), responses);
    if !action.requires.is_empty() {
        operation.insert("x-requires-scopes".into(), json!(action.requires));
    }
    Value::Object(operation)
}

fn get_path(resource: &Resource, schema_name: &str) -> Value {
    let mut operation = json!({
        "operationId": format!("get_{}", resource.name.replace('-', "_")),
        "parameters": [{
            "name": "id",
            "in": "path",
            "required": true,
            "schema": {"type": "string"},
        }],
        "responses": {
            "200": {
                "description": "The resource.",
                "content": {"application/json": {"schema": {
                    "$ref": format!("#/components/schemas/{schema_name}"),
                }}},
            },
            "404": {"description": "Not found."},
        },
    });
    attach_scopes(&mut operation, &resource.reads_require);
    json!({ "get": operation })
}

/// The OpenAPI type name for a wire type.
fn type_name(kind: crate::ir::TypeRef) -> &'static str {
    match kind {
        crate::ir::TypeRef::String => "string",
        crate::ir::TypeRef::Int => "integer",
        crate::ir::TypeRef::Bool => "boolean",
        crate::ir::TypeRef::Json => "object",
    }
}

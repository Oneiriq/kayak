//! GraphQL SDL generation, the second face over the same contract.
//!
//! Emission is deterministic (resources and fields in declaration
//! order, scalars once, alphabetical only where the IR imposes no
//! order) so the checked-in `.graphql` artifact diffs cleanly. Types
//! come from the schema's `FieldDefinition`s exactly as OpenAPI's do:
//! `option<...>` columns drop the `!`, renames apply everywhere, sort
//! enums list only index-backed orderings. Generation refuses invalid
//! contracts through the same gate.

use surql::schema::{FieldDefinition, FieldType, TableDefinition};

use crate::emit::wln;
use crate::ir::{Action, ActionOutput, Contract, Resource, TypeRef};
use crate::naming::camel;
use crate::openapi::GenerateError;
use crate::validate::validate;

/// Generate the SDL document for `contract` over `schema`.
pub fn generate_sdl(
    contract: &Contract,
    schema: &[TableDefinition],
) -> Result<String, GenerateError> {
    let violations = validate(contract, schema);
    if !violations.is_empty() {
        return Err(GenerateError::Invalid(violations));
    }

    let mut uses_datetime = false;
    let mut uses_json = false;
    let mut body = String::new();

    for resource in &contract.resources {
        let table = crate::resolve::table(schema, &resource.table)?;
        let type_name = resource.graphql_type_name();

        wln!(body, "type {type_name} {{");
        // The identity column, unless the resource exposes it as a field --
        // then the loop below declares it once, with its real type.
        let identity = resource.identity_column();
        if !resource.fields.iter().any(|f| f.api_name() == identity) {
            wln!(body, "  {identity}: ID!");
        }
        for exposure in &resource.fields {
            let field = crate::resolve::column(table, &exposure.column)?;
            let (gql, datetime, json) = graphql_type(field, exposure.guard.is_some());
            uses_datetime |= datetime;
            uses_json |= json;
            wln!(body, "  {}: {gql}", exposure.api_name());
        }
        // Sub-collections read as fields on their parent, which is the
        // only place they exist.
        for sub in &resource.sub_resources {
            wln!(
                body,
                "  {}: {}Page!",
                sub_field_signature(resource, sub),
                sub.graphql_type_name(resource),
            );
        }
        wln!(body, "}}\n");

        if !resource.sortable.is_empty() {
            wln!(body, "enum {type_name}Sort {{");
            for column in &resource.sortable {
                let upper = column.to_ascii_uppercase();
                wln!(body, "  {upper}_ASC");
                wln!(body, "  {upper}_DESC");
            }
            wln!(body, "}}\n");
        }

        wln!(body, "type {type_name}Page {{");
        wln!(body, "  items: [{type_name}!]!");
        wln!(body, "  nextCursor: String");
        wln!(body, "}}\n");

        // Sub-resource types and pages. The field that reaches them
        // lives on the parent object, printed above.
        for sub in &resource.sub_resources {
            let sub_table = crate::resolve::table(schema, &sub.table)?;
            let sub_type = sub.graphql_type_name(resource);
            wln!(body, "type {sub_type} {{");
            wln!(body, "  id: ID!");
            for exposure in &sub.fields {
                let field = crate::resolve::column(sub_table, &exposure.column)?;
                let (gql, datetime, json) = graphql_type(field, exposure.guard.is_some());
                uses_datetime |= datetime;
                uses_json |= json;
                wln!(body, "  {}: {gql}", exposure.api_name());
            }
            wln!(body, "}}\n");

            if !sub.sortable.is_empty() {
                wln!(body, "enum {sub_type}Sort {{");
                for column in &sub.sortable {
                    let upper = column.to_ascii_uppercase();
                    wln!(body, "  {upper}_ASC");
                    wln!(body, "  {upper}_DESC");
                }
                wln!(body, "}}\n");
            }

            wln!(body, "type {sub_type}Page {{");
            wln!(body, "  items: [{sub_type}!]!");
            wln!(body, "  nextCursor: String");
            wln!(body, "}}\n");
        }
    }

    // Query root.
    body.push_str("type Query {\n");
    for resource in &contract.resources {
        let type_name = resource.graphql_type_name();
        let field = resource.graphql_list_field();
        let singular = resource.graphql_get_field();
        let mut arguments = vec![
            format!("limit: Int = {}", resource.max_page_size),
            "cursor: String".to_owned(),
        ];
        for column in &resource.filterable {
            arguments.push(format!("{}: String", camel(column)));
        }
        if !resource.sortable.is_empty() {
            arguments.push(format!("sort: {type_name}Sort"));
        }
        if resource.faces.list {
            wln!(
                body,
                "  {field}({args}): {type_name}Page!",
                args = arguments.join(", "),
            );
        }
        if resource.faces.get {
            wln!(body, "  {singular}(id: ID!): {type_name}");
        }
    }
    // Contract queries join the same root: a question with typed
    // arguments answers as JSON, because the answer's shape is the
    // resolver's business rather than a projected table.
    for query in &contract.queries {
        let (field, json) = query_field(query);
        uses_json |= json;
        wln!(body, "  {field}");
    }
    body.push_str("}\n");

    // Mutation root, only when any resource declares actions.
    let has_actions = contract.resources.iter().any(|r| !r.actions.is_empty());
    if has_actions {
        body.push_str("\ntype Mutation {\n");
        for resource in &contract.resources {
            for action in &resource.actions {
                let (field, json) = mutation_field(resource, action);
                uses_json |= json;
                wln!(body, "  {field}");
            }
        }
        body.push_str("}\n");
    }

    // Subscription root, only when any resource is watchable. Watchers
    // take the filter arguments list takes, minus paging: a stream has
    // no page to size or cursor into.
    let has_watchers = contract.resources.iter().any(|r| r.watchable);
    if has_watchers {
        body.push_str("\ntype Subscription {\n");
        for resource in contract.resources.iter().filter(|r| r.watchable) {
            let type_name = resource.graphql_type_name();
            let field = resource.graphql_watch_field();
            let arguments = resource
                .filterable
                .iter()
                .map(|column| format!("{}: String", camel(column)))
                .collect::<Vec<_>>();
            if arguments.is_empty() {
                wln!(body, "  {field}: {type_name}!");
            } else {
                wln!(
                    body,
                    "  {field}({args}): {type_name}!",
                    args = arguments.join(", "),
                );
            }
        }
        body.push_str("}\n");
    }

    let mut document = String::new();
    if uses_datetime {
        document.push_str("scalar DateTime\n");
    }
    if uses_json {
        document.push_str("scalar JSON\n");
    }
    if !document.is_empty() {
        document.push('\n');
    }
    document.push_str(&body);
    Ok(document)
}

/// The sub-collection field on the parent, arguments included:
/// `versions(limit: Int = 100, cursor: String, sort: FileVersionSort)`.
/// Shared by the SDL printer and the dynamic schema so the two cannot
/// drift in argument order or defaults.
pub(crate) fn sub_field_signature(parent: &Resource, sub: &crate::ir::SubResource) -> String {
    let mut arguments = vec![
        format!("limit: Int = {}", sub.max_page_size),
        "cursor: String".to_owned(),
    ];
    for column in &sub.filterable {
        arguments.push(format!("{}: String", camel(column)));
    }
    if !sub.sortable.is_empty() {
        arguments.push(format!("sort: {}Sort", sub.graphql_type_name(parent)));
    }
    format!("{}({})", sub.graphql_field(), arguments.join(", "))
}

/// Map a schema field to (GraphQL type, uses_datetime, uses_json).
/// Nullable columns drop the `!`, and so do guarded ones: a field the
/// dispatcher may omit cannot promise to be present.
fn graphql_type(field: &FieldDefinition, guarded: bool) -> (String, bool, bool) {
    let (base, datetime, json) = match field.field_type {
        FieldType::String | FieldType::Record | FieldType::File => ("String", false, false),
        FieldType::Int => ("Int", false, false),
        FieldType::Float | FieldType::Decimal | FieldType::Number => ("Float", false, false),
        FieldType::Bool => ("Boolean", false, false),
        FieldType::Datetime => ("DateTime", true, false),
        FieldType::Duration => ("String", false, false),
        FieldType::Object | FieldType::Geometry | FieldType::Array | FieldType::Any => {
            ("JSON", false, true)
        }
        FieldType::Bytes => ("String", false, false),
    };
    let rendered = if field.nullable || guarded {
        base.to_owned()
    } else {
        format!("{base}!")
    };
    (rendered, datetime, json)
}

/// One Mutation field for an action; returns (line, uses_json).
/// One contract query as a GraphQL query field.
fn query_field(query: &crate::ir::Query) -> (String, bool) {
    let mut arguments = Vec::new();
    for field in &query.input {
        let base = match field.kind {
            TypeRef::String => "String",
            TypeRef::Int => "Int",
            TypeRef::Bool => "Boolean",
            TypeRef::Json => "JSON",
        };
        let bang = if field.required { "!" } else { "" };
        arguments.push(format!("{}: {base}{bang}", camel(&field.name)));
    }
    let name = query.graphql_field_name();
    let rendered = if arguments.is_empty() {
        format!("{name}: JSON!")
    } else {
        format!("{name}({}): JSON!", arguments.join(", "))
    };
    // A query always answers JSON, so the scalar is always in play.
    (rendered, true)
}

fn mutation_field(resource: &Resource, action: &Action) -> (String, bool) {
    let name = action.graphql_field_name(resource);
    let mut arguments = Vec::new();
    let mut uses_json = false;
    if action.takes_id() {
        arguments.push("id: ID!".to_owned());
    }
    for field in &action.input {
        let base = match field.kind {
            TypeRef::String => "String",
            TypeRef::Int => "Int",
            TypeRef::Bool => "Boolean",
            TypeRef::Json => {
                uses_json = true;
                "JSON"
            }
        };
        let bang = if field.required { "!" } else { "" };
        arguments.push(format!("{}: {base}{bang}", camel(&field.name)));
    }
    let output = match action.output {
        ActionOutput::Resource => format!("{}!", resource.graphql_type_name()),
        ActionOutput::Json => {
            uses_json = true;
            "JSON!".to_owned()
        }
        ActionOutput::None => "Boolean!".to_owned(),
    };
    let rendered = if arguments.is_empty() {
        format!("{name}: {output}")
    } else {
        format!("{name}({}): {output}", arguments.join(", "))
    };
    (rendered, uses_json)
}

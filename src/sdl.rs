//! GraphQL SDL generation, the second face over the same contract.
//!
//! Emission is deterministic (resources and fields in declaration
//! order, scalars once, alphabetical only where the IR imposes no
//! order) so the checked-in `.graphql` artifact diffs cleanly. Types
//! come from the schema's `FieldDefinition`s exactly as OpenAPI's do:
//! `option<...>` columns drop the `!`, renames apply everywhere, sort
//! enums list only index-backed orderings. Generation refuses invalid
//! contracts through the same gate.

use std::fmt::Write as _;

use surql::schema::{FieldDefinition, FieldType, TableDefinition};

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
        let table = schema
            .iter()
            .find(|t| t.name == resource.table)
            .expect("validated: table exists");
        let type_name = resource.graphql_type_name();

        writeln!(body, "type {type_name} {{").unwrap();
        writeln!(body, "  id: ID!").unwrap();
        for exposure in &resource.fields {
            let field = table
                .fields
                .iter()
                .find(|f| f.name == exposure.column)
                .expect("validated: column exists");
            let (gql, datetime, json) = graphql_type(field);
            uses_datetime |= datetime;
            uses_json |= json;
            writeln!(body, "  {}: {gql}", exposure.api_name()).unwrap();
        }
        // Sub-collections read as fields on their parent, which is the
        // only place they exist.
        for sub in &resource.sub_resources {
            writeln!(
                body,
                "  {}: {}Page!",
                sub_field_signature(resource, sub),
                sub.graphql_type_name(resource),
            )
            .unwrap();
        }
        writeln!(body, "}}\n").unwrap();

        if !resource.sortable.is_empty() {
            writeln!(body, "enum {type_name}Sort {{").unwrap();
            for column in &resource.sortable {
                let upper = column.to_ascii_uppercase();
                writeln!(body, "  {upper}_ASC").unwrap();
                writeln!(body, "  {upper}_DESC").unwrap();
            }
            writeln!(body, "}}\n").unwrap();
        }

        writeln!(body, "type {type_name}Page {{").unwrap();
        writeln!(body, "  items: [{type_name}!]!").unwrap();
        writeln!(body, "  nextCursor: String").unwrap();
        writeln!(body, "}}\n").unwrap();

        // Sub-resource types and pages. The field that reaches them
        // lives on the parent object, printed above.
        for sub in &resource.sub_resources {
            let sub_table = schema
                .iter()
                .find(|t| t.name == sub.table)
                .expect("validated: table exists");
            let sub_type = sub.graphql_type_name(resource);
            writeln!(body, "type {sub_type} {{").unwrap();
            writeln!(body, "  id: ID!").unwrap();
            for exposure in &sub.fields {
                let field = sub_table
                    .fields
                    .iter()
                    .find(|f| f.name == exposure.column)
                    .expect("validated: column exists");
                let (gql, datetime, json) = graphql_type(field);
                uses_datetime |= datetime;
                uses_json |= json;
                writeln!(body, "  {}: {gql}", exposure.api_name()).unwrap();
            }
            writeln!(body, "}}\n").unwrap();

            if !sub.sortable.is_empty() {
                writeln!(body, "enum {sub_type}Sort {{").unwrap();
                for column in &sub.sortable {
                    let upper = column.to_ascii_uppercase();
                    writeln!(body, "  {upper}_ASC").unwrap();
                    writeln!(body, "  {upper}_DESC").unwrap();
                }
                writeln!(body, "}}\n").unwrap();
            }

            writeln!(body, "type {sub_type}Page {{").unwrap();
            writeln!(body, "  items: [{sub_type}!]!").unwrap();
            writeln!(body, "  nextCursor: String").unwrap();
            writeln!(body, "}}\n").unwrap();
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
        writeln!(
            body,
            "  {field}({args}): {type_name}Page!",
            args = arguments.join(", "),
        )
        .unwrap();
        writeln!(body, "  {singular}(id: ID!): {type_name}").unwrap();
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
                writeln!(body, "  {field}").unwrap();
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
                writeln!(body, "  {field}: {type_name}!").unwrap();
            } else {
                writeln!(
                    body,
                    "  {field}({args}): {type_name}!",
                    args = arguments.join(", "),
                )
                .unwrap();
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
/// Nullable columns drop the `!`.
fn graphql_type(field: &FieldDefinition) -> (String, bool, bool) {
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
    let rendered = if field.nullable {
        base.to_owned()
    } else {
        format!("{base}!")
    };
    (rendered, datetime, json)
}

/// One Mutation field for an action; returns (line, uses_json).
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

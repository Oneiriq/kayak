//! The live GraphQL face: an `async-graphql` dynamic schema built from
//! the same IR the SDL generator prints.
//!
//! Both faces derive from one contract object, so the checked-in
//! `.graphql` artifact and the served schema cannot disagree. Every
//! field resolver funnels through the [`Dispatcher`], so the middleware
//! chain and contract enforcement run identically for GraphQL and REST.
//!
//! Rows travel as `serde_json::Value`s: resolvers return wire-shaped
//! JSON, object fields pluck their key from the parent value. The
//! per-request [`JanusContext`] rides on the request data
//! (`Request::new(query).data(ctx)`); a missing context is an EMPTY
//! context, so context-requiring middleware still rejects.

use std::sync::Arc;

use async_graphql::dynamic::{
    Enum, Field, FieldFuture, FieldValue, InputValue, Object, Scalar, Schema, SchemaBuilder,
    TypeRef as Gql,
};
use async_graphql::Value as GqlValue;
use surql::schema::{FieldType, TableDefinition};

use crate::ir::{ActionOutput, TypeRef};
use crate::runtime::args::{ActionArgs, GetArgs, ListArgs, SortDirection};
use crate::runtime::context::JanusContext;
use crate::runtime::dispatch::Dispatcher;
use crate::runtime::error::JanusError;
use crate::validate::{validate, Violation};

/// Why the schema could not be built.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlBuildError {
    #[error("contract is invalid:\n{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<Violation>),
    #[error("schema registration failed: {0}")]
    Schema(String),
}

/// Build the executable schema with default settings.
pub fn build_schema(
    schema: &[TableDefinition],
    dispatcher: Arc<Dispatcher>,
) -> Result<Schema, GraphqlBuildError> {
    schema_builder(schema, dispatcher)?
        .finish()
        .map_err(|e| GraphqlBuildError::Schema(e.to_string()))
}

/// Build the schema BUILDER, the plugin seam. Callers may attach
/// `async-graphql` extensions, depth/complexity limits, or global data
/// before finishing:
///
/// ```ignore
/// let schema = schema_builder(&tables, dispatcher)?
///     .limit_depth(8)
///     .extension(Tracing)
///     .finish()?;
/// ```
pub fn schema_builder(
    schema: &[TableDefinition],
    dispatcher: Arc<Dispatcher>,
) -> Result<SchemaBuilder, GraphqlBuildError> {
    let contract = dispatcher.contract().clone();
    let violations = validate(&contract, schema);
    if !violations.is_empty() {
        return Err(GraphqlBuildError::Invalid(violations));
    }

    let mut uses_datetime = false;
    let mut uses_json = false;
    let has_actions = contract.resources.iter().any(|r| !r.actions.is_empty());

    let mut builder = Schema::build("Query", has_actions.then_some("Mutation"), None);
    let mut query = Object::new("Query");
    let mut mutation = Object::new("Mutation");

    for resource in &contract.resources {
        let table = schema
            .iter()
            .find(|t| t.name == resource.table)
            .expect("validated: table exists");
        let type_name = resource.graphql_type_name();
        let page_name = format!("{type_name}Page");
        let sort_name = format!("{type_name}Sort");

        // The object type: id plus every exposed field, each plucking
        // its key from the parent row.
        let mut object = Object::new(&type_name);
        object = object.field(Field::new("id", Gql::named_nn(Gql::ID), |ctx| {
            FieldFuture::new(async move {
                let row = parent_row(&ctx)?;
                Ok(row
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|id| FieldValue::value(GqlValue::String(id.to_owned()))))
            })
        }));
        for exposure in &resource.fields {
            let field_def = table
                .fields
                .iter()
                .find(|f| f.name == exposure.column)
                .expect("validated: column exists");
            let (base, datetime, json) = graphql_scalar(&field_def.field_type);
            uses_datetime |= datetime;
            uses_json |= json;
            let type_ref = if field_def.nullable {
                Gql::named(base)
            } else {
                Gql::named_nn(base)
            };
            let api_name = exposure.api_name().to_owned();
            object = object.field(Field::new(api_name.clone(), type_ref, move |ctx| {
                let key = api_name.clone();
                FieldFuture::new(async move {
                    let row = parent_row(&ctx)?;
                    field_from_row(row, &key)
                })
            }));
        }
        builder = builder.register(object);

        // The page type over owned JSON rows.
        let item_type = type_name.clone();
        let mut page = Object::new(&page_name);
        page = page.field(Field::new(
            "items",
            Gql::named_nn_list_nn(&item_type),
            |ctx| {
                FieldFuture::new(async move {
                    let row = parent_row(&ctx)?;
                    let items = row
                        .get("items")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    Ok(Some(FieldValue::list(
                        items.into_iter().map(FieldValue::owned_any),
                    )))
                })
            },
        ));
        page = page.field(Field::new("nextCursor", Gql::named(Gql::STRING), |ctx| {
            FieldFuture::new(async move {
                let row = parent_row(&ctx)?;
                field_from_row(row, "next_cursor")
            })
        }));
        builder = builder.register(page);

        // The sort enum, mirroring the SDL exactly.
        if !resource.sortable.is_empty() {
            let mut sort_enum = Enum::new(&sort_name);
            for column in &resource.sortable {
                let upper = column.to_ascii_uppercase();
                sort_enum = sort_enum.item(format!("{upper}_ASC"));
                sort_enum = sort_enum.item(format!("{upper}_DESC"));
            }
            builder = builder.register(sort_enum);
        }

        // Query: the list field.
        let list_dispatcher = dispatcher.clone();
        let list_resource = resource.name.clone();
        let filterable = resource.filterable.clone();
        let sortable = resource.sortable.clone();
        let mut list_field = Field::new(
            resource.graphql_list_field(),
            Gql::named_nn(&page_name),
            move |ctx| {
                let dispatcher = list_dispatcher.clone();
                let resource = list_resource.clone();
                let filterable = filterable.clone();
                let sortable = sortable.clone();
                FieldFuture::new(async move {
                    let mut args = ListArgs {
                        limit: ctx
                            .args
                            .get("limit")
                            .and_then(|v| v.u64().ok())
                            .and_then(|v| u32::try_from(v).ok())
                            .unwrap_or(u32::MAX),
                        cursor: ctx
                            .args
                            .get("cursor")
                            .and_then(|v| v.string().ok().map(str::to_owned)),
                        ..Default::default()
                    };
                    for column in &filterable {
                        if let Some(value) = ctx.args.get(crate::naming::camel(column).as_str()) {
                            args.filters.insert(
                                column.clone(),
                                serde_json::Value::String(value.string()?.to_owned()),
                            );
                        }
                    }
                    if let Some(value) = ctx.args.get("sort") {
                        args.sort = Some(parse_sort(value.enum_name()?, &sortable)?);
                    }
                    let jctx = request_context(&ctx);
                    let output = dispatcher
                        .list(&resource, jctx, args)
                        .await
                        .map_err(to_graphql_error)?;
                    Ok(Some(FieldValue::owned_any(serde_json::json!({
                        "items": output.items,
                        "next_cursor": output.next_cursor,
                    }))))
                })
            },
        )
        .argument(
            InputValue::new("limit", Gql::named(Gql::INT))
                .default_value(GqlValue::from(resource.max_page_size)),
        )
        .argument(InputValue::new("cursor", Gql::named(Gql::STRING)));
        for column in &resource.filterable {
            list_field = list_field.argument(InputValue::new(
                crate::naming::camel(column),
                Gql::named(Gql::STRING),
            ));
        }
        if !resource.sortable.is_empty() {
            list_field = list_field.argument(InputValue::new("sort", Gql::named(&sort_name)));
        }
        query = query.field(list_field);

        // Query: the get field.
        let get_dispatcher = dispatcher.clone();
        let get_resource = resource.name.clone();
        query = query.field(
            Field::new(
                resource.graphql_get_field(),
                Gql::named(&type_name),
                move |ctx| {
                    let dispatcher = get_dispatcher.clone();
                    let resource = get_resource.clone();
                    FieldFuture::new(async move {
                        let id = ctx.args.try_get("id")?.string()?.to_owned();
                        let jctx = request_context(&ctx);
                        let row = dispatcher
                            .get(&resource, jctx, GetArgs { id })
                            .await
                            .map_err(to_graphql_error)?;
                        Ok(row.map(FieldValue::owned_any))
                    })
                },
            )
            .argument(InputValue::new("id", Gql::named_nn(Gql::ID))),
        );

        // Mutation: one field per action.
        for action in &resource.actions {
            let output_type = match action.output {
                ActionOutput::Resource => Gql::named_nn(&type_name),
                ActionOutput::Json => {
                    uses_json = true;
                    Gql::named_nn("JSON")
                }
                ActionOutput::None => Gql::named_nn(Gql::BOOLEAN),
            };
            let action_dispatcher = dispatcher.clone();
            let action_resource = resource.name.clone();
            let action_name = action.name.clone();
            let takes_id = action.takes_id();
            let inputs: Vec<String> = action.input.iter().map(|f| f.name.clone()).collect();
            let output_kind = action.output;
            let mut field = Field::new(
                action.graphql_field_name(resource),
                output_type,
                move |ctx| {
                    let dispatcher = action_dispatcher.clone();
                    let resource = action_resource.clone();
                    let action = action_name.clone();
                    let inputs = inputs.clone();
                    FieldFuture::new(async move {
                        let mut args = ActionArgs::default();
                        if takes_id {
                            args.id = Some(ctx.args.try_get("id")?.string()?.to_owned());
                        }
                        for input in &inputs {
                            if let Some(value) = ctx.args.get(crate::naming::camel(input).as_str())
                            {
                                args.input.insert(
                                    input.clone(),
                                    value.deserialize::<serde_json::Value>()?,
                                );
                            }
                        }
                        let jctx = request_context(&ctx);
                        let value = dispatcher
                            .action(&resource, &action, jctx, args)
                            .await
                            .map_err(to_graphql_error)?;
                        Ok(Some(match (output_kind, value) {
                            (ActionOutput::None, _) => FieldValue::value(GqlValue::Boolean(true)),
                            (ActionOutput::Resource, Some(row)) => FieldValue::owned_any(row),
                            (ActionOutput::Json, Some(json)) => {
                                FieldValue::value(GqlValue::from_json(json)?)
                            }
                            (_, None) => {
                                return Err(to_graphql_error(JanusError::Internal(
                                    "action resolver returned no value for a value-bearing output"
                                        .into(),
                                )));
                            }
                        }))
                    })
                },
            );
            if takes_id {
                field = field.argument(InputValue::new("id", Gql::named_nn(Gql::ID)));
            }
            for input in &action.input {
                let base = match input.kind {
                    TypeRef::String => Gql::STRING.to_owned(),
                    TypeRef::Int => Gql::INT.to_owned(),
                    TypeRef::Bool => Gql::BOOLEAN.to_owned(),
                    TypeRef::Json => {
                        uses_json = true;
                        "JSON".to_owned()
                    }
                };
                let type_ref = if input.required {
                    Gql::named_nn(base)
                } else {
                    Gql::named(base)
                };
                field =
                    field.argument(InputValue::new(crate::naming::camel(&input.name), type_ref));
            }
            mutation = mutation.field(field);
        }
    }

    if uses_datetime {
        builder = builder.register(Scalar::new("DateTime"));
    }
    if uses_json {
        builder = builder.register(Scalar::new("JSON"));
    }
    builder = builder.register(query);
    if has_actions {
        builder = builder.register(mutation);
    }
    Ok(builder)
}

/// The per-request context, or empty when the caller injected none;
/// context-requiring middleware then rejects, which is the safe
/// default.
fn request_context(ctx: &async_graphql::dynamic::ResolverContext<'_>) -> JanusContext {
    ctx.data_opt::<JanusContext>().cloned().unwrap_or_default()
}

/// Downcast the parent value to the JSON row every object resolver
/// expects.
fn parent_row<'a>(
    ctx: &'a async_graphql::dynamic::ResolverContext<'_>,
) -> async_graphql::Result<&'a serde_json::Value> {
    ctx.parent_value.try_downcast_ref::<serde_json::Value>()
}

/// Pluck one key from a row and convert it to a GraphQL value.
fn field_from_row(
    row: &serde_json::Value,
    key: &str,
) -> async_graphql::Result<Option<FieldValue<'static>>> {
    match row.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => Ok(Some(FieldValue::value(GqlValue::from_json(value.clone())?))),
    }
}

/// `CREATED_AT_DESC` -> `("created_at", Desc)`, validated against the
/// declared sortable columns.
fn parse_sort(name: &str, sortable: &[String]) -> async_graphql::Result<(String, SortDirection)> {
    let (stem, direction) = match name.strip_suffix("_DESC") {
        Some(stem) => (stem, SortDirection::Desc),
        None => match name.strip_suffix("_ASC") {
            Some(stem) => (stem, SortDirection::Asc),
            None => return Err(format!("unknown sort {name}").into()),
        },
    };
    let column = sortable
        .iter()
        .find(|c| c.to_ascii_uppercase() == stem)
        .ok_or_else(|| async_graphql::Error::new(format!("unknown sort {name}")))?;
    Ok((column.clone(), direction))
}

/// Carry the runtime error's stable code into GraphQL error extensions.
fn to_graphql_error(error: JanusError) -> async_graphql::Error {
    let mut out = async_graphql::Error::new(error.to_string());
    let mut extensions = async_graphql::ErrorExtensionValues::default();
    extensions.set("code", error.code());
    out.extensions = Some(extensions);
    out
}

/// Map a schema field type to its GraphQL scalar, mirroring the SDL
/// generator exactly. Returns (name, uses_datetime, uses_json).
fn graphql_scalar(field_type: &FieldType) -> (&'static str, bool, bool) {
    match field_type {
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
    }
}

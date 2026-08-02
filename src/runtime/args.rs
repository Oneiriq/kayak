//! Operation arguments, validated against the contract BEFORE any
//! middleware or resolver runs. A resolver can trust what it receives:
//! the limit is clamped, every filter is allowlisted, the sort is
//! declared, action inputs are present and correctly typed.

use std::collections::BTreeMap;

use crate::ir::{Action, Resource, TypeRef};
use crate::runtime::error::JanusError;

/// Direction for a declared sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Arguments to a list operation. `filters` is keyed by COLUMN name
/// (the contract's vocabulary), whatever the protocol called it.
#[derive(Debug, Clone, Default)]
pub struct ListArgs {
    pub limit: u32,
    pub cursor: Option<String>,
    pub filters: BTreeMap<String, serde_json::Value>,
    pub sort: Option<(String, SortDirection)>,
}

/// Arguments to a get operation.
#[derive(Debug, Clone)]
pub struct GetArgs {
    pub id: String,
}

/// Arguments to a sub-list operation: the parent instance, plus the
/// same paging and narrowing a list takes.
#[derive(Debug, Clone, Default)]
pub struct SubListArgs {
    /// The parent id the collection hangs off. Always present; a
    /// sub-resource has no collection-wide form.
    pub parent_id: String,
    pub limit: u32,
    pub cursor: Option<String>,
    pub filters: BTreeMap<String, serde_json::Value>,
    pub sort: Option<(String, SortDirection)>,
}

/// Arguments to a watch operation: the filters narrowing the stream,
/// keyed by COLUMN name like [`ListArgs::filters`]. There is no limit
/// or cursor; a stream is not a page.
#[derive(Debug, Clone, Default)]
pub struct WatchArgs {
    pub filters: BTreeMap<String, serde_json::Value>,
}

/// Arguments to an action: the instance id when the action targets
/// one, plus the declared inputs.
#[derive(Debug, Clone, Default)]
pub struct ActionArgs {
    pub id: Option<String>,
    pub input: serde_json::Map<String, serde_json::Value>,
}

/// What a contract query receives: its declared parameters, already
/// checked against the declaration.
#[derive(Debug, Clone, Default)]
pub struct QueryArgs {
    pub input: serde_json::Map<String, serde_json::Value>,
}

/// What a list resolver returns: one page in wire shape.
#[derive(Debug, Clone, Default)]
pub struct ListOutput {
    pub items: Vec<serde_json::Value>,
    pub next_cursor: Option<String>,
}

/// Clamp and check list arguments against the resource's declarations.
pub(crate) fn validate_list(resource: &Resource, args: &mut ListArgs) -> Result<(), JanusError> {
    args.limit = args.limit.clamp(1, resource.max_page_size);
    for column in args.filters.keys() {
        if !resource.filterable.iter().any(|c| c == column) {
            return Err(JanusError::BadRequest(format!(
                "filtering on {column} is not allowed",
            )));
        }
    }
    if let Some((column, _)) = &args.sort {
        if !resource.sortable.iter().any(|c| c == column) {
            return Err(JanusError::BadRequest(format!(
                "sorting on {column} is not allowed",
            )));
        }
    }
    Ok(())
}

/// Clamp and check sub-list arguments against the sub-resource's own
/// declarations, which are separate from the parent's.
pub(crate) fn validate_sub_list(
    sub: &crate::ir::SubResource,
    args: &mut SubListArgs,
) -> Result<(), JanusError> {
    if args.parent_id.is_empty() {
        return Err(JanusError::BadRequest(format!(
            "{} is reached through a parent id",
            sub.name,
        )));
    }
    args.limit = args.limit.clamp(1, sub.max_page_size);
    for column in args.filters.keys() {
        if !sub.filterable.iter().any(|c| c == column) {
            return Err(JanusError::BadRequest(format!(
                "filtering on {column} is not allowed",
            )));
        }
    }
    if let Some((column, _)) = &args.sort {
        if !sub.sortable.iter().any(|c| c == column) {
            return Err(JanusError::BadRequest(format!(
                "sorting on {column} is not allowed",
            )));
        }
    }
    Ok(())
}

/// Check watch arguments against the resource's declarations. A
/// resource that never declared itself watchable refuses here, so a
/// protocol layer that offers the field by mistake cannot open a
/// stream.
pub(crate) fn validate_watch(resource: &Resource, args: &WatchArgs) -> Result<(), JanusError> {
    if !resource.watchable {
        return Err(JanusError::BadRequest(format!(
            "{} cannot be watched",
            resource.name,
        )));
    }
    for column in args.filters.keys() {
        if !resource.filterable.iter().any(|c| c == column) {
            return Err(JanusError::BadRequest(format!(
                "filtering on {column} is not allowed",
            )));
        }
    }
    Ok(())
}

/// Check action arguments: instance id presence, required inputs,
/// input types. Unknown input keys are dropped; the
/// differ promises that removing an optional input is compatible, and
/// that only holds if servers ignore fields they no longer declare.
/// Check a query's parameters against its declaration: required ones
/// present, declared types honoured, undeclared ones refused. The
/// same discipline actions get, because a query is a wire surface
/// like any other.
pub(crate) fn validate_query(
    query: &crate::ir::Query,
    args: &mut QueryArgs,
) -> Result<(), JanusError> {
    let mut checked = serde_json::Map::new();
    for field in &query.input {
        match args.input.get(&field.name) {
            None | Some(serde_json::Value::Null) if field.required => {
                return Err(JanusError::BadRequest(format!(
                    "input {} is required",
                    field.name,
                )));
            }
            None | Some(serde_json::Value::Null) => {}
            Some(value) => {
                let ok = match field.kind {
                    TypeRef::String => value.is_string(),
                    TypeRef::Int => value.is_i64() || value.is_u64(),
                    TypeRef::Bool => value.is_boolean(),
                    TypeRef::Json => true,
                };
                if !ok {
                    return Err(JanusError::BadRequest(format!(
                        "input {} has the wrong type",
                        field.name,
                    )));
                }
                checked.insert(field.name.clone(), value.clone());
            }
        }
    }
    // Undeclared parameters are refused rather than ignored: a caller
    // who thinks a filter applies deserves to hear that it does not.
    for name in args.input.keys() {
        if !query.input.iter().any(|f| &f.name == name) {
            return Err(JanusError::BadRequest(format!("no input named {name}")));
        }
    }
    args.input = checked;
    Ok(())
}

pub(crate) fn validate_action(action: &Action, args: &mut ActionArgs) -> Result<(), JanusError> {
    if action.takes_id() && args.id.is_none() {
        return Err(JanusError::BadRequest(format!(
            "action {} targets an instance and requires an id",
            action.name,
        )));
    }

    let mut checked = serde_json::Map::new();
    for field in &action.input {
        match args.input.get(&field.name) {
            None | Some(serde_json::Value::Null) if field.required => {
                return Err(JanusError::BadRequest(format!(
                    "input {} is required",
                    field.name,
                )));
            }
            None | Some(serde_json::Value::Null) => {}
            Some(value) => {
                let ok = match field.kind {
                    TypeRef::String => value.is_string(),
                    TypeRef::Int => value.is_i64() || value.is_u64(),
                    TypeRef::Bool => value.is_boolean(),
                    TypeRef::Json => true,
                };
                if !ok {
                    return Err(JanusError::BadRequest(format!(
                        "input {} must be a {}",
                        field.name,
                        match field.kind {
                            TypeRef::String => "string",
                            TypeRef::Int => "integer",
                            TypeRef::Bool => "boolean",
                            TypeRef::Json => "value",
                        },
                    )));
                }
                checked.insert(field.name.clone(), value.clone());
            }
        }
    }
    args.input = checked;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ActionField;

    fn resource() -> Resource {
        Resource {
            name: "files".into(),
            table: "file".into(),
            fields: vec![crate::ir::FieldExposure::column("path")],
            pinned: vec![],
            filterable: vec!["state".into()],
            sortable: vec!["created_at".into()],
            max_page_size: 100,
            actions: vec![],
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
        }
    }

    fn action() -> Action {
        Action {
            name: "issue_url".into(),
            method: "POST".into(),
            path: "/{id}/url".into(),
            input: vec![
                ActionField {
                    name: "ttl_secs".into(),
                    kind: TypeRef::Int,
                    required: true,
                    description: None,
                },
                ActionField {
                    name: "note".into(),
                    kind: TypeRef::String,
                    required: false,
                    description: None,
                },
            ],
            output: crate::ir::ActionOutput::Json,
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        }
    }

    #[test]
    fn limits_clamp_and_allowlists_enforce() {
        let resource = resource();
        let mut args = ListArgs {
            limit: 5000,
            ..Default::default()
        };
        validate_list(&resource, &mut args).unwrap();
        assert_eq!(args.limit, 100);

        args.filters.insert("digest".into(), "x".into());
        let error = validate_list(&resource, &mut args).unwrap_err();
        assert!(error.to_string().contains("digest"), "{error}");

        let mut args = ListArgs {
            limit: 10,
            sort: Some(("state".into(), SortDirection::Asc)),
            ..Default::default()
        };
        let error = validate_list(&resource, &mut args).unwrap_err();
        assert!(error.to_string().contains("sorting on state"), "{error}");
    }

    #[test]
    fn action_inputs_are_typed_and_unknowns_drop() {
        let action = action();

        let mut args = ActionArgs {
            id: Some("01ABC".into()),
            input: serde_json::json!({
                "ttl_secs": 300,
                "stray": "ignored",
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        validate_action(&action, &mut args).unwrap();
        assert!(!args.input.contains_key("stray"));
        assert_eq!(args.input["ttl_secs"], 300);

        let mut missing = ActionArgs {
            id: Some("01ABC".into()),
            ..Default::default()
        };
        let error = validate_action(&action, &mut missing).unwrap_err();
        assert!(
            error.to_string().contains("ttl_secs is required"),
            "{error}"
        );

        let mut wrong = ActionArgs {
            id: Some("01ABC".into()),
            input: serde_json::json!({ "ttl_secs": "soon" })
                .as_object()
                .unwrap()
                .clone(),
        };
        let error = validate_action(&action, &mut wrong).unwrap_err();
        assert!(error.to_string().contains("must be a integer"), "{error}");

        let mut anonymous = ActionArgs::default();
        let error = validate_action(&action, &mut anonymous).unwrap_err();
        assert!(error.to_string().contains("requires an id"), "{error}");
    }
}

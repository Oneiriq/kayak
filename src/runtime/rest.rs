//! The REST face of the runtime: contract-declared routes answered
//! by the same dispatcher every other face uses.
//!
//! A route exists because the contract declares it: collection
//! listings, instance gets, sub-collections, actions, and contract
//! queries all derive from the declaration, with the same path
//! formulas the OpenAPI document renders. Content faces stream bytes
//! and stay with the host; everything JSON routes here.
//!
//! The router is transport-free: the host hands in a method, a path,
//! a query string, an optional parsed body, and a seeded
//! [`KayakContext`], and receives a status plus a JSON body. Scope
//! checks, argument validation, rate classes, and guards all run in
//! the dispatcher, exactly as they do for GraphQL and MCP callers.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::ir::TypeRef;
use crate::runtime::args::{ActionArgs, GetArgs, ListArgs, QueryArgs, SortDirection, SubListArgs};
use crate::runtime::context::KayakContext;
use crate::runtime::dispatch::Dispatcher;
use crate::runtime::error::KayakError;
use crate::runtime::wire::percent_decode;

/// One answered request: an HTTP status and a JSON body.
#[derive(Debug, Clone)]
pub struct RestAnswer {
    pub status: u16,
    pub body: Value,
}

fn answer(status: u16, body: Value) -> RestAnswer {
    RestAnswer { status, body }
}

fn refusal(error: &KayakError) -> RestAnswer {
    let status = match error {
        KayakError::BadRequest(_) => 400,
        KayakError::Unauthorized(_) => 401,
        KayakError::Forbidden(_) => 403,
        KayakError::NotFound => 404,
        KayakError::Conflict(_) => 409,
        KayakError::PayloadTooLarge(_) => 413,
        KayakError::TooManyRequests(_) => 429,
        _ => 500,
    };
    answer(status, json!({ "error": error.to_string() }))
}

enum Segment {
    Literal(String),
    Param(String),
}

enum Target {
    List { resource: String },
    Get { resource: String },
    SubList { resource: String, sub: String },
    Action { resource: String, action: String },
    Query { name: String },
}

struct Route {
    method: String,
    segments: Vec<Segment>,
    target: Target,
}

fn template(path: &str) -> Vec<Segment> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(
            |part| match part.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                Some(name) => Segment::Param(name.to_owned()),
                None => Segment::Literal(part.to_owned()),
            },
        )
        .collect()
}

/// The runtime REST router over one dispatcher.
pub struct RestRouter {
    dispatcher: Arc<Dispatcher>,
    routes: Vec<Route>,
}

impl RestRouter {
    /// Derive the route table from the dispatcher's contract. The
    /// formulas are the OpenAPI document's: `{prefix}/{plural}` lists,
    /// `{prefix}/{plural}/{id}` gets, `{prefix}/{plural}/{id}/{sub}`
    /// walks a sub-collection, `{prefix}/{plural}{action.path}`
    /// performs an action, and a query serves at its own declared path.
    /// The prefix is the contract's `api_prefix`, `/v1` by default.
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        let contract = dispatcher.contract().clone();
        let prefix = contract.prefix();
        let mut routes = Vec::new();
        for resource in &contract.resources {
            // A face the resource withholds gets no route, so the path
            // answers 404 the way an undeclared one does.
            if resource.faces.list {
                routes.push(Route {
                    method: "GET".to_owned(),
                    segments: template(&format!("{prefix}/{}", resource.name)),
                    target: Target::List {
                        resource: resource.name.clone(),
                    },
                });
            }
            if resource.faces.get {
                routes.push(Route {
                    method: "GET".to_owned(),
                    segments: template(&format!("{prefix}/{}/{{id}}", resource.name)),
                    target: Target::Get {
                        resource: resource.name.clone(),
                    },
                });
            }
            for sub in &resource.sub_resources {
                routes.push(Route {
                    method: "GET".to_owned(),
                    segments: template(&format!("{prefix}/{}/{{id}}/{}", resource.name, sub.name)),
                    target: Target::SubList {
                        resource: resource.name.clone(),
                        sub: sub.name.clone(),
                    },
                });
            }
            for action in &resource.actions {
                routes.push(Route {
                    method: action.method.to_uppercase(),
                    segments: template(&format!("{prefix}/{}{}", resource.name, action.path)),
                    target: Target::Action {
                        resource: resource.name.clone(),
                        action: action.name.clone(),
                    },
                });
            }
        }
        for query in &contract.queries {
            routes.push(Route {
                method: "GET".to_owned(),
                segments: template(&query.path),
                target: Target::Query {
                    name: query.name.clone(),
                },
            });
        }
        Self { dispatcher, routes }
    }

    /// Answer one request. `path` is the full request path (the
    /// contract's own prefix included), `query` the raw query string
    /// without the `?`, `body` the parsed JSON body when one arrived.
    /// Never errs: every failure is a status-shaped answer.
    pub async fn handle(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: Option<Value>,
        ctx: KayakContext,
    ) -> RestAnswer {
        let actual: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // Best match wins by literal count, so a literal tail beats a
        // parameter at the same position and declaration order never
        // matters.
        let mut best: Option<(&Route, BTreeMap<String, String>, usize)> = None;
        let mut path_known = false;
        for route in &self.routes {
            if route.segments.len() != actual.len() {
                continue;
            }
            let mut params = BTreeMap::new();
            let mut literals = 0usize;
            let mut matched = true;
            for (segment, part) in route.segments.iter().zip(&actual) {
                match segment {
                    Segment::Literal(literal) => {
                        if literal == part {
                            literals += 1;
                        } else {
                            matched = false;
                            break;
                        }
                    }
                    Segment::Param(name) => {
                        params.insert(name.clone(), percent_decode(part));
                    }
                }
            }
            if !matched {
                continue;
            }
            path_known = true;
            if !route.method.eq_ignore_ascii_case(method) {
                continue;
            }
            if best.as_ref().is_none_or(|(_, _, b)| literals > *b) {
                best = Some((route, params, literals));
            }
        }
        let Some((route, params, _)) = best else {
            return if path_known {
                answer(405, json!({ "error": "method not allowed on this path" }))
            } else {
                answer(404, json!({ "error": "no declared route matches" }))
            };
        };

        let mut pairs = parse_query(query);
        match &route.target {
            Target::List { resource } => {
                let args = match list_args(&mut pairs) {
                    Ok(args) => args,
                    Err(reply) => return reply,
                };
                match self.dispatcher.list(resource, ctx, args).await {
                    Ok(output) => answer(
                        200,
                        json!({ "items": output.items, "next_cursor": output.next_cursor }),
                    ),
                    Err(error) => refusal(&error),
                }
            }
            Target::Get { resource } => {
                let id = params.get("id").cloned().unwrap_or_default();
                match self.dispatcher.get(resource, ctx, GetArgs { id }).await {
                    Ok(Some(row)) => answer(200, row),
                    Ok(None) => refusal(&KayakError::NotFound),
                    Err(error) => refusal(&error),
                }
            }
            Target::SubList { resource, sub } => {
                let list = match list_args(&mut pairs) {
                    Ok(args) => args,
                    Err(reply) => return reply,
                };
                let args = SubListArgs {
                    parent_id: params.get("id").cloned().unwrap_or_default(),
                    limit: list.limit,
                    cursor: list.cursor,
                    filters: list.filters,
                    sort: list.sort,
                };
                match self.dispatcher.sub_list(resource, sub, ctx, args).await {
                    Ok(output) => answer(
                        200,
                        json!({ "items": output.items, "next_cursor": output.next_cursor }),
                    ),
                    Err(error) => refusal(&error),
                }
            }
            Target::Action { resource, action } => {
                let input = match body {
                    Some(Value::Object(map)) => map,
                    Some(Value::Null) | None => Map::new(),
                    Some(_) => {
                        return answer(400, json!({ "error": "body must be a JSON object" }));
                    }
                };
                let args = ActionArgs {
                    id: params.get("id").cloned(),
                    input,
                };
                match self.dispatcher.action(resource, action, ctx, args).await {
                    Ok(Some(value)) => answer(200, value),
                    Ok(None) => answer(200, json!({ "ok": true })),
                    Err(error) => refusal(&error),
                }
            }
            Target::Query { name } => {
                let declared = self
                    .dispatcher
                    .contract()
                    .queries
                    .iter()
                    .find(|q| q.name == *name);
                let mut input = Map::new();
                // A parameter the path names arrives in the path, the way
                // the OpenAPI document and every client send it. Chained
                // last, so the path wins over a query pair of the same name.
                for (key, raw) in pairs.into_iter().chain(params) {
                    let kind = declared
                        .and_then(|q| q.input.iter().find(|f| f.name == key))
                        .map(|f| &f.kind);
                    input.insert(key, coerce(raw, kind));
                }
                match self.dispatcher.query(name, ctx, QueryArgs { input }).await {
                    Ok(value) => answer(200, value),
                    Err(error) => refusal(&error),
                }
            }
        }
    }
}

/// Paging and narrowing from the query string. Remaining pairs after
/// `limit`, `cursor`, and `sort` are filters; the dispatcher
/// validates them against the declaration. Sort reads `column` or
/// `column:desc`.
fn list_args(pairs: &mut Vec<(String, String)>) -> Result<ListArgs, RestAnswer> {
    let mut args = ListArgs {
        // No limit asks for a full page. The dispatcher clamps this to
        // the declared `max_page_size`, the default the OpenAPI
        // document states and the GraphQL face applies.
        limit: u32::MAX,
        cursor: None,
        filters: BTreeMap::new(),
        sort: None,
    };
    for (key, raw) in pairs.drain(..) {
        match key.as_str() {
            "limit" => match raw.parse::<u32>() {
                Ok(limit) => args.limit = limit,
                Err(_) => {
                    return Err(answer(400, json!({ "error": "limit must be an integer" })));
                }
            },
            "cursor" => args.cursor = Some(raw),
            "sort" => {
                args.sort = Some(match raw.strip_suffix(":desc") {
                    Some(column) => (column.to_owned(), SortDirection::Desc),
                    None => (raw, SortDirection::Asc),
                });
            }
            _ => {
                args.filters.insert(key, Value::String(raw));
            }
        }
    }
    Ok(args)
}

/// Coerce a query-string value by its declared type. Strings pass
/// through; a value that refuses its declared shape passes through
/// as a string too, so the dispatcher's validator names the refusal.
fn coerce(raw: String, kind: Option<&TypeRef>) -> Value {
    match kind {
        Some(TypeRef::Int) => raw
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or(Value::String(raw)),
        Some(TypeRef::Bool) => match raw.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(raw),
        },
        Some(TypeRef::Json) => serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
        _ => Value::String(raw),
    }
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

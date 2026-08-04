//! The operator console: contract-declared pages rendered as HTML.
//!
//! The same declaration that renders the OpenAPI document, the
//! GraphQL schema, the MCP manifest, and the REST route table also
//! renders an operator's pages: a resource's listing with its
//! declared filters and sort orders, an instance with its fields and
//! sub-collections, an action as a form, a contract query as a
//! panel. Every read and every submitted form runs the dispatcher,
//! so the console refuses exactly where the API refuses.
//!
//! The renderer is transport-free and asset-free: the host hands in
//! a path, a query string, and a seeded [`JanusContext`], and
//! receives HTML. There is no JavaScript and no external asset;
//! forms are plain form posts and pages are plain documents, so the
//! console works wherever a browser reaches the host, air-gapped
//! included.

use std::sync::Arc;

use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde_json::Value;

use crate::ir::{Action, ActionField, Resource, TypeRef};
use crate::runtime::args::{ActionArgs, GetArgs, ListArgs, QueryArgs, SortDirection, SubListArgs};
use crate::runtime::context::JanusContext;
use crate::runtime::dispatch::Dispatcher;
use crate::runtime::error::JanusError;

/// One rendered page: an HTTP status and a complete HTML document.
#[derive(Debug, Clone)]
pub struct ConsoleAnswer {
    pub status: u16,
    pub html: String,
}

/// What a submitted form produces: a redirect back into the console,
/// or a page (an error, typically) to render in place.
#[derive(Debug, Clone)]
pub enum FormOutcome {
    Redirect(String),
    Page(ConsoleAnswer),
}

/// Host-provided knobs: where the console is mounted (used in every
/// link) and the name it wears.
#[derive(Debug, Clone)]
pub struct ConsoleConfig {
    pub base: String,
    pub title: String,
}

/// The contract-driven console over one dispatcher.
pub struct ConsoleRouter {
    dispatcher: Arc<Dispatcher>,
    config: ConsoleConfig,
}

const STYLE: &str = "
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.5 ui-monospace, 'Cascadia Code', Menlo, monospace;
  background: #101014; color: #d6d6dc; }
a { color: #8fb8ff; text-decoration: none; }
a:hover { text-decoration: underline; }
header { display: flex; gap: 1.5rem; align-items: baseline; padding: .75rem 1.25rem;
  border-bottom: 1px solid #26262e; }
header .title { font-weight: 700; color: #f2f2f6; }
header nav { display: flex; gap: 1rem; flex-wrap: wrap; }
main { padding: 1.25rem; max-width: 72rem; }
h1 { font-size: 1.15rem; margin: 0 0 1rem; color: #f2f2f6; }
h2 { font-size: .95rem; margin: 1.5rem 0 .5rem; color: #c9c9d4; }
table { border-collapse: collapse; width: 100%; margin: .5rem 0 1rem; }
th, td { text-align: left; padding: .3rem .6rem; border-bottom: 1px solid #26262e;
  vertical-align: top; }
th { color: #9a9aa8; font-weight: 600; }
tr:hover td { background: #16161c; }
code { color: #b8d7a3; word-break: break-all; }
.dim { color: #5c5c68; }
.cards { display: flex; gap: .75rem; flex-wrap: wrap; }
.card { border: 1px solid #26262e; padding: .75rem 1rem; min-width: 14rem; }
.card .name { font-weight: 700; }
form.inline { display: flex; gap: .5rem; flex-wrap: wrap; align-items: end; margin: .5rem 0; }
label { display: flex; flex-direction: column; gap: .15rem; font-size: .8rem; color: #9a9aa8; }
input, select, textarea, button { font: inherit; background: #16161c; color: #d6d6dc;
  border: 1px solid #33333e; padding: .3rem .5rem; }
button { cursor: pointer; background: #1d2733; border-color: #35507a; }
button:hover { background: #24344a; }
.banner { border: 1px solid #35507a; background: #16202e; padding: .5rem .75rem;
  margin-bottom: 1rem; }
.error { border-color: #7a3535; background: #2e1616; }
.actions { display: flex; gap: 1rem; flex-wrap: wrap; }
.action { border: 1px solid #26262e; padding: .75rem 1rem; }
pre { background: #16161c; border: 1px solid #26262e; padding: .75rem; overflow-x: auto; }
";

impl ConsoleRouter {
    pub fn new(dispatcher: Arc<Dispatcher>, config: ConsoleConfig) -> Self {
        Self { dispatcher, config }
    }

    /// Render one GET page. `path` is relative to the mount (leading
    /// slash optional), `query` the raw query string without `?`.
    pub async fn page(&self, path: &str, query: &str, ctx: JanusContext) -> ConsoleAnswer {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let pairs = parse_query(query);
        match parts.as_slice() {
            [] => self.overview(),
            ["r", resource] => self.list_page(resource, &pairs, ctx).await,
            ["r", resource, id] => self.detail_page(resource, &decode(id), &pairs, ctx).await,
            ["q", name] => self.query_page(name, &pairs, ctx).await,
            _ => self.error_page(404, "no such console page"),
        }
    }

    /// Handle one submitted form. Action forms post here; a
    /// successful dispatch redirects back into the console, a
    /// refusal renders in place with the dispatcher's words.
    pub async fn submit(
        &self,
        path: &str,
        form: &[(String, String)],
        ctx: JanusContext,
    ) -> FormOutcome {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let (resource, id, action) = match parts.as_slice() {
            ["r", resource, "a", action] => (*resource, None, *action),
            ["r", resource, id, "a", action] => (*resource, Some(decode(id)), *action),
            _ => {
                return FormOutcome::Page(self.error_page(404, "no such console form"));
            }
        };
        let Some(declared) = self.action_of(resource, action) else {
            return FormOutcome::Page(self.error_page(404, "no such declared action"));
        };
        let mut input = serde_json::Map::new();
        for (key, raw) in form {
            if raw.is_empty() {
                continue;
            }
            let kind = declared
                .input
                .iter()
                .find(|f| f.name == *key)
                .map(|f| &f.kind);
            if matches!(kind, Some(TypeRef::Json)) && serde_json::from_str::<Value>(raw).is_err() {
                return FormOutcome::Page(self.error_page(
                    400,
                    &format!(
                        "{key} takes JSON: a list reads [\"one\", \"two\"] \
                         and a string reads \"one\"",
                    ),
                ));
            }
            input.insert(key.clone(), coerce(raw.clone(), kind));
        }
        let args = ActionArgs {
            id: id.clone(),
            input,
        };
        match self.dispatcher.action(resource, action, ctx, args).await {
            Ok(answer) => {
                let back = match &id {
                    Some(id) => format!(
                        "{}/r/{}/{}?done={}",
                        self.config.base,
                        resource,
                        encode(id),
                        action
                    ),
                    None => format!("{}/r/{}?done={}", self.config.base, resource, action),
                };
                match answer.filter(|value| !is_empty(value)) {
                    Some(value) => FormOutcome::Page(self.answer_page(action, &value, &back)),
                    None => FormOutcome::Redirect(back),
                }
            }
            Err(error) => FormOutcome::Page(self.refusal_page(&error)),
        }
    }

    fn resource(&self, name: &str) -> Option<Resource> {
        self.dispatcher
            .contract()
            .resources
            .iter()
            .find(|r| r.name == name)
            .cloned()
    }

    fn action_of(&self, resource: &str, action: &str) -> Option<Action> {
        self.resource(resource)?
            .actions
            .iter()
            .find(|a| a.name == action)
            .cloned()
    }

    fn overview(&self) -> ConsoleAnswer {
        let contract = self.dispatcher.contract();
        let body = html! {
            h1 { "contract " (contract.name) " v" (contract.version) }
            div.cards {
                @for resource in &contract.resources {
                    div.card {
                        div.name {
                            a href=(format!("{}/r/{}", self.config.base, resource.name)) {
                                (resource.name)
                            }
                        }
                        div.dim { "table " (resource.table) }
                        div.dim {
                            (resource.actions.len()) " actions, "
                            (resource.sub_resources.len()) " sub-collections"
                        }
                    }
                }
            }
            @if !contract.queries.is_empty() {
                h2 { "queries" }
                ul {
                    @for query in &contract.queries {
                        li {
                            a href=(format!("{}/q/{}", self.config.base, query.name)) {
                                (query.name)
                            }
                            @if let Some(text) = &query.description {
                                span.dim { " " (text) }
                            }
                        }
                    }
                }
            }
        };
        self.shell(200, &self.config.title.clone(), body)
    }

    async fn list_page(
        &self,
        name: &str,
        pairs: &[(String, String)],
        ctx: JanusContext,
    ) -> ConsoleAnswer {
        let Some(resource) = self.resource(name) else {
            return self.error_page(404, "no such declared resource");
        };
        let mut args = ListArgs {
            limit: 50,
            cursor: None,
            filters: Default::default(),
            sort: None,
        };
        let mut done: Option<String> = None;
        let mut sort_state = String::new();
        for (key, value) in pairs {
            match key.as_str() {
                "cursor" => args.cursor = Some(value.clone()),
                "sort" => {
                    sort_state = value.clone();
                    args.sort = Some(match value.strip_suffix(":desc") {
                        Some(column) => (column.to_owned(), SortDirection::Desc),
                        None => (value.clone(), SortDirection::Asc),
                    });
                }
                "done" => done = Some(value.clone()),
                _ if !value.is_empty() => {
                    args.filters
                        .insert(key.clone(), Value::String(value.clone()));
                }
                _ => {}
            }
        }
        let filter_state = args.filters.clone();
        let output = match self.dispatcher.list(name, ctx, args).await {
            Ok(output) => output,
            Err(error) => return self.refusal_page(&error),
        };
        // The id is the instance's address and every wire row carries
        // one whether or not the declaration lists it, so it leads the
        // table implicitly.
        let mut columns: Vec<String> = vec!["id".to_owned()];
        for field in &resource.fields {
            let api = field.api_name().to_owned();
            if !columns.contains(&api) {
                columns.push(api);
            }
        }
        let collection_actions: Vec<&Action> = resource
            .actions
            .iter()
            .filter(|a| !a.path.contains("{id}"))
            .collect();
        let body = html! {
            h1 { (name) }
            @if let Some(action) = &done {
                div.banner { "action " (action) " completed" }
            }
            @if !resource.filterable.is_empty() || !resource.sortable.is_empty() {
                form.inline method="get" action=(format!("{}/r/{}", self.config.base, name)) {
                    @for column in &resource.filterable {
                        label {
                            (column)
                            input name=(column)
                                value=(filter_state.get(column).and_then(Value::as_str).unwrap_or(""));
                        }
                    }
                    @if !resource.sortable.is_empty() {
                        label {
                            "sort"
                            select name="sort" {
                                option value="" selected[sort_state.is_empty()] {
                                    "declared order"
                                }
                                @for column in &resource.sortable {
                                    option value=(column) selected[sort_state == *column] {
                                        (column)
                                    }
                                    @let descending = format!("{column}:desc");
                                    option value=(descending)
                                        selected[sort_state == descending] {
                                        (column) ":desc"
                                    }
                                }
                            }
                        }
                    }
                    button { "narrow" }
                }
            }
            table {
                thead { tr { @for column in &columns { th { (column) } } } }
                tbody {
                    @for item in &output.items {
                        tr {
                            @for column in &columns {
                                td {
                                    @if column == "id" {
                                        @if let Some(id) = item.get("id").and_then(Value::as_str) {
                                            a href=(format!("{}/r/{}/{}", self.config.base, name, encode(id))) {
                                                (id)
                                            }
                                        } @else { (cell(item.get(column.as_str()))) }
                                    } @else {
                                        (cell(item.get(column.as_str())))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            @if output.items.is_empty() { p.dim { "nothing listed" } }
            @if let Some(cursor) = &output.next_cursor {
                p {
                    a href=(format!("{}/r/{}?cursor={}", self.config.base, name, encode(cursor))) {
                        "older"
                    }
                }
            }
            @if !collection_actions.is_empty() {
                h2 { "actions" }
                div.actions {
                    @for action in &collection_actions {
                        (self.action_form(name, None, action))
                    }
                }
            }
        };
        self.shell(200, name, body)
    }

    async fn detail_page(
        &self,
        name: &str,
        id: &str,
        pairs: &[(String, String)],
        ctx: JanusContext,
    ) -> ConsoleAnswer {
        let Some(resource) = self.resource(name) else {
            return self.error_page(404, "no such declared resource");
        };
        let row = match self
            .dispatcher
            .get(name, ctx.clone(), GetArgs { id: id.to_owned() })
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return self.error_page(404, "no such instance"),
            Err(error) => return self.refusal_page(&error),
        };
        let done = pairs
            .iter()
            .find(|(key, _)| key == "done")
            .map(|(_, value)| value.clone());
        let mut subs: Vec<(String, Vec<Value>)> = Vec::new();
        for sub in &resource.sub_resources {
            let args = SubListArgs {
                parent_id: id.to_owned(),
                limit: 10,
                cursor: None,
                filters: Default::default(),
                sort: None,
            };
            match self
                .dispatcher
                .sub_list(name, &sub.name, ctx.clone(), args)
                .await
            {
                Ok(output) => subs.push((sub.name.clone(), output.items)),
                Err(_) => subs.push((sub.name.clone(), Vec::new())),
            }
        }
        let instance_actions: Vec<&Action> = resource
            .actions
            .iter()
            .filter(|a| a.path.contains("{id}"))
            .collect();
        let body = html! {
            h1 { (name) " / " (id) }
            @if let Some(action) = &done {
                div.banner { "action " (action) " completed" }
            }
            table {
                @for field in &resource.fields {
                    tr {
                        th { (field.api_name()) }
                        td { (cell(row.get(field.api_name()))) }
                    }
                }
            }
            @for (sub_name, items) in &subs {
                h2 { (sub_name) }
                @if items.is_empty() { p.dim { "nothing listed" } } @else {
                    (object_table(items))
                }
            }
            @if !instance_actions.is_empty() {
                h2 { "actions" }
                div.actions {
                    @for action in &instance_actions {
                        (self.action_form(name, Some(id), action))
                    }
                }
            }
        };
        self.shell(200, &format!("{name}/{id}"), body)
    }

    async fn query_page(
        &self,
        name: &str,
        pairs: &[(String, String)],
        ctx: JanusContext,
    ) -> ConsoleAnswer {
        let Some(query) = self
            .dispatcher
            .contract()
            .queries
            .iter()
            .find(|q| q.name == name)
            .cloned()
        else {
            return self.error_page(404, "no such declared query");
        };
        let filled = pairs.iter().any(|(_, v)| !v.is_empty());
        let mut result: Option<Result<Value, JanusError>> = None;
        if filled {
            let mut input = serde_json::Map::new();
            for (key, raw) in pairs {
                if raw.is_empty() {
                    continue;
                }
                let kind = query.input.iter().find(|f| f.name == *key).map(|f| &f.kind);
                input.insert(key.clone(), coerce(raw.clone(), kind));
            }
            result = Some(self.dispatcher.query(name, ctx, QueryArgs { input }).await);
        }
        let body = html! {
            h1 { (name) }
            @if let Some(text) = &query.description { p.dim { (text) } }
            form.inline method="get" action=(format!("{}/q/{}", self.config.base, name)) {
                @for field in &query.input {
                    label {
                        (field.name) @if field.required { " *" }
                        (input_for(field, pairs))
                    }
                }
                button { "run" }
            }
            @match &result {
                Some(Ok(value)) => {
                    @if let Some(items) = value.get("items").and_then(Value::as_array) {
                        (object_table(items))
                        @if items.is_empty() { p.dim { "nothing answered" } }
                    } @else {
                        pre { code { (pretty(value)) } }
                    }
                }
                Some(Err(error)) => { div.banner.error { (error.to_string()) } }
                None => {}
            }
        };
        self.shell(200, name, body)
    }

    fn action_form(&self, resource: &str, id: Option<&str>, action: &Action) -> Markup {
        let target = match id {
            Some(id) => format!(
                "{}/r/{}/{}/a/{}",
                self.config.base,
                resource,
                encode(id),
                action.name
            ),
            None => format!("{}/r/{}/a/{}", self.config.base, resource, action.name),
        };
        html! {
            div.action {
                form method="post" action=(target) {
                    div { strong { (action.name) } }
                    @if let Some(text) = &action.description { p.dim { (text) } }
                    @for field in &action.input {
                        label {
                            (field.name) @if field.required { " *" }
                            (input_for(field, &[]))
                        }
                    }
                    button { "perform" }
                }
            }
        }
    }

    /// What an action answered, shown rather than discarded. A
    /// secret that appears once appears here, and nowhere later.
    fn answer_page(&self, action: &str, value: &Value, back: &str) -> ConsoleAnswer {
        let body = html! {
            h1 { (action) }
            div.banner { "completed" }
            @if let Some(map) = value.as_object() {
                table {
                    @for (key, field) in map {
                        tr {
                            th { (key) }
                            td { (cell(Some(field))) }
                        }
                    }
                }
            } @else {
                pre { code { (pretty(value)) } }
            }
            p.dim { "This answer is not stored. Copy anything you need before leaving." }
            p { a href=(back) { "back" } }
        };
        self.shell(200, action, body)
    }

    fn refusal_page(&self, error: &JanusError) -> ConsoleAnswer {
        let status = match error {
            JanusError::BadRequest(_) => 400,
            JanusError::Unauthorized(_) => 401,
            JanusError::Forbidden(_) => 403,
            JanusError::NotFound => 404,
            JanusError::Conflict(_) => 409,
            JanusError::TooManyRequests(_) => 429,
            _ => 500,
        };
        self.error_page(status, &error.to_string())
    }

    fn error_page(&self, status: u16, message: &str) -> ConsoleAnswer {
        let body = html! {
            h1 { "refused" }
            div.banner.error { (message) }
            p { a href=(&self.config.base) { "back to the overview" } }
        };
        self.shell(status, "refused", body)
    }

    fn shell(&self, status: u16, title: &str, content: Markup) -> ConsoleAnswer {
        let contract = self.dispatcher.contract();
        let document = html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " · " (self.config.title) }
                    link rel="icon" href="data:,";
                    style { (PreEscaped(STYLE)) }
                }
                body {
                    header {
                        span.title {
                            a href=(&self.config.base) { (self.config.title) }
                        }
                        nav {
                            @for resource in &contract.resources {
                                a href=(format!("{}/r/{}", self.config.base, resource.name)) {
                                    (resource.name)
                                }
                            }
                            @for query in &contract.queries {
                                a href=(format!("{}/q/{}", self.config.base, query.name)) {
                                    (query.name)
                                }
                            }
                        }
                    }
                    main { (content) }
                }
            }
        };
        ConsoleAnswer {
            status,
            html: document.into_string(),
        }
    }
}

/// One table over loosely-shaped objects: columns are the union of
/// keys in declaration-free order (first seen wins).
fn object_table(items: &[Value]) -> Markup {
    let mut columns: Vec<String> = Vec::new();
    for item in items {
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    html! {
        table {
            thead { tr { @for column in &columns { th { (column) } } } }
            tbody {
                @for item in items {
                    tr {
                        @for column in &columns {
                            td { (cell(item.get(column.as_str()))) }
                        }
                    }
                }
            }
        }
    }
}

fn input_for(field: &ActionField, pairs: &[(String, String)]) -> Markup {
    let current = pairs
        .iter()
        .find(|(k, _)| k == &field.name)
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    match field.kind {
        TypeRef::Int => html! { input type="number" name=(field.name) value=(current); },
        TypeRef::Bool => html! {
            select name=(field.name) {
                option value="" { "unset" }
                option value="true" selected[current == "true"] { "true" }
                option value="false" selected[current == "false"] { "false" }
            }
        },
        TypeRef::Json => html! { textarea name=(field.name) rows="3" { (current) } },
        TypeRef::String => html! { input name=(field.name) value=(current); },
    }
}

fn cell(value: Option<&Value>) -> Markup {
    match value {
        None | Some(Value::Null) => html! { span.dim { "·" } },
        Some(Value::String(text)) => html! { (text) },
        Some(Value::Bool(flag)) => html! { (flag) },
        Some(Value::Number(number)) => html! { (number.to_string()) },
        Some(other) => {
            let compact = serde_json::to_string(other).unwrap_or_default();
            let shown: String = compact.chars().take(120).collect();
            html! { code { (shown) @if compact.len() > 120 { "…" } } }
        }
    }
}

/// Whether an answer carries nothing worth showing.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

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
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&raw[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

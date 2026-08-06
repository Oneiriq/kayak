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

/// The console's whole stylesheet, exported because a host renders
/// pages of its own beside the generated ones and two stylesheets
/// means two consoles. Copal's deployment page is the case: it kept a
/// copy, so it went on printing raw byte counts and nanosecond
/// timestamps after the generated pages stopped.
///
/// Monospace for data, because columns of ids and digests line up and
/// digits have to. A UI face for headings, labels, and prose, because
/// setting those in monospace too is what made the page read as a
/// debug dump rather than as somewhere to work.
pub const STYLE: &str = "
:root {
  color-scheme: dark light;
  --ground: #0e0e13;
  --raised: #16161d;
  --line: #262630;
  --text: #d8d8e0;
  --soft: #8b8b9c;
  --faint: #5a5a68;
  --bright: #f4f4f8;
  --accent: #7fa9ff;
  --good: #6fcf8b;
  --bad: #f08a7c;
  --busy: #e3c069;
  --ui: system-ui, -apple-system, 'Segoe UI', sans-serif;
  --mono: ui-monospace, 'Cascadia Code', Menlo, monospace;
}

* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.55 var(--mono);
  background: var(--ground); color: var(--text); }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

header { display: flex; gap: 1.5rem; align-items: baseline; padding: .8rem 1.5rem;
  border-bottom: 1px solid var(--line); background: var(--raised);
  position: sticky; top: 0; z-index: 3; flex-wrap: wrap; }
header .title { font-weight: 700; color: var(--bright); }
header nav { display: flex; gap: 1.1rem; flex-wrap: wrap; }

main { padding: 1.5rem; max-width: 120rem; margin: 0 auto; }
h1 { font: 600 1.35rem/1.3 var(--ui); margin: 0 0 .25rem; color: var(--bright);
  letter-spacing: -.01em; }
h2 { font: 600 .8rem/1.4 var(--ui); margin: 2rem 0 .6rem; color: var(--soft);
  text-transform: uppercase; letter-spacing: .08em; }
p { font-family: var(--ui); }

/* Wide tables scroll in their own box, so the page never does. */
.scroll { overflow-x: auto; border: 1px solid var(--line); border-radius: 3px;
  background: var(--raised); }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: .45rem .7rem; vertical-align: top;
  border-bottom: 1px solid var(--line); white-space: nowrap; }
thead th { position: sticky; top: 0; background: var(--raised); z-index: 1;
  font: 600 .72rem/1.5 var(--ui); text-transform: uppercase; letter-spacing: .06em;
  color: var(--soft); border-bottom-color: #33333f; }
tbody tr:last-child td { border-bottom: 0; }
/* A label/value table: the label hugs its text rather than taking
   half the page and leaving the value stranded. */
table.fields th { width: 1px; white-space: nowrap; padding-right: 2rem;
  color: var(--soft); font-weight: 600; }
tbody tr:hover td { background: #1c1c25; }
td { font-variant-numeric: tabular-nums; }
code { color: #a8cf9a; }
.dim { color: var(--faint); }
.count { font-family: var(--ui); color: var(--soft); font-size: .8rem; margin: .5rem 0 0; }
.card .count { margin: .1rem 0 .4rem; }
ul.preview { list-style: none; margin: 0; padding: 0; display: grid; gap: .15rem; }
/* min-width:0 on the row as well as the label: a grid item defaults
   to min-width:auto and refuses to shrink below its content, so the
   row overflowed the card at 338px inside 257px and clipped the time
   however the label was styled. */
ul.preview li { display: flex; gap: .5rem; align-items: baseline; min-width: 0;
  font-size: .85rem; color: var(--text); }
/* min-width:0 is what lets the ellipsis happen: a flex item will not
   shrink below its content without it, so a long label pushed the
   time off the card instead of truncating itself. */
ul.preview .label { flex: 1 1 auto; min-width: 0; overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; }
ul.preview .when { flex: 0 0 auto; margin-left: auto; color: var(--faint);
  font-size: .78rem; white-space: nowrap; }
.error-note { color: var(--bad); }

/* A nested value costs one line closed, whatever it holds. */
details.nested > summary { cursor: pointer; color: var(--soft); font-size: .85rem; }
details.nested > summary:hover { color: var(--text); }
details.nested[open] > summary { margin-bottom: .35rem; }
details.nested pre { margin: 0; max-height: 20rem; overflow: auto; white-space: pre;
  font-size: .82rem; }

/* State reads at a glance or the column is not worth its width. */
.chip { display: inline-block; padding: .05rem .45rem; border-radius: 999px;
  font-size: .78rem; border: 1px solid var(--line); background: #1e1e27;
  color: var(--soft); }
.chip-good { color: var(--good); border-color: #2f5c3d; background: #14231a; }
.chip-bad { color: var(--bad); border-color: #6b3630; background: #241614; }
.chip-busy { color: var(--busy); border-color: #5e4f26; background: #221e12; }
.chip-gone { color: var(--faint); }
.chip-yes { color: var(--good); border-color: #2f5c3d; }
.chip-no { color: var(--faint); }

.cards { display: grid; gap: .75rem; margin: .5rem 0;
  grid-template-columns: repeat(auto-fill, minmax(18rem, 1fr)); }
.card { border: 1px solid var(--line); border-radius: 3px; padding: .8rem 1rem;
  background: var(--raised); }
.card .name { font-weight: 700; }

form.inline { display: flex; gap: .6rem; flex-wrap: wrap; align-items: end;
  margin: .5rem 0 1rem; }
label { display: flex; flex-direction: column; gap: .2rem;
  font: .75rem/1.4 var(--ui); color: var(--soft); }
input, select, textarea, button { font: inherit; font-family: var(--mono);
  background: #101017; color: var(--text); border: 1px solid #34343f;
  border-radius: 3px; padding: .35rem .55rem; }
input:focus, select:focus, textarea:focus { border-color: var(--accent); outline: none; }
button { cursor: pointer; background: #1e2a3a; border-color: #3a5680;
  color: var(--bright); font-family: var(--ui); font-weight: 600; padding: .38rem .9rem; }
button:hover { background: #27374d; }

.choices { display: flex; gap: .7rem; flex-wrap: wrap; align-items: center;
  padding: .25rem 0; }
.choice { flex-direction: row; align-items: center; gap: .3rem; color: var(--text);
  font-family: var(--mono); font-size: .85rem; cursor: pointer; }
.choice input { margin: 0; accent-color: var(--accent); }

.banner { border: 1px solid #3a5680; background: #16202e; padding: .6rem .85rem;
  border-radius: 3px; margin-bottom: 1rem; font-family: var(--ui); }
.error { border-color: #6b3630; background: #241614; }

.actions { display: grid; gap: 1rem;
  grid-template-columns: repeat(auto-fill, minmax(22rem, 1fr)); align-items: start; }
.action { border: 1px solid var(--line); border-radius: 3px; padding: .9rem 1.1rem;
  background: var(--raised); }
.action form.inline { display: grid; gap: .55rem; }
.action input, .action select, .action textarea { width: 100%; }
.action button { justify-self: start; }
pre { background: #101017; border: 1px solid var(--line); border-radius: 3px;
  padding: .8rem; overflow-x: auto; }

/* Every colour above goes through a token, so daylight is the tokens
   said again. An operator on a bright screen reading a black page is
   the same problem as the reverse, and neither is a preference the
   console gets to hold on their behalf. */
@media (prefers-color-scheme: light) {
  :root {
    --ground: #fbfbfc;
    --raised: #ffffff;
    --line: #e2e2e8;
    --text: #22222a;
    --soft: #63636f;
    --faint: #93939f;
    --bright: #0d0d12;
    --accent: #2c5fc4;
    --good: #1f7a3d;
    --bad: #b3372a;
    --busy: #8a6412;
  }
  input, select, textarea { background: #ffffff; border-color: #cfcfd8; }
  button { background: #e8eefb; border-color: #b6c6e6; color: #143a7d; }
  button:hover { background: #dbe5f8; }
  tbody tr:hover td { background: #f2f2f6; }
  code { color: #2f6b2a; }
  pre { background: #f6f6f9; }
  .chip { background: #f1f1f5; }
  .chip-good { background: #e8f5ec; border-color: #b6ddc3; }
  .chip-bad { background: #fdeceb; border-color: #eec2bd; }
  .chip-busy { background: #fbf3df; border-color: #e6d5a6; }
  .banner { background: #eef3fd; border-color: #b6c6e6; }
  .error { background: #fdeceb; border-color: #eec2bd; }
}
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
            [] => self.overview(ctx).await,
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

    /// What is in this deployment, rather than what its contract
    /// declares.
    ///
    /// The cards used to read "8 actions, 1 sub-collections", which
    /// answers a question nobody opening a console has. An operator
    /// arrives wanting to know what is here and whether anything needs
    /// attention, so each card lists the resource's first page: how
    /// many rows, whether more follow, and a few of them by whichever
    /// column names a row to a reader.
    ///
    /// One listing per resource, at the page size a card can show. A
    /// resource whose listing refuses says so on its own card and
    /// leaves the rest of the page standing, because an overview that
    /// blanks on one failure is worse than one that reports it.
    async fn overview(&self, ctx: JanusContext) -> ConsoleAnswer {
        let contract = self.dispatcher.contract().clone();
        let mut cards = Vec::new();
        for resource in &contract.resources {
            let args = ListArgs {
                limit: PREVIEW_ROWS,
                cursor: None,
                filters: Default::default(),
                sort: None,
            };
            let outcome = self
                .dispatcher
                .list(&resource.name, ctx.clone(), args)
                .await;
            cards.push((resource, outcome));
        }

        let body = html! {
            h1 { (contract.name) " v" (contract.version) }
            div.cards {
                @for (resource, outcome) in &cards {
                    div.card {
                        div.name {
                            a href=(format!("{}/r/{}", self.config.base, resource.name)) {
                                (resource.name)
                            }
                        }
                        @match outcome {
                            Ok(page) => {
                                div.count {
                                    (page.items.len())
                                    @if page.next_cursor.is_some() { "+" }
                                    @if page.items.len() == 1 { " row" } @else { " rows" }
                                }
                                @if page.items.is_empty() {
                                    div.dim { "nothing yet" }
                                } @else {
                                    @let preview = preview_columns(resource, &page.items);
                                    ul.preview {
                                        @for item in &page.items {
                                            li { (preview_line(&preview, item)) }
                                        }
                                    }
                                }
                            }
                            Err(reason) => div.dim.error-note { (reason.to_string()) }
                        }
                    }
                }
            }
            @if !contract.queries.is_empty() {
                h2 { "queries" }
                div.cards {
                    @for query in &contract.queries {
                        div.card {
                            div.name {
                                a href=(format!("{}/q/{}", self.config.base, query.name)) {
                                    (query.name)
                                }
                            }
                            @if let Some(text) = &query.description {
                                div.dim { (text) }
                            }
                        }
                    }
                }
            }
        };
        self.shell(200, "overview", body)
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
                // An empty value is the control saying nothing, which
                // is what the form submits when a caller leaves it
                // alone. Reading it as a request would sort on a
                // column named "", and the narrow button answered 400
                // for exactly that whenever the sort was left at
                // declared order.
                "cursor" | "sort" if value.is_empty() => {}
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
                        @let chosen = filter_state
                            .get(column)
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        label {
                            (column)
                            @match resource.filter_options.get(column) {
                                Some(options) => select name=(column) {
                                    option value="" selected[chosen.is_empty()] { "any" }
                                    @for option in options {
                                        option value=(option) selected[chosen == option] {
                                            (option)
                                        }
                                    }
                                },
                                None => input name=(column) value=(chosen);,
                            }
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
            div.scroll {
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
                                        } @else { (cell(column, item.get(column.as_str()))) }
                                    } @else {
                                        (cell(column, item.get(column.as_str())))
                                    }
                                }
                            }
                        }
                    }
                }
              }
            }
            @if output.items.is_empty() {
                p.dim { "Nothing here yet. Anything created below shows up in this list." }
            } @else {
                p.count {
                    (output.items.len())
                    @if output.items.len() == 1 { " row" } @else { " rows" }
                    @if output.next_cursor.is_some() { ", more after these" }
                }
            }
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
            div.scroll {
                table.fields {
                    @for field in &resource.fields {
                        tr {
                            th { (field.api_name()) }
                            td { (cell(field.api_name(), row.get(field.api_name()))) }
                        }
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
                div.scroll {
                    table.fields {
                        @for (key, field) in map {
                            tr {
                                th { (key) }
                                td { (cell(key, Some(field))) }
                            }
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
        div.scroll {
            table {
                thead { tr { @for column in &columns { th { (column) } } } }
                tbody {
                    @for item in items {
                        tr {
                            @for column in &columns {
                                td { (cell(column, item.get(column.as_str()))) }
                            }
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
    // Several of a set is a row of checkboxes. A menu that allows
    // multiple selection hides that it does, and needs a modifier key
    // to use; a checkbox says what it is.
    if field.multiple && !field.options.is_empty() {
        let chosen: Vec<&str> = current.split(',').map(str::trim).collect();
        return html! {
            span.choices {
                @for option in &field.options {
                    label.choice {
                        input type="checkbox" name=(field.name) value=(option)
                            checked[chosen.contains(&option.as_str())];
                        (option)
                    }
                }
            }
        };
    }
    // A closed set is a menu. Typing into a box and hoping is what
    // this replaces, and the list is the contract's own.
    if !field.options.is_empty() {
        return html! {
            select name=(field.name) {
                @if !field.required {
                    option value="" selected[current.is_empty()] { "unset" }
                }
                @for option in &field.options {
                    option value=(option) selected[current == option] { (option) }
                }
            }
        };
    }
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

/// How many rows a card previews. Enough to show what a resource
/// holds, few enough that a deployment with many resources still
/// fits on a screen.
const PREVIEW_ROWS: u32 = 4;

/// What a preview row says: a label, and when it happened.
///
/// No single column names an event or a run. Asking for the most
/// distinct column alone gave four raw timestamps, and preferring a
/// name-like column alone gave "file.ready" four times, which is four
/// rows of nothing either way. What an operator wants from a card is
/// what the row is and when, so a preview carries both when both
/// exist.
struct Preview<'a> {
    label: Option<&'a str>,
    time: Option<&'a str>,
}

/// Timestamps are excluded from the label, since they answer the other
/// half of the line, and a column that repeats one value across the
/// page is not naming anything whatever it is called.
fn preview_columns<'a>(resource: &'a crate::ir::Resource, items: &[Value]) -> Preview<'a> {
    const LABELS: [&str; 7] = ["name", "path", "title", "key", "label", "slug", "subject"];
    let mut label: Option<(&str, usize, bool)> = None;
    let mut time: Option<&str> = None;

    for field in &resource.fields {
        let api = field.api_name();
        let mut distinct = std::collections::BTreeSet::new();
        let mut temporal = 0;
        let mut present = 0;
        for item in items {
            if let Some(Value::String(text)) = item.get(api) {
                present += 1;
                distinct.insert(text.as_str());
                if shorten_timestamp(text).is_some() {
                    temporal += 1;
                }
            }
        }
        if present == 0 {
            continue;
        }
        if temporal == present {
            time = time.or(Some(api));
            continue;
        }
        let candidate = (api, distinct.len(), LABELS.contains(&api));
        let better = match label {
            None => true,
            Some((_, count, labelled)) => {
                candidate.1 > count || (candidate.1 == count && candidate.2 && !labelled)
            }
        };
        if better {
            label = Some(candidate);
        }
    }

    Preview {
        label: label.map(|(api, _, _)| api).or(Some("id")),
        time,
    }
}

/// One preview line, formatted the way the tables format the same
/// values.
fn preview_line(preview: &Preview, item: &Value) -> Markup {
    let text = |column: Option<&str>| {
        column.and_then(|c| item.get(c)).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        })
    };
    let label = text(preview.label).or_else(|| {
        item.get("id")
            .and_then(Value::as_str)
            .map(std::borrow::ToOwned::to_owned)
    });
    // `08-05 19:29` rather than the full stamp: a card has room for a
    // label or for a year, and the label is what a reader came for.
    // The listing page still carries the whole value.
    let when = text(preview.time).map(|raw| match shorten_timestamp(&raw) {
        Some(short) if short.len() >= 16 => short[5..16].to_owned(),
        Some(short) => short,
        None => raw,
    });
    html! {
        @if let Some(label) = label { span.label { (label) } }
        @if let Some(when) = when { span.when { (when) } }
    }
}

/// One table cell, formatted for what the value turns out to be.
///
/// A listing is scanned rather than read, and the raw projection
/// defeats scanning. A nested object printed as a hundred and twenty
/// characters of JSON wraps down twenty lines in a narrow column and
/// takes the whole row with it, so five files became a page three
/// thousand pixels tall. A digest is sixty-four characters nobody
/// reads across. A timestamp carries nanoseconds nobody reads at all.
///
/// Every shortening keeps the full value in `title`, so the exact
/// bytes stay one hover away and the table never becomes the reason a
/// value cannot be seen.
pub fn cell(column: &str, value: Option<&Value>) -> Markup {
    match value {
        None | Some(Value::Null) => html! { span.dim { "·" } },
        Some(Value::Bool(flag)) => html! {
            span.chip.chip-yes[*flag].chip-no[!*flag] { (flag) }
        },
        Some(Value::Number(number)) => byte_cell(column, number),
        Some(Value::String(text)) => string_cell(column, text),
        // Objects and arrays collapse. Opening one is a click, and a
        // closed one costs a single line whatever it holds.
        Some(other) => {
            let summary = match other {
                Value::Array(items) => format!("[{}]", items.len()),
                Value::Object(map) => match map.len() {
                    1 => "{1 field}".to_owned(),
                    n => format!("{{{n} fields}}"),
                },
                _ => "…".to_owned(),
            };
            html! {
                details.nested {
                    summary { (summary) }
                    pre { (pretty(other)) }
                }
            }
        }
    }
}

/// Numbers that are byte counts, said in units a reader holds in mind.
///
/// The column name is the only signal available, and `size_bytes` is
/// the convention this reads. A wrong guess costs a unit suffix on a
/// number whose exact value is still in `title`.
fn byte_cell(column: &str, number: &serde_json::Number) -> Markup {
    let counts_bytes = column == "size" || column == "bytes" || column.ends_with("_bytes");
    match (counts_bytes, number.as_u64()) {
        (true, Some(bytes)) => html! {
            span title=(format!("{bytes} bytes")) { (human_bytes(bytes)) }
        },
        _ => html! { (number.to_string()) },
    }
}

fn string_cell(column: &str, text: &str) -> Markup {
    // A state reads at a glance or it is not worth a column. Which
    // words mean trouble is a small fixed vocabulary, and anything
    // outside it renders as a plain chip rather than a guess.
    if is_state_column(column) && text.len() <= 24 && !text.contains(' ') {
        let tone = state_tone(text);
        return html! { span class=(format!("chip chip-{tone}")) { (text) } };
    }
    if let Some(short) = shorten_digest(text) {
        return html! { code title=(text) { (short) "…" } };
    }
    if let Some(short) = shorten_timestamp(text) {
        return html! { span title=(text) { (short) } };
    }
    html! { (text) }
}

fn is_state_column(column: &str) -> bool {
    matches!(
        column,
        "state" | "status" | "access" | "verdict" | "mode" | "kind" | "result"
    )
}

/// The tone a state carries. Unknown words are neutral, because a
/// console that colours a word it does not understand is guessing in
/// the one place an operator trusts colour.
///
/// Access levels stay neutral for the same reason turned around.
/// Green reads as healthy, and `public` on a storage service is the
/// most exposed a record gets; colouring it well would say the
/// opposite of what it means.
fn state_tone(text: &str) -> &'static str {
    match text {
        "ready" | "active" | "ok" | "clean" | "succeeded" | "delivered" => "good",
        "failed" | "error" | "quarantined" | "refused" | "expired" | "infected" => "bad",
        "draft" | "uploading" | "scanning" | "pending" | "running" | "queued" => "busy",
        "deleted" | "disabled" | "inactive" => "gone",
        _ => "flat",
    }
}

/// Content digests and other long hex runs, cut to a readable stub.
fn shorten_digest(text: &str) -> Option<String> {
    let long_hex = text.len() >= 32 && text.chars().all(|c| c.is_ascii_hexdigit());
    long_hex.then(|| text.chars().take(12).collect())
}

/// `2026-08-05T19:29:23.394140700Z` down to `2026-08-05 19:29:23`.
///
/// Sub-second precision belongs in an audit export rather than in a
/// column a reader scans, and the full value stays in `title`.
fn shorten_timestamp(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let shaped = text.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && text.ends_with('Z');
    shaped.then(|| format!("{} {}", &text[..10], &text[11..19]))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
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
    let pairs = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(pair), String::new()),
        });
    // A checked box submits its own pair, so a caller ticking three
    // of them sends the key three times. The wire carries one
    // comma-separated value, so repeats are joined here rather than
    // asking every resolver to cope with either shape.
    let mut collected: Vec<(String, String)> = Vec::new();
    for (key, value) in pairs {
        match collected.iter_mut().find(|(k, _)| *k == key) {
            Some((_, existing)) if !value.is_empty() => {
                if existing.is_empty() {
                    *existing = value;
                } else {
                    existing.push(',');
                    existing.push_str(&value);
                }
            }
            Some(_) => {}
            None => collected.push((key, value)),
        }
    }
    collected
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

#[cfg(test)]
mod cells {
    use super::*;
    use serde_json::json;

    fn rendered(column: &str, value: serde_json::Value) -> String {
        cell(column, Some(&value)).into_string()
    }

    /// The defect this formatting exists for: a nested object printed
    /// in full wrapped down twenty lines in a narrow column and took
    /// the row with it, so five records made a page three thousand
    /// pixels tall. Closed, it costs one line whatever it holds.
    #[test]
    fn a_nested_value_collapses_to_one_line() {
        let html = rendered("metadata", json!({"a": 1, "b": 2, "c": {"d": 3}}));
        assert!(html.contains("{3 fields}"), "{html}");
        assert!(html.contains("<details"), "{html}");
        // The whole value is still there, behind the disclosure, and
        // escaped on the way out.
        assert!(html.contains("&quot;d&quot;"), "{html}");

        assert!(rendered("tags", json!(["x", "y"])).contains("[2]"));
        assert!(rendered("metadata", json!({"only": 1})).contains("{1 field}"));
    }

    /// Every shortening keeps the exact value in `title`, so the table
    /// never becomes the reason a value cannot be read.
    #[test]
    fn shortening_never_loses_the_value() {
        let digest = "8d44475c7cc26e9343de5e7666bd63f144762809dedbed5442a8f569498d6e0a";
        let html = rendered("digest", json!(digest));
        assert!(html.contains("8d44475c7cc2"), "{html}");
        assert!(
            html.contains(digest),
            "the full digest stays in title: {html}"
        );

        let stamp = "2026-08-05T19:29:23.394140700Z";
        let html = rendered("created_at", json!(stamp));
        assert!(html.contains("2026-08-05 19:29:23"), "{html}");
        assert!(html.contains(stamp), "{html}");

        let html = rendered("size_bytes", json!(1067));
        assert!(html.contains("1.0 KB"), "{html}");
        assert!(html.contains("1067 bytes"), "{html}");
    }

    /// A byte count is guessed from the column name, so a number that
    /// counts something else is left alone.
    #[test]
    fn only_byte_columns_are_read_as_bytes() {
        assert!(rendered("version_count", json!(1067)).contains("1067"));
        assert!(!rendered("version_count", json!(1067)).contains("KB"));
        assert!(rendered("bytes", json!(2048)).contains("2.0 KB"));
        assert!(rendered("size", json!(512)).contains("512 B"));
    }

    /// Colour is the one thing an operator trusts without reading, so
    /// a word the vocabulary does not know gets none.
    #[test]
    fn only_known_states_carry_a_tone() {
        assert!(rendered("state", json!("ready")).contains("chip-good"));
        assert!(rendered("state", json!("quarantined")).contains("chip-bad"));
        assert!(rendered("state", json!("scanning")).contains("chip-busy"));
        assert!(rendered("state", json!("wobbly")).contains("chip-flat"));

        // Green reads as healthy, and public is the most exposed a
        // record gets on a storage service.
        assert!(rendered("access", json!("public")).contains("chip-flat"));
        assert!(!rendered("access", json!("public")).contains("chip-good"));

        // A column that is not a state keeps its text plain.
        assert!(!rendered("path", json!("ready")).contains("chip"));
    }

    #[test]
    fn bytes_read_in_units_a_reader_holds() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1_572_864), "1.5 MB");
        assert_eq!(human_bytes(268_435_456), "256.0 MB");
    }

    /// Something shaped like neither a digest nor a timestamp comes
    /// through untouched.
    #[test]
    fn ordinary_text_is_left_alone() {
        assert!(rendered("path", json!("docs/log.txt")).contains("docs/log.txt"));
        // A short hex run is too short to be a digest.
        assert!(rendered("code", json!("abc123")).contains("abc123"));
        assert!(!rendered("code", json!("abc123")).contains('…'));
    }
}

#[cfg(test)]
mod previews {
    use super::*;
    use crate::ir::{FieldExposure, Resource};
    use serde_json::json;

    fn resource(fields: &[&str]) -> Resource {
        Resource {
            name: "things".into(),
            table: "thing".into(),
            fields: fields.iter().map(|f| FieldExposure::column(*f)).collect(),
            pinned: vec![],
            filterable: vec![],
            filter_options: Default::default(),
            sortable: vec![],
            max_page_size: 50,
            actions: vec![],
            content: None,
            sub_resources: vec![],
            rate_class: None,
            reads_require: vec![],
            watchable: false,
            graphql: None,
        }
    }

    /// The column that tells rows apart is the one that names them.
    #[test]
    fn the_label_is_the_column_that_distinguishes() {
        let items = vec![
            json!({"kind": "file.ready", "path": "a.txt"}),
            json!({"kind": "file.ready", "path": "b.txt"}),
        ];
        let subject = resource(&["kind", "path"]);
        let preview = preview_columns(&subject, &items);
        assert_eq!(
            preview.label,
            Some("path"),
            "kind repeats and names nothing"
        );
    }

    /// Timestamps answer when, so they never answer what. Asking for
    /// the most distinct column alone put four raw stamps on a card.
    #[test]
    fn a_timestamp_is_the_time_and_never_the_label() {
        let items = vec![
            json!({"kind": "file.ready", "created_at": "2026-08-05T19:29:24.394140700Z"}),
            json!({"kind": "file.ready", "created_at": "2026-08-05T16:05:57.917213800Z"}),
        ];
        let subject = resource(&["kind", "created_at"]);
        let preview = preview_columns(&subject, &items);
        assert_eq!(preview.time, Some("created_at"));
        assert_eq!(
            preview.label,
            Some("kind"),
            "a repeated label still beats a timestamp for saying what a row is",
        );
    }

    /// A card orients; the listing carries the exact value.
    #[test]
    fn the_preview_time_drops_the_year_and_the_seconds() {
        let items = vec![json!({"kind": "run", "at": "2026-08-05T19:29:24.394140700Z"})];
        let subject = resource(&["kind", "at"]);
        let preview = preview_columns(&subject, &items);
        let line = preview_line(&preview, &items[0]).into_string();
        assert!(line.contains("08-05 19:29"), "{line}");
        assert!(!line.contains("2026"), "{line}");
        assert!(!line.contains(":24"), "{line}");
    }

    #[test]
    fn a_row_with_nothing_to_say_falls_back_to_its_id() {
        let items = vec![json!({"id": "01ABC"})];
        let subject = resource(&["missing"]);
        let preview = preview_columns(&subject, &items);
        let line = preview_line(&preview, &items[0]).into_string();
        assert!(line.contains("01ABC"), "{line}");
    }
}

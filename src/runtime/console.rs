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
use crate::naming::{camel, singular};
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
/// The appearance control, as a mark rather than a word.
///
/// A control whose whole job is switching light and dark says that
/// faster as a half-lit circle than as the word "theme" set in the
/// same size as the navigation beside it.
const THEME_ICON: &str = r#"<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" focusable="false"><circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 1.5a6.5 6.5 0 0 1 0 13z" fill="currentColor"/></svg>"#;

/// The theme control, and the two lines that apply a stored choice.
///
/// Without this the console follows `prefers-color-scheme`, which is
/// right for most readers and stays right when this script does not
/// run: everything here is additive, and a browser with scripting off
/// keeps the automatic behaviour.
///
/// It sits in the head so the attribute lands before first paint. A
/// choice applied after the page draws is a flash of the other theme,
/// which is worse than not offering the choice.
const THEME_SCRIPT: &str = r#"
(function () {
  var KEY = 'janus-theme';
  var root = document.documentElement;
  function apply(mode) {
    if (mode) { root.setAttribute('data-theme', mode); }
    else { root.removeAttribute('data-theme'); }
  }
  apply(localStorage.getItem(KEY));
  // system -> light -> dark -> system, so a reader can always get
  // back to following the machine.
  window.janusTheme = function () {
    var next = { '': 'light', light: 'dark', dark: '' }[
      root.getAttribute('data-theme') || ''
    ];
    if (next) { localStorage.setItem(KEY, next); } else { localStorage.removeItem(KEY); }
    apply(next);
  };
})();
"#;

pub const STYLE: &str = "
/* Dark is the base. Every colour below goes through a token, so a
   theme is one block of tokens rather than overrides scattered
   through the sheet: that is what let the light theme ship as a
   palette instead of a second stylesheet. */
:root {
  color-scheme: dark;
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
  --field: #101017;
  --field-line: #34343f;
  --button: #1e2a3a;
  --button-line: #3a5680;
  --button-text: #f4f4f8;
  --button-hover: #27374d;
  --hover: #1c1c25;
  --code: #a8cf9a;
  --chip: #1e1e27;
  --chip-good: #14231a;
  --chip-good-line: #2f5c3d;
  --chip-bad: #241614;
  --chip-bad-line: #6b3630;
  --chip-busy: #221e12;
  --chip-busy-line: #5e4f26;
  --note: #16202e;
  --ui: system-ui, -apple-system, 'Segoe UI', sans-serif;
  --mono: ui-monospace, 'Cascadia Code', Menlo, monospace;
}
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.55 var(--mono);
  background: var(--ground); color: var(--text); }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

header { display: flex; gap: 1rem; align-items: center; padding: .7rem 1.25rem;
  border-bottom: 1px solid var(--line); background: var(--raised);
  position: sticky; top: 0; z-index: 3; }
header .title { font-weight: 700; color: var(--bright); font-family: var(--ui);
  font-size: .95rem; letter-spacing: -.01em; }
header .icon { margin-left: auto; }

/* Navigation down the left, data filling the rest: the shape an
   operator already knows, and it leaves the whole width for the thing
   they came to look at. */
.frame { display: grid; grid-template-columns: 14rem minmax(0, 1fr);
  min-height: calc(100vh - 3.1rem); align-items: start; }
.column { display: flex; flex-direction: column; min-height: calc(100vh - 3.1rem);
  min-width: 0; }
.rail { position: sticky; top: 3.1rem; padding: 1.25rem .75rem;
  border-right: 1px solid var(--line); background: var(--raised);
  min-height: calc(100vh - 3.1rem); }
.rail-heading { font: 600 .68rem/1.4 var(--ui); text-transform: uppercase;
  letter-spacing: .09em; color: var(--faint); padding: 0 .6rem; margin: 0 0 .4rem; }
.rail-heading + nav { margin-bottom: 1.4rem; }
.rail nav { display: grid; gap: .1rem; }
.rail nav a { font-family: var(--ui); font-size: .88rem; padding: .35rem .6rem;
  border-radius: 4px; color: var(--text); }
.rail nav a:hover { background: var(--hover); text-decoration: none; }
.rail nav a.here { background: var(--chip); color: var(--bright); font-weight: 600;
  box-shadow: inset 2px 0 0 var(--accent); }

main { padding: 1.5rem 1.75rem 3rem; max-width: 110rem; min-width: 0; flex: 1; }

/* A page should end rather than stop. */
footer { display: flex; gap: 1.25rem; align-items: baseline; flex-wrap: wrap;
  padding: 1rem 1.75rem; border-top: 1px solid var(--line);
  font: .78rem/1.5 var(--ui); color: var(--soft); }
footer .dim { margin-left: auto; }

/* Actions sit above the data as the things you can do, rather than
   below it as forms nobody asked to see. */
.toolbar { display: flex; gap: .5rem; flex-wrap: wrap; margin: .9rem 0 1.1rem; }

/* The reference: one operation per row, and every face it reaches. */
table.faces td { white-space: nowrap; }
table.faces td:first-child { font-family: var(--ui); }
.shape { display: grid; gap: 1.25rem; margin: .75rem 0 1.75rem;
  grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr)); align-items: start; }
.shape-heading { font: 600 .7rem/1.4 var(--ui); text-transform: uppercase;
  letter-spacing: .08em; color: var(--faint); margin-bottom: .35rem; }
ul.plain { list-style: none; margin: 0; padding: 0; display: grid; gap: .2rem;
  font-size: .85rem; }
ul.plain li { display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; }
.req { color: var(--bad); }
ul.notes { margin-top: .45rem; color: var(--soft); font-size: .8rem; }
ul.notes li { font-family: var(--mono); }
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
  color: var(--soft); }
tbody tr:last-child td { border-bottom: 0; }
/* A label/value table: the label hugs its text rather than taking
   half the page and leaving the value stranded. */
table.fields th { width: 1px; white-space: nowrap; padding-right: 2rem;
  color: var(--soft); font-weight: 600; }
tbody tr:hover td { background: var(--hover); }
td { font-variant-numeric: tabular-nums; }
code { color: var(--code); }
.dim { color: var(--faint); }
.count { font-family: var(--ui); color: var(--soft); font-size: .8rem; margin: .5rem 0 0; }
.card .count { margin: .1rem 0 .4rem; }
ul.preview { list-style: none; margin: 0; padding: 0; display: grid; gap: .15rem; }
/* min-width:0 on the row as well as the label: a grid item defaults
   to min-width:auto and refuses to shrink below its content, so the
   row overflowed the card and clipped the time however the label was
   styled. */
ul.preview li { display: flex; gap: .5rem; align-items: baseline; min-width: 0;
  font-size: .85rem; color: var(--text); }
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
  font-size: .78rem; border: 1px solid var(--line); background: var(--chip);
  color: var(--soft); }
.chip-good { color: var(--good); border-color: var(--chip-good-line);
  background: var(--chip-good); }
.chip-bad { color: var(--bad); border-color: var(--chip-bad-line);
  background: var(--chip-bad); }
.chip-busy { color: var(--busy); border-color: var(--chip-busy-line);
  background: var(--chip-busy); }
.chip-gone { color: var(--faint); }
.chip-yes { color: var(--good); border-color: var(--chip-good-line); }
.chip-no { color: var(--faint); }

.cards { display: grid; gap: .75rem; margin: .5rem 0;
  grid-template-columns: repeat(auto-fill, minmax(18rem, 1fr)); }
.card { border: 1px solid var(--line); border-radius: 3px; padding: .8rem 1rem;
  background: var(--raised); }
.card .name { font-weight: 700; }

form.inline { display: flex; gap: .6rem; flex-wrap: wrap; align-items: end;
  margin: .5rem 0 1.25rem; padding: .8rem .9rem; border: 1px solid var(--line);
  border-radius: 4px; background: var(--raised); }
form.inline button { margin-top: .1rem; }
label { display: flex; flex-direction: column; gap: .2rem;
  font: .75rem/1.4 var(--ui); color: var(--soft); }
input, select, textarea, button { font: inherit; font-family: var(--mono);
  background: var(--field); color: var(--text); border: 1px solid var(--field-line);
  border-radius: 3px; padding: .35rem .55rem; }
input:focus, select:focus, textarea:focus { border-color: var(--accent); outline: none; }
button { cursor: pointer; background: var(--button); border-color: var(--button-line);
  color: var(--button-text); font-family: var(--ui); font-weight: 600;
  padding: .38rem .9rem; }
button:hover { background: var(--button-hover); }
button.ghost { background: transparent; color: var(--text);
  border-color: var(--field-line); }
button.ghost:hover { background: var(--hover); border-color: var(--button-line); }
button.icon { background: transparent; border-color: transparent; color: var(--soft);
  padding: .25rem .4rem; display: inline-flex; align-items: center; }
button.icon:hover { background: var(--hover); color: var(--text); }
.choices { display: flex; gap: .7rem; flex-wrap: wrap; align-items: center;
  padding: .25rem 0; }
.choice { flex-direction: row; align-items: center; gap: .3rem; color: var(--text);
  font-family: var(--mono); font-size: .85rem; cursor: pointer; }
.choice input { margin: 0; accent-color: var(--accent); }

.banner { border: 1px solid var(--button-line); background: var(--note);
  padding: .6rem .85rem; border-radius: 3px; margin-bottom: 1rem;
  font-family: var(--ui); }
.error { border-color: var(--chip-bad-line); background: var(--chip-bad); }

/* A form arrives over the page when it is asked for. The browser
   already knows about the backdrop, the escape key, and the focus. */
dialog { border: 1px solid var(--line); border-radius: 6px; background: var(--raised);
  color: var(--text); padding: 0; width: min(34rem, calc(100vw - 2rem));
  box-shadow: 0 18px 50px rgba(0, 0, 0, .45); }
dialog::backdrop { background: rgba(0, 0, 0, .55); }
dialog form { display: grid; gap: .7rem; padding: 1.1rem 1.25rem 1.25rem; }
dialog p.dim { margin: 0; font-size: .85rem; }
dialog input, dialog select, dialog textarea { width: 100%; }
.dialog-head { display: flex; align-items: center; gap: 1rem;
  margin: 0 0 .2rem; }
.dialog-head h2 { margin: 0; font: 600 1rem/1.3 var(--ui); text-transform: none;
  letter-spacing: 0; color: var(--bright); }
.dialog-head .icon { margin-left: auto; font-size: 1.1rem; line-height: 1; }
/* The submit is an act, so it stands apart from the fields above it
   rather than butting against the last one. */
.dialog-foot { display: flex; justify-content: flex-end; gap: .5rem;
  margin-top: .5rem; padding-top: .9rem; border-top: 1px solid var(--line); }
pre { background: var(--field); border: 1px solid var(--line); border-radius: 3px;
  padding: .8rem; overflow-x: auto; }

@media (max-width: 60rem) {
  .frame { grid-template-columns: minmax(0, 1fr); }
  .rail { position: static; min-height: 0; border-right: 0;
    border-bottom: 1px solid var(--line); }
  .rail nav { grid-auto-flow: column; grid-auto-columns: max-content;
    overflow-x: auto; gap: .3rem; }
  .rail-heading + nav { margin-bottom: .8rem; }
  main { padding: 1.25rem 1rem 3rem; }
}

/* Daylight. Written twice on purpose: once for a reader whose system
   says so, once for a reader who said so here, and CSS has no way to
   share a block between a media query and a selector. Both carry only
   tokens, so nothing else in the sheet needs a second version.
   `:not([data-theme])` is what lets the control win over the system. */
@media (prefers-color-scheme: light) {
  :root:not([data-theme]) {
    color-scheme: light;
    --ground: #fbfbfc; --raised: #ffffff; --line: #e2e2e8;
    --text: #22222a; --soft: #63636f; --faint: #93939f; --bright: #0d0d12;
    --accent: #2c5fc4; --good: #1f7a3d; --bad: #b3372a; --busy: #8a6412;
    --field: #ffffff; --field-line: #cfcfd8;
    --button: #e8eefb; --button-line: #b6c6e6; --button-text: #143a7d;
    --button-hover: #dbe5f8; --hover: #f2f2f6; --code: #2f6b2a;
    --chip: #f1f1f5;
    --chip-good: #e8f5ec; --chip-good-line: #b6ddc3;
    --chip-bad: #fdeceb; --chip-bad-line: #eec2bd;
    --chip-busy: #fbf3df; --chip-busy-line: #e6d5a6;
    --note: #eef3fd;
  }
}
:root[data-theme=light] {
  color-scheme: light;
  --ground: #fbfbfc; --raised: #ffffff; --line: #e2e2e8;
  --text: #22222a; --soft: #63636f; --faint: #93939f; --bright: #0d0d12;
  --accent: #2c5fc4; --good: #1f7a3d; --bad: #b3372a; --busy: #8a6412;
  --field: #ffffff; --field-line: #cfcfd8;
  --button: #e8eefb; --button-line: #b6c6e6; --button-text: #143a7d;
  --button-hover: #dbe5f8; --hover: #f2f2f6; --code: #2f6b2a;
  --chip: #f1f1f5;
  --chip-good: #e8f5ec; --chip-good-line: #b6ddc3;
  --chip-bad: #fdeceb; --chip-bad-line: #eec2bd;
  --chip-busy: #fbf3df; --chip-busy-line: #e6d5a6;
  --note: #eef3fd;
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
            ["reference"] => self.reference(),
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
                h2 { "Queries" }
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

    /// The contract, as the surface it becomes.
    ///
    /// A service built this way declares its shape once and janus
    /// lands it on REST, GraphQL, and MCP by rules nobody should have
    /// to hold in their head. Reading the OpenAPI document tells you
    /// the REST half; reading the SDL tells you the GraphQL half; the
    /// mapping between them lives in the generator and nowhere a
    /// caller can see it.
    ///
    /// So this page says it: for every operation the contract
    /// declares, the path a REST caller takes, the field a GraphQL
    /// caller selects, the tool an agent calls, what it accepts, what
    /// it costs, and what it requires. Derived from the same
    /// functions the generators use, so it cannot drift from the
    /// documents.
    fn reference(&self) -> ConsoleAnswer {
        let contract = self.dispatcher.contract().clone();
        let base = &self.config.base;
        let body = html! {
            h1 { "Reference" }
            p.dim {
                "Every operation " (contract.name) " v" (contract.version)
                " declares, the request each face takes, and where to run it."
            }

            @for resource in &contract.resources {
                h2 { (humanize(&resource.name)) }
                @let shown: Vec<&str> = resource
                    .fields
                    .iter()
                    .take(3)
                    .map(|f| f.api_name())
                    .collect();
                // Each nesting level indents by two, so a page's fields
                // sit deeper than an instance's.
                @let page_selection = format!(
                    "items {{\n      {}\n    }}\n    nextCursor",
                    shown.join("\n      "),
                );
                @let one_selection = format!("id\n    {}", shown.join("\n    "));
                @let list_inputs = vec![];
                @let id_input = vec![ActionField {
                    name: "id".to_owned(),
                    kind: TypeRef::String,
                    required: true,
                    multiple: false,
                    options: vec![],
                    description: None,
                }];
                @let faces = {
                    let mut faces: Vec<Face> = Vec::new();
                    faces.push(Face {
                        what: "List".to_owned(),
                        rest_method: "GET",
                        rest_path: format!("/v1/{}", resource.name),
                        graphql: Some(resource.graphql_list_field()),
                        graphql_kind: "query",
                        tool: Some(format!("{}_list", resource.name)),
                        requires: &resource.reads_require,
                        inputs: &list_inputs,
                        body: false,
                        takes_id: false,
                        selection: Some(page_selection.clone()),
                        try_at: Some(format!("{base}/r/{}", resource.name)),
                    });
                    faces.push(Face {
                        what: "Get one".to_owned(),
                        rest_method: "GET",
                        rest_path: format!("/v1/{}/{{id}}", resource.name),
                        graphql: Some(resource.graphql_get_field()),
                        graphql_kind: "query",
                        tool: Some(format!("{}_get", singular(&resource.name))),
                        requires: &resource.reads_require,
                        inputs: &id_input,
                        body: false,
                        takes_id: true,
                        selection: Some(one_selection.clone()),
                        try_at: Some(format!("{base}/r/{}", resource.name)),
                    });
                    if resource.watchable {
                        faces.push(Face {
                            what: "Watch".to_owned(),
                            rest_method: "",
                            rest_path: String::new(),
                            graphql: Some(resource.graphql_watch_field()),
                            graphql_kind: "subscription",
                            tool: None,
                            requires: &resource.reads_require,
                            inputs: &list_inputs,
                            body: false,
                            takes_id: false,
                            selection: Some(one_selection.clone()),
                            try_at: None,
                        });
                    }
                    for sub in &resource.sub_resources {
                        faces.push(Face {
                            what: humanize(&sub.name),
                            rest_method: "GET",
                            rest_path: format!("/v1/{}/{{id}}/{}", resource.name, sub.name),
                            graphql: None,
                            graphql_kind: "query",
                            tool: None,
                            requires: &resource.reads_require,
                            inputs: &id_input,
                            body: false,
                            takes_id: true,
                            selection: None,
                            try_at: Some(format!("{base}/r/{}", resource.name)),
                        });
                    }
                    for action in &resource.actions {
                        faces.push(Face {
                            what: action_label(action, None),
                            rest_method: match action.method.as_str() {
                                "POST" => "POST",
                                "PUT" => "PUT",
                                "PATCH" => "PATCH",
                                "DELETE" => "DELETE",
                                _ => "POST",
                            },
                            rest_path: format!("/v1/{}{}", resource.name, action.path),
                            graphql: Some(action.graphql_field_name(resource)),
                            graphql_kind: "mutation",
                            tool: Some(format!("{}_{}", singular(&resource.name), action.name)),
                            requires: &action.requires,
                            inputs: &action.input,
                            body: action.method != "DELETE" && !action.input.is_empty(),
                            takes_id: action.takes_id(),
                            // An action answering the resource returns
                            // an object, so the document needs a
                            // selection or it will not parse. JSON and
                            // Boolean are scalars and must not carry
                            // one.
                            selection: match action.output {
                                crate::ir::ActionOutput::Resource => {
                                    Some(one_selection.clone())
                                }
                                _ => None,
                            },
                            try_at: Some(format!(
                                "{base}/r/{}?open={}",
                                resource.name, action.name
                            )),
                        });
                    }
                    faces
                };
                (faces_table(&faces))
                (self.shape_of(resource))
            }

            @if !contract.queries.is_empty() {
                h2 { "Queries" }
                @let query_faces: Vec<Face> = contract
                    .queries
                    .iter()
                    .map(|query| Face {
                        what: humanize(&query.name),
                        rest_method: "GET",
                        rest_path: query.path.clone(),
                        graphql: Some(query.graphql_field_name()),
                        graphql_kind: "query",
                        tool: Some(query.name.clone()),
                        requires: &query.requires,
                        inputs: &query.input,
                        body: false,
                        takes_id: query.path.contains("{id}"),
                        selection: None,
                        try_at: Some(format!("{base}/q/{}", query.name)),
                    })
                    .collect();
                (faces_table(&query_faces))
            }

            @if !contract.rate_classes.is_empty() {
                h2 { "Rate classes" }
                div.scroll {
                    table.faces {
                        thead { tr { th { "Class" } th { "Units per minute" } } }
                        tbody {
                            @for class in &contract.rate_classes {
                                tr {
                                    td { code { (class.name) } }
                                    td { (class.units_per_minute) }
                                }
                            }
                        }
                    }
                }
            }

            @if let Some(limits) = &contract.limits {
                h2 { "Ceilings" }
                ul.plain {
                    @if let Some(depth) = limits.max_depth {
                        li { "GraphQL selection depth: " (depth) }
                    }
                    @if let Some(complexity) = limits.max_complexity {
                        li { "GraphQL selection count: " (complexity) }
                    }
                    @if let Some(watches) = limits.max_watches_per_principal {
                        li { "Open subscriptions per principal: " (watches) }
                    }
                }
            }
        };
        self.shell_at(200, "Reference", Some("__reference"), body)
    }

    /// What one resource hands back, and how a caller may narrow it.
    fn shape_of(&self, resource: &Resource) -> Markup {
        html! {
            div.shape {
                div {
                    div.shape-heading { "Fields" }
                    ul.plain {
                        @for field in &resource.fields {
                            li {
                                code { (field.api_name()) }
                                @if field.rename.is_some() {
                                    span.dim { " from " (field.column) }
                                }
                                @if let Some(guard) = &field.guard {
                                    span.chip.chip-busy { "guarded: " (guard) }
                                }
                            }
                        }
                    }
                }
                div {
                    div.shape-heading { "Narrowing" }
                    ul.plain {
                        @if resource.filterable.is_empty() && resource.sortable.is_empty() {
                            li.dim { "Nothing declared" }
                        }
                        @for column in &resource.filterable {
                            li {
                                "Filter " code { (column) }
                                @if let Some(options) = resource.filter_options.get(column) {
                                    span.dim { " one of " (options.join(", ")) }
                                }
                            }
                        }
                        @for column in &resource.sortable {
                            li { "Sort " code { (column) } }
                        }
                        li.dim { "Page size at most " (resource.max_page_size) }
                    }
                }
            }
        }
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
        let mut opened: Option<String> = None;
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
                // The reference links here to run an action, and
                // arriving next to the button is not the same as
                // arriving at the form.
                "open" => opened = Some(value.clone()),
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
            h1 { (humanize(name)) }
            @if let Some(action) = &done {
                div.banner { "action " (action) " completed" }
            }
            @if !collection_actions.is_empty() {
                div.toolbar {
                    @for action in &collection_actions {
                        (self.action_form(name, None, action))
                    }
                }
                @if let Some(open) = &opened {
                    script {
                        (PreEscaped(format!(
                            "document.getElementById('act-{name}-{open}')?.showModal()"
                        )))
                    }
                }
            }
            @if !resource.filterable.is_empty() || !resource.sortable.is_empty() {
                form.inline method="get" action=(format!("{}/r/{}", self.config.base, name)) {
                    @for column in &resource.filterable {
                        @let chosen = filter_state
                            .get(column)
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        label {
                            (humanize(column))
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
                            "Sort"
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
                    button { "Narrow" }
                }
            }
            div.scroll {
              table {
                thead { tr { @for column in &columns { th { (humanize(column)) } } } }
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
        };
        self.shell_at(200, name, Some(name), body)
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
            h1 { (humanize(name)) " / " (id) }
            @if !instance_actions.is_empty() {
                div.toolbar {
                    @for action in &instance_actions {
                        (self.action_form(name, Some(id), action))
                    }
                }
            }
            @if let Some(action) = &done {
                div.banner { "action " (action) " completed" }
            }
            div.scroll {
                table.fields {
                    @for field in &resource.fields {
                        tr {
                            th { (humanize(field.api_name())) }
                            td { (cell(field.api_name(), row.get(field.api_name()))) }
                        }
                    }
                }
            }
            @for (sub_name, items) in &subs {
                h2 { (humanize(sub_name)) }
                @if items.is_empty() { p.dim { "nothing listed" } } @else {
                    (object_table(items))
                }
            }
        };
        self.shell_at(200, &format!("{name}/{id}"), Some(name), body)
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
            h1 { (humanize(name)) }
            @if let Some(text) = &query.description { p.dim { (text) } }
            form.inline method="get" action=(format!("{}/q/{}", self.config.base, name)) {
                @for field in &query.input {
                    label {
                        (humanize(&field.name)) @if field.required { " *" }
                        (input_for(field, pairs))
                    }
                }
                button { "Run" }
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
        self.shell_at(200, name, Some(name), body)
    }

    /// One action, as a button that opens its form.
    ///
    /// Every action's form used to lie open below the data, so a
    /// resource with five of them buried its own listing under a wall
    /// of inputs and the page had no shape. A form is something a
    /// caller goes to when they mean to act, so it waits behind the
    /// button that names it and arrives over the page when asked for.
    ///
    /// `<dialog>` rather than a hand-built overlay: the browser
    /// already knows about a backdrop, the escape key, and where the
    /// focus goes.
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
        // Instance actions already have their subject on the page and
        // would only repeat it.
        let label = action_label(action, id.is_none().then_some(resource));
        let handle = format!("act-{}-{}", resource, action.name);
        html! {
            button.ghost type="button"
                onclick=(format!("document.getElementById('{handle}').showModal()")) {
                (label)
            }
            dialog id=(handle) {
                form method="post" action=(target) {
                    div.dialog-head {
                        h2 { (label) }
                        button.icon type="button" aria-label="Close"
                            onclick=(format!("document.getElementById('{handle}').close()")) {
                            "\u{00d7}"
                        }
                    }
                    @if let Some(text) = &action.description { p.dim { (text) } }
                    @for field in &action.input {
                        label {
                            (humanize(&field.name)) @if field.required { " *" }
                            (input_for(field, &[]))
                        }
                    }
                    div.dialog-foot { button { (label) } }
                }
            }
        }
    }

    /// What an action answered, shown rather than discarded. A
    /// secret that appears once appears here, and nowhere later.
    fn answer_page(&self, action: &str, value: &Value, back: &str) -> ConsoleAnswer {
        let body = html! {
            h1 { (humanize(action)) }
            div.banner { "completed" }
            @if let Some(map) = value.as_object() {
                div.scroll {
                    table.fields {
                        @for (key, field) in map {
                            tr {
                                th { (humanize(key)) }
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
            h1 { "Refused" }
            div.banner.error { (message) }
            p { a href=(&self.config.base) { "back to the overview" } }
        };
        self.shell(status, "refused", body)
    }

    fn shell(&self, status: u16, title: &str, content: Markup) -> ConsoleAnswer {
        self.shell_at(status, title, None, content)
    }

    /// The page, in its frame.
    ///
    /// Navigation lives in a rail down the left and the data fills the
    /// rest, which is the shape an operator already knows from every
    /// console they use. A horizontal strip of links above a column of
    /// floating cards reads as a document; this reads as a place to
    /// work, and it leaves the whole width for the thing they came to
    /// look at.
    fn shell_at(
        &self,
        status: u16,
        title: &str,
        active: Option<&str>,
        content: Markup,
    ) -> ConsoleAnswer {
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
                    script { (PreEscaped(THEME_SCRIPT)) }
                }
                body {
                    header {
                        span.title {
                            a href=(&self.config.base) { (self.config.title) }
                        }
                        button.icon type="button" onclick="janusTheme()"
                            title="Appearance: follow the system, or force light or dark"
                            aria-label="Appearance" {
                            (PreEscaped(THEME_ICON))
                        }
                    }
                    div.frame {
                        aside.rail {
                            @if !contract.resources.is_empty() {
                                div.rail-heading { "Resources" }
                                nav {
                                    @for resource in &contract.resources {
                                        @let here = active == Some(resource.name.as_str());
                                        a.here[here]
                                            href=(format!("{}/r/{}", self.config.base, resource.name)) {
                                            (humanize(&resource.name))
                                        }
                                    }
                                }
                            }
                            div.rail-heading { "Contract" }
                            nav {
                                @let here = active == Some("__reference");
                                a.here[here] href=(format!("{}/reference", self.config.base)) {
                                    "Reference"
                                }
                            }
                            @if !contract.queries.is_empty() {
                                div.rail-heading { "Queries" }
                                nav {
                                    @for query in &contract.queries {
                                        @let here = active == Some(query.name.as_str());
                                        a.here[here]
                                            href=(format!("{}/q/{}", self.config.base, query.name)) {
                                            (humanize(&query.name))
                                        }
                                    }
                                }
                            }
                        }
                        div.column {
                            main { (content) }
                            footer {
                                span {
                                    (contract.name) " v" (contract.version)
                                }
                                a href=(format!("{}/reference", self.config.base)) {
                                    "Reference"
                                }
                                span.dim { "Generated from the contract by janus" }
                            }
                        }
                    }
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
                thead { tr { @for column in &columns { th { (humanize(column)) } } } }
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

/// One operation, on every face it reaches.
///
/// Built once and rendered twice: as a row in the table, and as the
/// request a caller would actually send. Holding it in one place is
/// what keeps the two from disagreeing.
struct Face<'a> {
    what: String,
    rest_method: &'static str,
    rest_path: String,
    graphql: Option<String>,
    graphql_kind: &'static str,
    tool: Option<String>,
    requires: &'a [String],
    inputs: &'a [ActionField],
    /// Whether the inputs travel as a JSON body. A GET carries them in
    /// the query string, and showing a body on one would be teaching
    /// the wrong request.
    body: bool,
    takes_id: bool,
    /// The selection a GraphQL caller would write, when the answer has
    /// a declared shape rather than open JSON.
    selection: Option<String>,
    /// Where in this console the operation can actually be run.
    try_at: Option<String>,
}

/// A placeholder that shows the type rather than pretending to be a
/// value, since a caller copying this has to substitute anyway.
fn sample(field: &ActionField) -> String {
    if let Some(first) = field.options.first() {
        return if field.multiple {
            format!("\"{}\"", field.options.join(","))
        } else {
            format!("\"{first}\"")
        };
    }
    match field.kind {
        TypeRef::Int => "0".to_owned(),
        TypeRef::Bool => "false".to_owned(),
        TypeRef::Json => "{}".to_owned(),
        TypeRef::String => format!("\"<{}>\"", field.name),
    }
}

impl Face<'_> {
    /// The request a REST caller sends.
    fn rest_example(&self) -> String {
        let mut out = String::new();
        if self.body {
            out.push_str(&format!("{} {}\n", self.rest_method, self.rest_path));
            out.push_str("content-type: application/json\n");
            let carried: Vec<&ActionField> = self
                .inputs
                .iter()
                .filter(|f| !(self.takes_id && f.name == "id"))
                .collect();
            if carried.is_empty() {
                out.push_str("\n{}");
            } else {
                out.push_str("\n{\n");
                for (index, field) in carried.iter().enumerate() {
                    out.push_str(&format!("  \"{}\": {}", field.name, sample(field)));
                    if index + 1 < carried.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push('}');
            }
        } else {
            let query: Vec<String> = self
                .inputs
                .iter()
                .filter(|f| !self.rest_path.contains(&format!("{{{}}}", f.name)))
                .map(|f| format!("{}={}", f.name, sample(f).trim_matches('"')))
                .collect();
            out.push_str(&format!("{} {}", self.rest_method, self.rest_path));
            if !query.is_empty() {
                out.push('?');
                out.push_str(&query.join("&"));
            }
        }
        out
    }

    /// The document a GraphQL caller sends.
    fn graphql_example(&self) -> Option<String> {
        let field = self.graphql.as_ref()?;
        let mut arguments: Vec<String> = Vec::new();
        // An instance action carries its subject in the path on REST
        // and as an argument on GraphQL, so the id is not among the
        // declared inputs and has to be said here. Without it
        // `mutation { fileRemove }` does not even parse.
        if self.takes_id && !self.inputs.iter().any(|f| f.name == "id") {
            arguments.push("id: \"<id>\"".to_owned());
        }
        arguments.extend(
            self.inputs
                .iter()
                .map(|f| format!("{}: {}", camel(&f.name), sample(f))),
        );
        let call = if arguments.is_empty() {
            field.clone()
        } else {
            format!("{field}({})", arguments.join(", "))
        };
        let selection = self
            .selection
            .clone()
            .unwrap_or_else(|| "# answers JSON".to_owned());
        Some(if self.selection.is_some() {
            format!(
                "{} {{\n  {call} {{\n    {selection}\n  }}\n}}",
                self.graphql_kind
            )
        } else {
            format!("{} {{\n  {call}   {selection}\n}}", self.graphql_kind)
        })
    }

    /// What each input means, said beside the request rather than
    /// inside it.
    ///
    /// These were comments in the body until a copy-paste proved the
    /// point: `// optional` is not JSON, so the example a caller
    /// lifted straight into curl could not be sent.
    fn notes(&self) -> Vec<String> {
        self.inputs
            .iter()
            .filter_map(|field| {
                let mut said: Vec<String> = Vec::new();
                if !field.required {
                    said.push("optional".to_owned());
                }
                if !field.options.is_empty() {
                    said.push(format!(
                        "{} of {}",
                        if field.multiple { "any" } else { "one" },
                        field.options.join(", "),
                    ));
                }
                (!said.is_empty()).then(|| format!("{}: {}", field.name, said.join("; ")))
            })
            .collect()
    }

    /// The call an agent makes.
    fn tool_example(&self) -> Option<String> {
        let tool = self.tool.as_ref()?;
        let mut arguments: Vec<String> = Vec::new();
        if self.takes_id && !self.inputs.iter().any(|f| f.name == "id") {
            arguments.push("    \"id\": \"<id>\"".to_owned());
        }
        arguments.extend(
            self.inputs
                .iter()
                .map(|f| format!("    \"{}\": {}", f.name, sample(f))),
        );
        Some(format!(
            "{{\n  \"name\": \"{tool}\",\n  \"arguments\": {{\n{}\n  }}\n}}",
            arguments.join(",\n")
        ))
    }
}

/// The operations, and under each the request every face takes.
///
/// A mapping alone answers "where does this live"; a caller's next
/// question is always "what do I send", and answering it anywhere but
/// here means they go and read a document instead.
fn faces_table(faces: &[Face]) -> Markup {
    html! {
        div.scroll {
            table.faces {
                thead {
                    tr {
                        th { "Operation" } th { "REST" } th { "GraphQL" }
                        th { "MCP tool" } th { "Requires" } th { "" }
                    }
                }
                tbody {
                    @for face in faces {
                        tr {
                            td { (face.what) }
                            td {
                                @if face.rest_path.is_empty() {
                                    span.dim { "no REST face" }
                                } @else {
                                    code { (face.rest_method) " " (face.rest_path) }
                                }
                            }
                            td {
                                @match &face.graphql {
                                    Some(field) => code { (field) },
                                    None => span.dim { "on the parent type" },
                                }
                            }
                            td {
                                @match &face.tool {
                                    Some(tool) => code { (tool) },
                                    None => span.dim { "no tool" },
                                }
                            }
                            td { (scopes(face.requires)) }
                            td {
                                @if let Some(link) = &face.try_at {
                                    a.try href=(link) { "Try it" }
                                }
                            }
                        }
                        tr.detail {
                            td colspan="6" {
                                details {
                                    summary { "Request" }
                                    div.requests {
                                        @if !face.rest_path.is_empty() {
                                            div {
                                                div.shape-heading { "REST" }
                                                pre { code { (face.rest_example()) } }
                                                @let notes = face.notes();
                                                @if !notes.is_empty() {
                                                    ul.plain.notes {
                                                        @for note in &notes {
                                                            li { (note) }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        @if let Some(document) = face.graphql_example() {
                                            div {
                                                div.shape-heading { "GraphQL" }
                                                pre { code { (document) } }
                                            }
                                        }
                                        @if let Some(call) = face.tool_example() {
                                            div {
                                                div.shape-heading { "MCP" }
                                                pre { code { (call) } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Scopes a caller must hold, or the fact that none are asked for.
fn scopes(required: &[String]) -> Markup {
    html! {
        @if required.is_empty() {
            span.dim { "open" }
        } @else {
            @for scope in required { span.chip { (scope) } }
        }
    }
}

/// A declared name, said the way a person writes it.
///
/// Contracts name things for machines: `content_type`, `issue_url`,
/// `file_text`. Printing those raw is what made the console read as a
/// dump of the IR. Sentence case, because a label is a phrase rather
/// than a headline, with the acronyms an operator would never see
/// lowercased.
fn humanize(name: &str) -> String {
    const ACRONYMS: [(&str, &str); 8] = [
        ("url", "URL"),
        ("id", "ID"),
        ("ttl", "TTL"),
        ("s3", "S3"),
        ("api", "API"),
        ("mcp", "MCP"),
        ("http", "HTTP"),
        ("uri", "URI"),
    ];
    let mut out = String::new();
    for (index, word) in name.split(['_', '-']).filter(|w| !w.is_empty()).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        match ACRONYMS
            .iter()
            .find(|(raw, _)| *raw == word.to_ascii_lowercase())
        {
            Some((_, shown)) => out.push_str(shown),
            None if index == 0 => {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            }
            None => out.push_str(word),
        }
    }
    out
}

/// What a control says it will do.
///
/// "Create" alone names nothing: an operator reading a row of cards
/// has to fall through to the description to learn what each one
/// makes. A collection action takes the thing it acts on, so the
/// button reads "Create file". An instance action already has its
/// subject on the page and would only repeat it.
fn action_label(action: &Action, resource: Option<&str>) -> String {
    let verb = humanize(&action.name);
    match resource {
        Some(name) => format!("{verb} {}", singular(name)),
        None => verb,
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

#[cfg(test)]
mod naming {
    use super::*;

    /// Contracts name things for machines. A console is read by
    /// people, so `content_type` is a column called "Content type"
    /// and `issue_url` is a button that says "Issue URL".
    #[test]
    fn declared_names_are_said_the_way_people_write_them() {
        assert_eq!(humanize("content_type"), "Content type");
        assert_eq!(humanize("issue_upload_url"), "Issue upload URL");
        assert_eq!(humanize("file_text"), "File text");
        assert_eq!(humanize("files"), "Files");
        assert_eq!(humanize("id"), "ID");
        assert_eq!(humanize(""), "");
    }

    /// "Create" alone names nothing: a row of cards all reading
    /// "create" made an operator fall through to the description to
    /// learn what each one made.
    #[test]
    fn a_collection_action_says_what_it_acts_on() {
        let action = Action {
            name: "create".into(),
            method: "POST".into(),
            path: "".into(),
            input: vec![],
            output: crate::ir::ActionOutput::Json,
            description: None,
            graphql_field: None,
            requires: vec![],
            rate_class: None,
        };
        assert_eq!(action_label(&action, Some("files")), "Create file");
        // An instance action has its subject on the page already and
        // would only repeat it.
        assert_eq!(action_label(&action, None), "Create");
    }
}

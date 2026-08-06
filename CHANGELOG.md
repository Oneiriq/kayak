# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Janus has not cut a release yet. Everything below is the road to 0.1.0.

## [Unreleased]

### Added

- **A reference page: the contract, as the surface it becomes.** A
  service declares its shape once and janus lands it on REST,
  GraphQL, and MCP by rules nobody should have to hold in their head.
  The OpenAPI document gives the REST half, the SDL gives the GraphQL
  half, and until now nothing put them side by side.

  For every operation the contract declares, the page shows the path a
  REST caller takes, the field a GraphQL caller selects, the tool an
  agent calls, the scopes it requires, and its rate class. Each
  resource carries what it hands back, including a field's underlying
  column when it is renamed and a guard when one applies, and how a
  caller may narrow it, including a filter's own option list and the
  page-size ceiling. It reads the same functions the generators read,
  so it cannot drift from the documents.

- **A footer**, carrying the contract's name and version, so a page
  ends rather than stops.

### Changed

- **The console has a shape.** Navigation moved into a rail down the
  left and the data fills the rest, which is what an operator already
  knows from every console they use and leaves the whole width for the
  thing they came to look at. The rail marks where they are.

- **Actions are things you can do, rather than forms nobody asked to
  see.** Every action's form used to lie open below the data, so a
  file with six of them buried its own record under a wall of inputs.
  Each action is now a button in a toolbar beside the heading, and its
  form arrives in a `<dialog>` when asked for: the browser already
  knows about the backdrop, the escape key, and where the focus goes.

- **Declared names are said the way people write them.** A contract
  names things for machines, and printing `content_type` and
  `issue_url` raw is what made the console read as a dump of the IR.
  Columns, labels, headings, and navigation are sentence case now,
  with the acronyms an operator would never see lowercased.

  A control also says what it will do. "Create" alone names nothing,
  so a collection action takes the thing it acts on and reads "Create
  file"; the submit inside the dialog says the same rather than
  "perform". An instance action already has its subject on the page
  and would only repeat it.

- **The appearance control is a mark rather than a word**, since
  switching light and dark says itself faster as a half-lit circle
  than as the word "theme" set beside the navigation.

- **A submit stands apart from the fields above it**, on its own
  footer with a rule, instead of butting against the last input.

### Added

- **Several of a set.** `ActionField.multiple` says a caller may name
  more than one of `options`. The value travels as one comma-separated
  string, which is what a query parameter carries without ceremony,
  and every part answers to the set rather than the joined whole. The
  console renders checkboxes, since a menu that allows several hides
  that it does and wants a modifier key to work; the OpenAPI document
  and the MCP manifest render an array of the enum. `validate` refuses
  `multiple` on a field that lists no options, which would be several
  of nothing.

- **A light theme, and a control that overrides it.** Daylight arrives
  on its own under `prefers-color-scheme: light`, since an operator on
  a bright screen reading a black page is the same problem as the
  reverse. A `theme` control in the header cycles system, light, dark,
  and remembers the choice, because following the machine is a good
  default rather than an answer for everyone.

  The whole sheet moved onto tokens to make this cheap: every colour
  reads a custom property, so a theme is one block of tokens and
  nothing else in the sheet needs a second version. The control writes
  `data-theme` on the root, which beats the media query in both
  directions, and the two lines that apply a stored choice sit in the
  head so a chosen theme never flashes the other one. All of it is
  additive: with scripting off, the automatic behaviour is exactly
  what it was.

### Changed

- **The overview says what is here.** Each resource card read "8
  actions, 1 sub-collections", which describes the contract and
  answers a question nobody opening a console has. A card now lists
  the resource's first page: how many rows, whether more follow, and a
  few of them.

  Which column names a row is decided by asking which one tells the
  rows apart, since a generated console knows nothing about the
  domain. Preferring a name-like column alone put "file.ready" on four
  event cards; asking for the most distinct column alone put four raw
  timestamps there. So a preview carries both, a label and a time,
  because what an operator wants from a card is what the row is and
  when. The time drops its year and seconds, which a card has no room
  for and the listing page still carries.

  A resource whose listing refuses says so on its own card and leaves
  the rest of the page standing.

### Added

- **Closed sets are declared, enforced, and offered as menus.** An
  input whose values are a fixed list said so in prose and nowhere a
  machine could read: the console gave a text box, the OpenAPI
  document promised a string, and a caller learned the vocabulary by
  guessing or by reading a description. `ActionField.options` and
  `Resource.filter_options` declare the list.

  It reaches every face. The OpenAPI document and the MCP manifest
  carry it as `enum`, the console renders a menu in place of a box,
  and the dispatcher refuses a value outside the list ahead of the
  resolver, so the declaration cannot drift into a promise nothing
  keeps. The differ reads a narrowing set as breaking, including the
  introduction of a set where anything used to pass, and a widening
  one as compatible.

  `validate` refuses filter options that name a column outside
  `filterable`, an empty list, options on a field that cannot hold a
  string, and a value listed twice.

### Fixed

- **The narrow button answered 400 when no sort was chosen.** The sort
  control offers "declared order" as an empty value, and the query
  parser read that as a request to sort on a column named `""`, which
  the dispatcher refused. Every other control already ignored an empty
  value; sort and cursor now do too.

### Changed

- **The console reads at a glance.** A listing is scanned rather than
  read, and the raw projection defeated scanning. A nested object
  printed as a hundred and twenty characters of JSON wrapped down
  twenty lines in a narrow column and took the row with it, so five
  records made a page three thousand pixels tall. Digests ran to
  sixty-four characters, timestamps to nanoseconds, byte counts to
  whatever integer the row held.

  Values now render for what they turn out to be. Objects and arrays
  collapse to `{3 fields}` or `[2]` and open on a click. Digests cut
  to a stub, timestamps lose their sub-seconds, byte counts read in
  units, and every shortening keeps the exact value in `title`, so the
  table never becomes the reason a value cannot be read. A state gets
  a chip coloured by a small fixed vocabulary; a word outside it stays
  neutral, because colour is the one thing an operator trusts without
  reading. Access levels stay neutral for the same reason turned
  around: green reads as healthy, and `public` is the most exposed a
  record gets.

  The page around them changed to match. Wide tables scroll in their
  own box so the body never does, headers stick, headings and labels
  take a UI face while data keeps the monospace that makes columns of
  ids line up, and action forms sit in a grid rather than a stack.

- **`STYLE` and `cell` are exported.** A host renders pages of its own
  beside the generated ones, and two stylesheets means two consoles.
  Copal's deployment page was the case: it kept a copy and went on
  printing raw byte counts and nanosecond timestamps after the
  generated pages stopped.

### Fixed

- **The generated clients carry queries.** `Contract.queries` reached
  OpenAPI, the SDL, the MCP manifest, the console, and both runtime
  routers; the four client generators never read it. A service could
  declare a search, publish it on every other face, and hand out an
  SDK with no way to call it. Copal was in that position: `search` and
  `file_text` were absent from all four clients.

  Each query now emits a method taking the path parameter, if the path
  spells one, and its remaining inputs as named typed parameters, the
  way listings already read. The answer is the language's open JSON
  type, because the IR declares no shape for a query and inventing a
  struct would invent a promise. Required parameters are ordered ahead
  of optional ones, since TypeScript and Python both refuse the other
  order. A query with nothing to encode builds no query string, which
  also keeps the Rust binding free of an unused `mut`.

  The Rust client encodes through reqwest rather than joining pairs by
  hand, because a search term carries spaces and ampersands. Both
  query shapes are in the shared test fixtures now, so the goldens
  record them and the CLI test's real Python and Go toolchains parse
  what the generators emit.

### Added

- **`janus scaffold`, for services that already have a database.**
  Janus refused to do anything without a contract, and writing the
  first one meant transcribing every column and checking every filter
  and sort against an index by hand, which is the step that stopped
  adoption before it started. `scaffold --schema` reads the schema and
  writes a contract that validates against it, so the first `generate`
  produces artifacts instead of a list of claims to repair. Against
  copal's real 25 tables it derives 47 filters and 33 sorts, and all
  seven targets generate from the result unedited.

  It claims less than it could, deliberately. Sorts are claimed only
  where pinned columns cover the index prefix ahead of them, which is
  stricter than `validate` accepts. `filterable` describes what a
  caller may send, nothing obliges them to send it, and an unfiltered
  sort down a composite index scans. The differ calls a removed sort breaking and
  an added one compatible, so an invented claim costs a major version
  to withdraw while an omitted one costs a line. Columns whose names
  suggest a secret are left unexposed and reported, on the same
  reasoning applied to a worse outcome.

- **Content faces.** A resource may declare `content: { upload,
  download }`, and the byte paths render into the OpenAPI document
  as octet-stream operations (`PUT/GET /v1/{resource}/{id}/content`)
  under the differ's governance: removing a face is breaking. Bytes
  are streams rather than JSON, so these stay off the GraphQL and
  MCP shapes; the grant actions remain the agent path to the same
  content.

### Added

- **The MCP tool manifest.** `generate_mcp_tools` derives an MCP
  `tools/list` document from the contract: every resource's list and
  get, every action, and every query becomes a tool with a JSON
  Schema input derived from the same declarations, and scope and
  rate classes ride each tool as annotations. The manifest joins
  `generate_all` as the `mcp` target, so the agent surface is
  governed by the same drift gate and differ as every other face.
  `serde_json` now pins `preserve_order` unconditionally, so
  generated artifacts are byte-stable across feature sets.

### Changed

- **Field guards see the row.** A guard now receives
  `Option<&serde_json::Value>` beside the context: `Some(row)` when
  projecting, `None` when the question precedes rows (filter and
  sort narrowing). Ownership guards become expressible: show a
  caller their own rows' values while hiding the rest, within one
  listing, on every face. The rowless form still gates narrowing,
  because a caller who only partially sees a column must not filter
  by it. `guarded_fields` and `strip_guarded` join the shared
  projection API so hand-written faces project row-by-row exactly as
  the dispatcher does.

### Added

- **Contract queries: reads that answer a question.** A listing
  cannot express a search, because relevance is not a sort column and
  a query string is not a filter. `Contract.queries` declares named
  reads with typed parameters; they render as GraphQL query fields
  and REST `GET`s, dispatch through the same chain every other
  operation uses, and carry the same scope and rate declarations. The
  answer is JSON, since its shape belongs to the resolver rather than
  to a projected table. Resolvers are gated in both directions: a
  declared query without one refuses to build, and a resolver for a
  query nobody declared refuses too. The differ treats them like
  actions, so removing one, renaming its field, moving its path,
  tightening its scopes, or gaining a required parameter all read as
  breaking. Queries also render into OpenAPI as `GET` operations,
  with declared parameters split between path and query string, so
  the REST face of a query is published rather than merely served.

### Added

- **Per-principal watch ceilings.** `limits.max_watches_per_principal` caps
  concurrently open subscriptions per caller; the slot is taken before the
  resolver runs, so a refused open never starts a live query, and it rides
  the stream so dropping the subscription frees it. Over the ceiling is
  `too_many_requests`: closing a subscription is what frees a slot. Carried
  in `x-limits`; introducing or lowering the ceiling diffs as breaking.
- **The projection API for hand-written faces.** `hidden_fields`,
  `hidden_in`, and `strip_hidden` expose the dispatcher's own guard
  computation, so a service's hand-written REST handlers redact from the
  same declarations the GraphQL face enforces instead of drifting apart.
  The dispatcher delegates to the shared core, so the two cannot diverge.
- **Field guards as dispatcher projection.** A named visibility policy on an
  exposed field, registered by the service and applied once in the dispatcher
  on every row a resolver returns, subscriptions included. Denied fields are
  omitted; reads are never errors; a guarded field renders nullable on every
  generated surface and carries `x-guard` in OpenAPI. A caller who cannot see
  a column cannot filter or sort by it, because narrowing by a value is
  reading it. The completeness gate runs both directions, and the differ
  treats guarding an open field or swapping its policy as breaking.
- **Rate classes and the dispatch limiter.** A contract defines named budgets
  (`rate_classes`) and attaches them to resource reads or individual actions.
  The dispatcher charges the ledger before anything else runs: listings cost
  their clamped row limit, everything else costs one, buckets key on the
  principal's subject, and exhaustion is `too_many_requests`. A metered
  contract refuses to build without a registered `RateStore`; the in-memory
  store covers one process and a shared implementation covers a fleet. The
  differ treats new metering or a shrunken budget as breaking.
- **Principal and scopes.** A `Principal` (subject plus scopes) rides the
  context; `reads_require` on a resource gates list, get, sub-collections,
  and watching, and `requires` on an action gates invoking it. Enforcement
  happens once, in the dispatcher after the middleware chain and before the
  resolver, so every face inherits it identically. Anonymous against a
  declared scope is `unauthorized`; identified but missing it is `forbidden`,
  naming the scope. OpenAPI operations carry `x-requires-scopes`, and the
  differ treats a new requirement as breaking.
- **Consumption refusals in the error vocabulary.** `PayloadTooLarge` (413)
  and `TooManyRequests` (429), so a protocol face no longer downgrades an
  oversized body to a generic bad request and a metered refusal has a status a
  client can wait on.
- **Contract-declared limits.** Depth and complexity ceilings live in the
  contract, where the served GraphQL schema applies them before any resolver
  runs, the OpenAPI document carries them as `x-limits`, and the differ treats
  introducing or lowering one as a named breaking change.
- **Sub-resources.** A collection belonging to one parent instance (a file's
  versions, an endpoint's deliveries) is declared on the parent and reaches
  every face from that one declaration: `GET /v1/files/{id}/versions`, a field
  on the parent's GraphQL type, an OpenAPI path, and a method on all four
  clients. The index rulebook is shared with resources, with `parent_key`
  credited as equality-bound, and the GraphQL type name composes with the
  parent so two parents may each carry a `versions` collection.
- **Watchable resources and the subscription seam.** A resource may declare
  itself `watchable`, which adds a GraphQL Subscription field over a stream
  resolver the service registers. REST, OpenAPI, and the four generated clients
  are untouched: Janus generates no long-lived HTTP operations. Declaration and
  registration are checked in both directions at build, so a watchable resource
  without a resolver refuses by name, and so does a resolver for a resource the
  contract never opened. Watchers narrow the stream with the same `filterable`
  columns list callers use. The middleware chain runs once, at open.
- **The contract, executed.** The runtime: a resolver registry a service fills
  with its own data access, a middleware chain that is protocol-agnostic, and a
  dispatcher that enforces the contract before any resolver runs. Construction
  refuses, by name, any declared operation without a resolver.
- **The live GraphQL face.** An `async-graphql` dynamic schema built from the
  same IR the SDL generator prints, so the served schema and the checked-in
  artifact cannot disagree. `schema_builder` exposes the underlying builder for
  depth and complexity limits.
- **Generators for every surface**: OpenAPI 3.1, GraphQL SDL, and clients in
  Rust, TypeScript, Python, and Go, all from one contract object.
- **Actions**: verbs beyond list and get, with typed inputs and instance or
  collection targeting, rendered as OpenAPI operations, GraphQL mutations, and
  client methods from one declaration.
- **Validation against the real schema.** Tables and columns must exist,
  renames must not collide, filters must be indexed, and sorts must be
  reachable through an index prefix whose head is equality-bound. A sort no
  index can serve is a generation error naming the column.
- **Name gating.** Chosen names are checked against the GraphQL grammar, the
  `__` introspection prefix, root type names, cross-resource type collisions,
  and the SurrealDB v3 reserved-word list, exported as `janus::is_reserved`.
- **IR-level diffing.** `janus diff old.json new.json` classifies every change
  and exits non-zero on a breaking one, which makes the gate one line of CI.

### Fixed

- **OpenAPI list responses are the page envelope**, matching the SDL and the
  generated clients, rather than a bare array.

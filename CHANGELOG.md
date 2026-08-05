# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Janus has not cut a release yet. Everything below is the road to 0.1.0.

## [Unreleased]

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

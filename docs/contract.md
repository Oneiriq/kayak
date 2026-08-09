# Authoring a contract

A contract is data: a serializable object describing what an API exposes over
which tables. It never restates the database schema. Resources reference
tables and columns by name, and validation resolves those references against
the authoritative `surql-rs` definitions, so a contract cannot drift from the
schema without failing generation.

## The shape

```rust
use janus::{Action, ActionField, ActionOutput, Contract, FieldExposure,
            Resource, TypeRef};

Contract {
    name: "copal".into(),          // becomes the OpenAPI title
    version: "0.1.0".into(),       // the contract's own version
    ir_revision: 1,
    resources: vec![Resource {
        name: "files".into(),      // API name, plural
        table: "file".into(),      // backing table
        fields: vec![
            FieldExposure::column("path"),
            FieldExposure::renamed("size_bytes", "size"),
        ],
        pinned: vec!["tenant_id".into()],
        filterable: vec!["state".into()],
        sortable: vec!["created_at".into()],
        max_page_size: 100,
        watchable: false,
        graphql: None,
        actions: vec![/* see below */],
        sub_resources: vec![/* see below */],
    }],
}
```

Nothing is exposed by default. A column absent from `fields` does not exist
on any surface. Renames apply everywhere at once, from the wire name to every generated
client.

## Pinned, filterable, sortable

`pinned` names columns the server always equality-binds before any caller
input reaches a query: tenant scoping, soft-delete filters. They are never
API parameters. They exist so index validation can credit them.

Because the pins ride every read, they carry an index requirement of their
own, and it belongs to the set rather than to any single pin: some standard
or unique index must lead with a bound column, or the plain listing, nothing
filtered and nothing sorted, scans the whole table with the pins as its only
predicate. One leading bound column is enough; the engine seeks its range
and checks the remaining pins inside it. A sub-resource counts its
`parent_key` among the bound columns, which is how a table like a delivery
log, indexed by endpoint and never by tenant, is fine as a sub-collection
and refused as a top-level resource: the same rows, reached two ways, cost
two different things.

`filterable` columns become query parameters and GraphQL arguments. Each must
appear in at least one index on the table, because an unindexed filter works
in the demo and becomes a table scan in production.

`sortable` columns become sort options. The rule: some index must hold the
column at a position where every earlier column is pinned or filterable. An
index serves an ORDER BY only from a prefix whose head is equality-bound.
Given `(tenant_id, state, created_at)` with `tenant_id` pinned and `state`
filterable, `created_at` is a valid sort. Declaring a sort no index can serve
is a generation error naming the column.

Only a standard or unique index counts toward either rule. `DEFINE INDEX`
also spells FULLTEXT, HNSW, and MTREE, and none of the three narrows an
equality or supplies an order: a column covered only by one of them is, for a
filter or a sort, uncovered. Claiming it is a generation error that names the
index and its type, because "not covered by any index" against a table that
visibly has one sends the reader hunting the wrong bug. A column may of
course carry both, and a BM25 index beside a standard one is the ordinary way
to make a column searchable and filterable at once.

## Actions

Actions model verbs beyond list and get: uploads, deletions, signed URLs,
workflow starts. The contract describes the wire shape; the server binds the
behavior.

```rust
Action {
    name: "issue_url".into(),
    method: "POST".into(),
    path: "/{id}/url".into(),   // "" targets the collection
    input: vec![ActionField {
        name: "ttl_secs".into(),
        kind: TypeRef::Int,      // String, Int, Bool, Json
        required: false,
        description: Some("Seconds until the URL stops working.".into()),
    }],
    output: ActionOutput::Json,  // Resource, Json, None (HTTP 204)
    description: Some("Issue a signed URL.".into()),
    graphql_field: None,
}
```

A literal `{id}` in the path marks an instance action and becomes a required
id parameter on every surface: the OpenAPI path, the mutation argument, each
client method signature.

## Sub-resources

A collection that belongs to one instance of a parent is a sub-resource: a
file's versions, an endpoint's deliveries. It lists and pages like a resource
and has no id-addressable form of its own, because everything about it is
reached through the parent.

```rust
sub_resources: vec![SubResource {
    name: "versions".into(),
    table: "file_version".into(),
    parent_key: "file".into(),   // server-bound, credited like `pinned`
    fields: vec![FieldExposure::column("ordinal")],
    pinned: vec![],
    filterable: vec![],
    sortable: vec!["created_at".into()],
    max_page_size: 50,
    description: Some("Every stored version of this file.".into()),
    graphql: None,
}],
```

`GET /v1/files/{id}/versions` on REST, `file(id) { versions { items { ... } } }`
on GraphQL, and a `list_versions_files` method on each generated client. The
same index rules apply to the sub table, with `parent_key` credited as
equality-bound, so a sort of `created_at` needs an index holding it after
`file`.

The GraphQL type name composes with the parent (`FileVersion`), so two parents
may each carry a `versions` collection without colliding. Filters and page
ceilings belong to the sub-resource. Declaring `sortable` on `files` says
nothing about what `versions` may sort on.

## Field guards

A guard is a named visibility policy on one exposed field:

```rust
FieldExposure::column("digest").with_guard("audit_only"),
```

The service registers the decision and the dispatcher applies it as
projection on every row a resolver returns, on every face including
subscriptions, so a guarded value cannot leave through a forgotten
path. Rows OMIT denied fields; a read is never an error. A guarded
field renders nullable on every generated surface, since a field the
dispatcher may omit cannot promise to be present, and its OpenAPI
property carries `x-guard` naming the policy.

A caller who cannot see a column cannot narrow by it either:
filtering or sorting on a hidden column refuses, because narrowing by
a value is reading it.

A guard moving in ANY direction is breaking. Guarding an open field
takes values away from deployed callers and swapping guards changes
which callers those are; removing a guard refuses nobody, but it
takes away the redaction itself — the column becomes visible to every
caller the guard used to deny, on the API faces and in the derived
engine policy alike (see generators.md). Wider disclosure is not
additive for whoever the guard protected, so the differ names it and
review decides.

## Rate classes

A rate class is a named consumption budget, defined once and
referenced by the operations it bounds:

```rust
rate_classes: vec![RateClass { name: "reads".into(), units_per_minute: 6000 }],
resources: vec![Resource { rate_class: Some("reads".into()), /* ... */ }],
```

`rate_class` on a resource meters its reads (list, get,
sub-collections, watch opens); on an action it meters that action,
with no fallback to the resource's class. A listing costs its clamped
row limit and everything else costs one, so a caller asking for
hundred-row pages spends its budget a hundred times faster than one
probing single rows. Exhaustion refuses with `too_many_requests`,
which is retryable after waiting.

References to undefined classes refuse at validation. The differ
treats attaching a class to an unmetered operation or shrinking a
budget as breaking, and detaching or growing as compatible.

## Scopes

`reads_require` on a resource names the scopes a caller must hold to
list, get, read sub-collections of, or watch it. `requires` on an
action does the same for invoking it. Empty means open to any caller
the middleware admits, which is every existing contract's behavior.

```rust
reads_require: vec!["files_read".into()],
actions: vec![Action { requires: vec!["files_write".into()], /* ... */ }],
```

The dispatcher enforces them after the middleware chain and before the
resolver, so an auth layer that resolves the principal mid-chain still
counts and no guarded data is touched on a refusal. An anonymous
caller against a declared scope refuses `unauthorized`; an identified
caller missing one refuses `forbidden`, naming the scope. OpenAPI
operations carry their requirements as `x-requires-scopes`, and the
differ treats a new requirement as breaking and a removed one as
compatible. `reads_require` also feeds the engine policy face:
`derive_policy` renders it as a select conjunct on the resource's
table and its sub-resource tables, so a deployment enforcing at the
engine tightens both layers with one edit (see generators.md).

## Watching

`watchable: true` opens the resource to subscribers.
`limits.max_watches_per_principal` caps how many subscriptions one
caller may hold open at once; over the ceiling refuses with the
retryable code, and closing a subscription frees the slot. It adds a GraphQL
Subscription field and requires the service to register a watch resolver;
nothing about REST changes, because Janus generates no long-lived HTTP
operations.

Watchers narrow the stream with the same `filterable` columns list callers
use, so a resource has one filter vocabulary whichever operation reads it.
A column watchers should filter on therefore needs an index, like any other
filter. There is no limit or cursor: a stream is not a page.

## GraphQL name overrides

GraphQL names are part of a deployed schema's identity, since fragments name
types and queries name fields. When the derived defaults (type `File`, query
fields `files` and `file`, mutation `fileIssueUrl`, subscription
`fileChanged`) need to differ, override them per resource:

```rust
graphql: Some(GraphqlNames {
    type_name: Some("StoredFile".into()),
    list_field: Some("storedFiles".into()),
    get_field: Some("storedFile".into()),
    watch_field: Some("storedFileChanged".into()),
}),
```

Overrides touch the GraphQL surface only. REST paths and generated clients
keep the resource name. Every override is validated: GraphQL name grammar,
no `__` prefix, no collision with a root type, no collision across resources
on the effective type name, and no collision with a SurrealDB v3 reserved
name. The reserved-word list is exported as `janus::is_reserved` for schema
layers to reuse. Field renames pass through the same reserved gate.

## Validation

`janus::validate(&contract, &schema)` returns a list of violations; empty
means valid. Generation refuses invalid contracts with every violation named.
The checks: tables and columns exist, renames do not collide, filters are
indexed by an index that can narrow one, sorts are reachable through such an
index's prefix, the server-bound columns lead some index so the plain
listing seeks rather than scans, action definitions are well-formed, chosen
names are valid for every surface they reach.

Run the gate in the owning service's tests against the real schema
definitions. Schema drift then fails a test naming the offending column before anything
ships.

### Verifying against a live planner

Static validation proves an index exists; it cannot prove the planner
uses it. Behind the `verify` cargo feature (janus deliberately
carries no database client, so the client rides this gate the way
async-graphql rides `graphql`), `janus::verify::verify_contract`
composes one representative listing per filter claim and per sort
claim — pins as equality binds, the claimed filter bound, the claimed
sort ordered, always with a LIMIT — runs each through `EXPLAIN`
against a live database, and returns every claim whose plan falls
back to iterating the table, named the way validation names its
violations. `janus::verify::probes` exposes the composed queries
without running them, so what will be asked is inspectable before the
asker points at production. The same check runs from the CLI:

```
janus verify --contract contract.json --db ws://localhost:8000 \
    --namespace app --database app [--user root --pass secret]
```

Exit is non-zero when any claim scans, with each one printed, so it
gates in CI beside `diff`.

## Diffing

`janus::diff(&old, &new)` compares two contracts at the IR level and
classifies every change. Breaking: a removed resource, field, filter, or
sort; a field re-pointed to a different column under the same wire name; a
lowered page ceiling; a moved action; a changed output; an input that became
required or changed type; any effective GraphQL rename; a resource that
stopped being watchable; a removed sub-resource, or one that lost a field,
filter, sort, or page headroom. Compatible: additions, a resource that became
watchable, a new sub-resource, and removal of an optional input.

The CLI exits non-zero on breaking changes (`janus diff old.json new.json`),
which makes the gate one line of CI.

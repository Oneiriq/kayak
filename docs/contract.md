# Authoring a contract

A contract is data: a serializable object describing what an API exposes over
which tables. It never restates the database schema. Resources reference
tables and columns by name, and validation resolves those references against
the authoritative `surql-rs` definitions, so a contract cannot drift from the
schema without failing generation.

## The shape

```rust
use kayak::{Action, ActionField, ActionOutput, Contract, FieldExposure,
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

## Pinning to one of several columns

`pinned` is an AND: every column is equality-bound and credited as an
index prefix. A symmetric relationship cannot be said that way. A
friendship stored as one row per unordered pair holds its two accounts
in `a` and `b`, and the caller is *either* of them:

```rust
pinned: vec![],
pinned_either: vec!["a".into(), "b".into()],
filterable: vec!["state".into()],
```

The engine answers that as a union of one index seek per branch, so
**every** alternative must head an index whose remaining columns serve
the filter and sort claims — here `(a, state)` and `(b, state)`. A
branch with no index behind it drags the whole read back to a scan, and
is refused, naming the branch so the author knows which index is
missing.

Fewer than two alternatives is refused, as is a column that is both
pinned and an alternative. Changing the set is breaking in both
directions: narrowing hides rows a caller used to see, widening shows
rows they did not.

## Which faces a resource exposes

A resource exposes a listing and a getter by default. `faces` narrows
that:

```rust
faces: ResourceFaces::ALL,       // GET /accounts and GET /accounts/{id}
faces: ResourceFaces::GET_ONLY,  // reachable by id, never enumerable
faces: ResourceFaces::LIST_ONLY, // enumerable, no by-id face
faces: ResourceFaces::NONE,      // a place for actions to live
```

`GET_ONLY` is the shape a social service needs: `GET /accounts/{id}` is
served and `GET /accounts` must never be, because enumerating every user
is the thing it is careful not to do. `NONE` is a domain that is all
verbs — an RPC-shaped resource whose table has no browsable collection.

Actions and sub-resources are independent of both flags, which is what
makes `NONE` useful rather than empty.

Turning the listing off refuses `filterable`, `sortable`,
`filter_options` and `watchable`, because each is a claim about an
endpoint that no longer exists. A resource with no face, no action and
no sub-resource is refused outright: it generates nothing.

Withdrawing a face is breaking — an endpoint, a GraphQL field and a
client method all disappear. Adding one back is compatible.

## Where the routes live

Resource faces hang under `api_prefix`, `/v1` by default:

```rust
api_prefix: "/v1".into(),   // /v1/files, /v1/files/{id}, /v1/files/{id}/url
api_prefix: String::new(),  // /files, /files/{id} -- the service's own routes
api_prefix: "/api/v2".into(),
```

A contract that omits it gets `/v1`, and the field stays out of the
serialized document, so contracts written before it existed round-trip
unchanged.

Queries are unaffected: they declare absolute paths (`"/me"`,
`"/v1/search"`) and always have, which is why a query could describe a
service's real route before a resource could.

Moving the prefix is breaking in both directions — every resource route
moves at once, and every deployed client calls a path that is no longer
served. That is the point of declaring it rather than hardcoding it: a
service already serving `/accounts` adopts a generated client by saying
so, instead of moving its routes and breaking whatever is shipped.

## Authentication

A contract declares how its callers authenticate, and every face reads
the declaration: the four clients, the OpenAPI security scheme, and the
differ. The field defaults to `None` and is omitted from serialized
contracts when it is.

```rust
auth: AuthScheme::None,        // an open API: no credential, no field
auth: AuthScheme::Bearer,      // Authorization: Bearer <token>
auth: AuthScheme::Header {     // a named header, sent verbatim
    name: "x-copal-tenant".into(),
    credential: "tenant".into(),
},
```

`credential` names the thing, and the name reaches the generated
constructors: a bearer contract gets `Client::new(url, token)`, the
header above gets `Client::new(url, tenant)`, and an open one gets
`Client::new(url)` with no field to carry. Bearer always calls it
`token`; a header scheme deserialized without a `credential` gets
`credential`.

Changing the scheme is breaking in every direction, including relaxing
it — a client generated against a contract that sends a credential does
not stop compiling when the server stops requiring one, but every caller
holding the old client is now sending a header the contract no longer
describes, and the differ says so rather than letting that pass as a
compatible loosening.

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

## Queries

A query is a read that answers a question rather than paging a collection.
Search is the shape that motivated them: relevance is not a sort column and a
query string is not a filter, so a listing cannot express one.

```rust
queries: vec![Query {
    name: "search".into(),
    path: "/v1/search".into(),          // absolute under the API root
    input: vec![ActionField {
        name: "q".into(),
        kind: TypeRef::String,
        required: true,
        ..
    }],
    requires: vec!["read".into()],
    rate_class: Some("reads".into()),
    searches: vec![SearchKind::Lexical, SearchKind::Vector],
    backing: vec![/* see below */],
    ..
}],
```

Queries render as REST `GET`s, GraphQL query fields, client methods, and MCP
tools, and carry the same scope and rate declarations every other operation
carries. The answer is JSON, because its shape belongs to the resolver rather
than to a projected table. The differ treats them like actions: removing one,
renaming its field, moving its path, tightening its scopes, or gaining a
required input all read as breaking.

### Search backings

A listing declares its cost exhaustively — every filter and sort claim is
index-validated — while a query, the one read whose cost is most surprising,
would otherwise be an opaque box: typed inputs, a path, and nothing about the
machinery behind it. Nothing would stop a schema change from dropping the
FULLTEXT index while the contract went on promising search. A backing names
that machinery:

```rust
backing: vec![
    SearchBacking {
        table: "text_chunk".into(),
        column: "body".into(),
        index: "idx_chunk_body".into(),
        kind: SearchKind::Lexical,       // `@@` through FULLTEXT
        dimension: None,                 // a FULLTEXT index has no width
        optional: false,
    },
    SearchBacking {
        table: "text_chunk".into(),
        column: "embedding".into(),
        index: "idx_chunk_embedding".into(),
        kind: SearchKind::Vector,        // KNN through HNSW, MTREE, or DISKANN
        dimension: Some(768),            // held against the index's DIMENSION
        optional: true,                  // applied at startup where configured
    },
],
```

A fused search — BM25 candidates and vector neighbours rescored together —
is two backings on one query; the fusion itself is resolver behavior, not
contract. `backing` is optional and empty by default: a query without one
claims no search machinery, which is what every existing contract declares,
and old contracts deserialize unchanged.

Validation holds a backing to the mirror image of the listing index rules.
The named table, column, and index must exist; the index must hold the
column; and it must be the kind's own machinery — FULLTEXT for a lexical
backing, HNSW, MTREE, or DISKANN for a vector one. A backing resting on a plain b-tree
is refused the same way a filter resting on a FULLTEXT index is, with the
violation naming the index and what it turned out to be, because "the plain
index idx_chunk_tenant cannot answer it" is a diagnosis where a bare refusal
is a hunt.

Which of the three vector machineries answers is the schema's business. The
contract asks for nearest neighbours through an index; moving a column from
HNSW to DISKANN is a capacity decision, and the contract does not have to be
rewritten for it.

### The width

`dimension` pins what the search sends, and validation holds it against the
index's own `DIMENSION`. A vector of the wrong width is not a slower search,
it is a different one, and the width changes whenever the embedding model
does — so a model swap that outran its schema becomes a generation failure
instead of a quiet change in what comes back. Leave it `None` where the
deployment chooses the width; state it wherever the schema does. A lexical
backing has no width, and stating one there is refused rather than compared
against a FULLTEXT index that was never going to have one.

### Machinery a deployment configures

`optional: true` says the deployment is free not to provide this index. Copal
is the case that needed it: its HNSW index over `text_chunk.embedding` is
applied at startup, and only where an embedding model is configured, at that
model's width. Declared outright the claim would be false in every deployment
without one — so it was declared nowhere, and a search the contract never
mentions is exactly the silence these rules exist to end.

Optional relaxes one rule and no others: the index may be absent. An index
that IS there holds the column, is the kind's own machinery, and matches the
declared width like any other. May be absent, never may be wrong. `verify
--db` reads it the same way: the probe runs, and a plan that missed the index
is excused on exactly one fact — that this database does not define it.

### The declared search

A backing says what answers a search. `searches` says the search happens:

```rust
searches: vec![SearchKind::Lexical, SearchKind::Vector],
```

Every kind named there must have a backing of that kind behind it, or
generation fails naming the query and the kind. This is the one rule in the
toolchain that catches an ABSENCE rather than a mistake, and absence is the
shape unindexed search actually has: nobody writes down that the neighbour
query has no index, they write the resolver and move on. Before this field
there was no way to make the promise, so there was no way to break it — a
query that declared no backing and a query that needed none were the same
document.

What it cannot do is make anyone declare. That is the standing limit of a
declaration language, the same one that lets a listing simply not claim a
filterable column, and it is why `verify --db` exists beside the static gate.
What the declaration buys is that once made, it is load-bearing: dropping the
index becomes a build failure, the differ calls losing the capability
breaking, and the artifacts say which machinery a caller is relying on.

`searches` is empty by default and old contracts deserialize unchanged. A
backing whose kind is not declared is permitted — the machinery is validated
either way — so adopting the field is incremental rather than a flag day.

### What it moves

The backing is capacity metadata, not wire shape. REST paths, the SDL, and
client signatures do not change when one is declared; the declaration
surfaces where metadata already surfaces — the MCP tool's annotations, beside
scope and rate, and the OpenAPI operation description, where an optional
backing reads "where configured" because that is the one part of this a
caller should expect to feel. `searches` renders nowhere of its own: what a
query performs is implied by the backings that answer for it, and those
already render.

The differ reads a removed backing, or any of the four members that say where
the machinery is re-pointed (table, column, index, kind), as breaking, and a
backing added to an existing query as compatible: it promises more about the
same wire surface. The width and the optional flag move under a fixed
identity, so they read as one change each rather than a loss and a gain — a
width that changes or disappears and a backing that becomes optional are
breaking; a width that appears and a backing that becomes required are not.
Losing a declared search is breaking; gaining one is compatible.

`verify --db` probes each backing through its own operator — `@@` for
lexical, the `<|k,EF|>` KNN form for vector — and holds the plan to the
NAMED index, which is stricter than not scanning: a search served by some
other index than the declared one is drift too. One boundary is the
engine's: SurrealDB 3.x has removed MTREE, so while validation accepts an
MTREE-typed definition for a vector backing, a live 3.x database cannot hold
one and verification composes only the HNSW form.

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
nothing about REST changes, because Kayak generates no long-lived HTTP
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
name. The reserved-word list is exported as `kayak::is_reserved` for schema
layers to reuse. Field renames pass through the same reserved gate.

## Validation

`kayak::validate(&contract, &schema)` returns a list of violations; empty
means valid. Generation refuses invalid contracts with every violation named.
The checks: tables and columns exist, renames do not collide, filters are
indexed by an index that can narrow one, sorts are reachable through such an
index's prefix, the server-bound columns lead some index so the plain
listing seeks rather than scans, search backings rest on the kind of index
that can answer them at the width they claim, every declared search has a
backing behind it, action definitions are well-formed, chosen names are
valid for every surface they reach.

Run the gate in the owning service's tests against the real schema
definitions. Schema drift then fails a test naming the offending column before anything
ships.

### Verifying against a live planner

Static validation proves an index exists; it cannot prove the planner
uses it. Behind the `verify` cargo feature (kayak deliberately
carries no database client, so the client rides this gate the way
async-graphql rides `graphql`), `kayak::verify::verify_contract`
composes one representative listing per filter claim and per sort
claim — pins as equality binds, the claimed filter bound, the claimed
sort ordered, always with a LIMIT — and one probe per search backing
through its own operator, runs each through `EXPLAIN` against a live
database, and returns every claim the planner does not serve, named
the way validation names its violations: a listing claim fails when
its plan iterates the table, a backing when its plan does not reach
the named index — unless the backing is optional and this database
does not define that index, which is the one excuse on offer and it
costs one extra round trip, spent only on an optional backing that
already came back unserved. `kayak::verify::probes` exposes the composed queries
without running them, so what will be asked is inspectable before the
asker points at production. The same check runs from the CLI:

```
kayak verify --contract contract.json --db ws://localhost:8000 \
    --namespace app --database app [--user root --pass secret]
```

Exit is non-zero when any claim scans, with each one printed, so it
gates in CI beside `diff`.

## Diffing

`kayak::diff(&old, &new)` compares two contracts at the IR level and
classifies every change. Breaking: a removed resource, field, filter, or
sort; a field re-pointed to a different column under the same wire name; a
lowered page ceiling; a moved action; a changed output; an input that became
required or changed type; any effective GraphQL rename; a resource that
stopped being watchable; a removed sub-resource, or one that lost a field,
filter, sort, or page headroom; a removed search backing, or any of the four
members that place one re-pointed; a backing that stopped pinning its width
or pinned a different one; a backing that became optional; a search the query
no longer performs. Compatible: additions, a resource that became watchable,
a new sub-resource, a backing added to an existing query, a width newly
pinned, a backing now required of every deployment, a search newly performed,
and removal of an optional input.

The CLI exits non-zero on breaking changes (`kayak diff old.json new.json`),
which makes the gate one line of CI.

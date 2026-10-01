# Authoring a contract

A contract is a JSON document that says what an API exposes over which
SurrealDB tables. It does not restate the database schema. Resources name
tables and columns, and validation resolves those names against the
`surql` `TableDefinition`s that back the service (crate `oneiriq-surql`,
imported as `surql`). A contract that names a missing column, or claims a
filter that no index serves, fails validation, and generation refuses it.

This page covers every key a contract can hold, the rules validation
applies, and how the differ classifies a change. Turning a contract into
artifacts is covered in [generators.md](generators.md). Serving one is
covered in [runtime.md](runtime.md).

## A first contract

```json
{
  "name": "filestore",
  "version": "0.1.0",
  "auth": { "kind": "bearer" },
  "resources": [
    {
      "name": "files",
      "table": "file",
      "fields": [
        { "column": "path" },
        { "column": "state" },
        { "column": "size_bytes", "rename": "size" },
        { "column": "created_at" }
      ],
      "pinned": ["tenant_id"],
      "filterable": ["state"],
      "sortable": ["created_at"],
      "max_page_size": 100
    }
  ]
}
```

The `file` table behind it has the columns `tenant_id`, `path`, `state`,
`size_bytes`, and `created_at`, and a standard index on
`(tenant_id, state, created_at)`. Against that schema the contract
validates:

- Every column in `fields`, `pinned`, `filterable`, and `sortable` exists.
- The pinned column `tenant_id` leads an index, so the plain listing seeks.
- `state` appears in an index, so filtering on it is allowed.
- `created_at` sits in an index behind `tenant_id` (pinned) and `state`
  (filterable), so sorting on it is allowed.

From this one resource the generators produce `GET /v1/files` and
`GET /v1/files/{id}` in OpenAPI, a `File` type with `files` and `file`
query fields in GraphQL, `files_list` and `file_get` MCP tools, and
`list_files` and `get_file` methods in each client, cased per language.

To start from an existing schema, run `kayak scaffold` (see
[generators.md](generators.md#kayak-scaffold)). It writes a contract that
already validates, which you then narrow.

## JSON and Rust

Kayak reads contracts with serde, so JSON can leave out any key that has a
default. The tables below list every key and its default.

Kayak does not reject unknown keys. A misspelled key such as
`"filterabel"` is ignored without an error, and the contract behaves as if
the key were absent. After adding a key, check that its effect shows up in
the generated OpenAPI document.

In Rust, the simplest path is to parse the JSON:

```rust
let text = std::fs::read_to_string("api/contract.json")?;
let contract: kayak::Contract = serde_json::from_str(&text)?;
```

`Contract`, `Resource`, `SubResource`, `Action`, `ActionField`, and `Query`
do not implement `Default`, so a Rust struct literal has to name every
field. This is the `files` resource above, written out in full:

```rust
use kayak::{FieldExposure, Identity, Resource, ResourceFaces};

let files = Resource {
    name: "files".into(),
    table: "file".into(),
    identity: Identity::Id,
    fields: vec![
        FieldExposure::column("path"),
        FieldExposure::column("state"),
        FieldExposure::renamed("size_bytes", "size"),
        FieldExposure::column("created_at"),
    ],
    pinned: vec!["tenant_id".into()],
    pinned_either: vec![],
    filterable: vec!["state".into()],
    filter_options: Default::default(),
    sortable: vec!["created_at".into()],
    max_page_size: 100,
    actions: vec![],
    content: None,
    sub_resources: vec![],
    faces: ResourceFaces::ALL,
    rate_class: None,
    reads_require: vec![],
    watchable: false,
    graphql: None,
};
```

Helpers exist for the small types: `FieldExposure::column`,
`FieldExposure::renamed`, `FieldExposure::with_guard`, the
`ResourceFaces::ALL`, `GET_ONLY`, `LIST_ONLY`, and `NONE` constants, and
`Contract::default_api_prefix()`. `ResourceFaces`, `GraphqlNames`,
`SubGraphqlNames`, `ContractLimits`, `ContentFaces`, `AuthScheme`, and
`Identity` implement `Default`. The rest of this page uses JSON.

## Key reference

### Contract

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | Contract name. Becomes the OpenAPI title, appears in client file headers, and names the Go package. |
| `version` | required | The contract's own version. Becomes the OpenAPI `info.version`. |
| `ir_revision` | `1` | IR format revision. Validation refuses `0` and any revision newer than this build of Kayak reads, and every CLI command that reads a contract refuses a newer one, `diff` and `verify` included. |
| `api_prefix` | `"/v1"` | Path prefix for resource routes, in the generated artifacts and the runtime's REST router. See [Where the routes live](#where-the-routes-live). |
| `auth` | `{"kind": "none"}` | How callers authenticate. See [Authentication](#authentication). |
| `rate_classes` | `[]` | Named consumption budgets. See [Rate classes](#rate-classes). |
| `limits` | none | GraphQL cost ceilings and a subscription cap. See [Limits](#limits). |
| `resources` | required | The exposed resources. Write `[]` for a contract that only declares queries. |
| `queries` | `[]` | Reads that are not listings. See [Queries](#queries). |

### Resource

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | API name, plural, lowercase snake or kebab case. Becomes the path segment. |
| `table` | required | The backing table. |
| `fields` | required | The columns to expose. See [Exposing fields](#exposing-fields). |
| `identity` | `"id"` | The column that names one row on the wire, or `null` for none. See [Identity](#identity). |
| `pinned` | `[]` | Columns the server always equality-binds. |
| `pinned_either` | `[]` | Columns the server binds the caller's value to one of. |
| `filterable` | `[]` | Columns callers may filter on. |
| `filter_options` | `{}` | Allowed values per filterable column. |
| `sortable` | `[]` | Columns callers may sort on. |
| `max_page_size` | `100` | Page-size ceiling for the listing. |
| `faces` | `{"list": true, "get": true}` | Which collection faces exist. |
| `actions` | `[]` | Verbs beyond list and get. |
| `content` | none | Byte upload and download faces. |
| `sub_resources` | `[]` | Collections reached through one instance. |
| `rate_class` | none | The budget that meters reads of this resource. |
| `reads_require` | `[]` | Scopes a caller must hold to read. |
| `watchable` | `false` | Whether callers may subscribe to changes. |
| `graphql` | none | GraphQL name overrides. |

### Field exposure

| Key | Default | Meaning |
| --- | --- | --- |
| `column` | required | The column in the table. |
| `rename` | none | The API name. Defaults to the column name. |
| `guard` | none | A named visibility policy. See [Field guards](#field-guards). |

### Sub-resource

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | API name, plural, lowercase snake or kebab case. |
| `table` | required | The backing table. |
| `parent_key` | required | The column on `table` that holds the parent's id. |
| `fields` | required | The columns to expose. |
| `identity` | `"id"` | As on a resource. |
| `pinned` | `[]` | Server-bound columns beyond `parent_key`. |
| `filterable` | `[]` | As on a resource. |
| `sortable` | `[]` | As on a resource. |
| `max_page_size` | `100` | Page-size ceiling. |
| `description` | none | Rendered into OpenAPI. |
| `graphql` | none | `{"type_name": ..., "field": ...}` overrides. |

### Action

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | Lowercase snake case. |
| `method` | required | `POST`, `PUT`, `DELETE`, or `PATCH`. |
| `path` | `""` | Suffix under the resource path. `""` targets the collection. A literal `{id}` targets one instance. |
| `input` | `[]` | Request-body fields. |
| `output` | `"json"` | `"resource"`, `"json"`, or `"none"`. |
| `description` | none | Rendered into OpenAPI, MCP, and the console. |
| `graphql_field` | none | Mutation field name override. |
| `requires` | `[]` | Scopes a caller must hold to invoke it. |
| `rate_class` | none | The budget that meters it. |

### Input field (actions and queries)

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | Lowercase snake case. |
| `kind` | required | `"string"`, `"int"`, `"bool"`, or `"json"`. |
| `required` | `false` | Whether the caller must send it. |
| `description` | none | Rendered into OpenAPI and MCP. |
| `options` | `[]` | A closed set of allowed values. String inputs only. |
| `multiple` | `false` | Whether the caller may send several of `options`, as a JSON array or one comma-separated string. |

### Query

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | Lowercase snake case. |
| `path` | required | Absolute REST path, such as `"/v1/search"`. Never prefixed. |
| `input` | `[]` | Parameters. |
| `description` | none | Rendered into OpenAPI and MCP. |
| `graphql_field` | none | Query field name override. |
| `requires` | `[]` | Scopes a caller must hold. |
| `rate_class` | none | The budget that meters it. |
| `searches` | `[]` | The search kinds it performs: `"lexical"`, `"vector"`. |
| `backing` | `[]` | The indexes that answer those searches. |

## Exposing fields

Nothing is exposed by default. A column absent from `fields` does not
appear on any surface. A `rename` changes the API name everywhere a row is
described: the OpenAPI schema, the GraphQL type, the client types, and the
keys a resolver's rows are expected to carry. A rename must be lowercase
snake case and must not be a SurrealDB v3 reserved word.

Filters and sorts are named by column, even when the column is exposed
under another name. If `size_bytes` in the first example were filterable,
it would be declared as `size_bytes` and sent as `size_bytes` on REST and
MCP (`sizeBytes` as a GraphQL argument), while rows carry `size`.

## Identity

Every resource names its rows on the wire somehow. `identity` says how,
and has three states:

| JSON | Meaning |
| --- | --- |
| key absent, or `"identity": "id"` | Rows carry `id`. This is the default. |
| `"identity": "user"` | Rows are named by the `user` column. |
| `"identity": null` | Rows carry no identity field at all. |

The generated artifacts read it. For a `presence` resource with
`"identity": "user"`, the OpenAPI get path becomes `/v1/presence/{user}`,
the SDL get field takes `user: ID!`, the MCP get tool takes `user`, and
every client type carries a `user` field. When the identity column is not
in `fields`, the artifacts add it as a required string. When it is in
`fields`, they describe it once, with the column's real type.

A named identity column must exist on the table. `null` means no instance
can be addressed, so validation refuses it together with a get face,
content faces, any action whose path holds `{id}`, and any sub-resource.
Sub-resources take `identity` too, with the same three states.

The live GraphQL schema reads it too: its type carries the identity field
the SDL prints, its get field takes the identity column as its argument,
and a sub-collection field reads the parent row's identity column. The
REST router addresses an instance by its position in the path, so it
serves any identity. The console's listing page still links rows through
their `id` key (see [runtime.md](runtime.md#known-limitations)).

## Pinned, filterable, sortable

`pinned` names columns the server always equality-binds before any caller
input reaches a query: tenant scoping, soft-delete filters. They are never
API parameters. They exist so index validation can credit them.

Because the pins apply to every read, they carry an index requirement of
their own, and the requirement applies to the pins as a set. Some
standard or unique index must lead with a bound column. If none
does, the plain listing (nothing filtered, nothing sorted) scans the whole
table with the pins as its only predicate. One leading bound column is
enough: the engine seeks its range and checks the remaining pins inside
it. A sub-resource counts its `parent_key` among the bound columns. That
is why a table indexed by endpoint and never by tenant, such as a delivery
log, passes as a sub-collection of an endpoint and fails as a top-level
resource.

`filterable` columns become query parameters and GraphQL arguments. Each
must appear in at least one index on the table. An unindexed filter
returns correct results on small data and scans the table once the table
grows.

`sortable` columns become sort options. Some index must hold the column at
a position where every earlier column is pinned or filterable, because an
index serves an ORDER BY only from a prefix whose head is equality-bound.
Given `(tenant_id, state, created_at)` with `tenant_id` pinned and `state`
filterable, `created_at` is a valid sort. Declaring a sort no index can
serve is a generation error naming the column.

Only a standard or unique index counts toward either rule. `DEFINE INDEX`
also defines FULLTEXT, HNSW, DISKANN, and COUNT indexes, and none of them
narrows an equality or supplies an order. A column covered only by one of
them is uncovered for a filter or a sort. Claiming it is a generation error that
names the index and its type, so the message points at the index you were
looking at. A column may carry both kinds: a FULLTEXT index beside a
standard one makes a column searchable and filterable.

### Filter options

`filter_options` lists the values a filterable column accepts, for columns
whose values form a closed set:

```json
"filterable": ["state"],
"filter_options": { "state": ["pending", "ready", "deleted"] }
```

Validation requires every key to be a filterable column and every list to
be non-empty. The OpenAPI list parameter and the MCP list tool publish the
set as an `enum`, and the console renders it as a menu. The dispatcher
enforces it on every face: a listing or a subscription that filters on a
value outside the list is refused with `bad_request` before the resolver
runs.

## Pinning to one of several columns

`pinned` is an AND: every column is equality-bound and credited as an
index prefix. A symmetric relationship cannot be written that way. A
friendship stored as one row per unordered pair holds its two accounts in
`a` and `b`, and the caller is either of them:

```json
"pinned": [],
"pinned_either": ["a", "b"],
"filterable": ["state"]
```

The engine answers that as a union of one index seek per branch, so every
alternative must head an index whose remaining columns serve the filter and
sort claims. Here that means `(a, state)` and `(b, state)`. A branch with
no index behind it turns the whole read into a scan, so validation refuses
it and names the branch.

Validation also refuses fewer than two alternatives, a column named twice,
and a column that is both pinned and an alternative. Changing the set is
breaking in both directions: narrowing it hides rows a caller used to see,
and widening it shows rows they did not.

## Which faces a resource exposes

A resource exposes a listing and a getter by default. `faces` narrows that:

| JSON | Rust | Generated faces |
| --- | --- | --- |
| absent | `ResourceFaces::ALL` | `GET /v1/accounts` and `GET /v1/accounts/{id}` |
| `{"list": false}` | `ResourceFaces::GET_ONLY` | `GET /v1/accounts/{id}` only |
| `{"get": false}` | `ResourceFaces::LIST_ONLY` | `GET /v1/accounts` only |
| `{"list": false, "get": false}` | `ResourceFaces::NONE` | Neither. The resource holds actions and sub-resources. |

`GET_ONLY` fits a resource whose instances are reachable by id and whose
collection must never be enumerated, such as a social service's accounts.
`NONE` fits an RPC-shaped resource that is all verbs. Actions and
sub-resources do not depend on either flag.

A withdrawn face disappears from the OpenAPI document, the SDL, the MCP
manifest, and the clients. Turning the listing off makes validation refuse
`filterable`, `sortable`, `filter_options`, and `watchable`, since each is
a claim about the listing. A resource with no face, no action, and no
sub-resource is refused because it generates nothing.

Withdrawing a face is breaking. Adding one back is compatible.

The runtime serves only the declared faces. The dispatcher needs no
resolver for a withdrawn face and refuses a call to it, the REST router
has no route for it, the live GraphQL schema has no field for it, and the
console neither lists nor links to it (see
[runtime.md](runtime.md#resolvers)).

## Where the routes live

Resource routes hang under `api_prefix`, which defaults to `/v1`:

| `api_prefix` | Generated routes |
| --- | --- |
| `"/v1"` (default) | `/v1/files`, `/v1/files/{id}`, `/v1/files/{id}/url` |
| `""` | `/files`, `/files/{id}`, `/files/{id}/url` |
| `"/api/v2"` | `/api/v2/files`, `/api/v2/files/{id}`, `/api/v2/files/{id}/url` |

The OpenAPI paths, all client methods, the runtime's REST router, and the
console reference use the prefix. A contract that
omits the key gets `/v1`, and a contract that uses `/v1` serializes without
the key. An empty prefix or `"/"` puts resources at the root. Otherwise the
prefix must start with `/` and contain no empty segment and no whitespace.
A trailing slash is trimmed.

Queries are unaffected. They declare absolute paths (`"/me"`,
`"/v1/search"`), and the prefix is never added to them.

The prefix exists so a service that already serves `/accounts` can adopt
generated clients without moving its routes. Changing it is breaking in
both directions, because every generated resource route moves at once.

## Authentication

A contract declares how its callers authenticate:

```json
"auth": { "kind": "header", "name": "x-tenant", "credential": "tenant" }
```

| `auth` | On the wire | Rust client constructor |
| --- | --- | --- |
| absent, or `{"kind": "none"}` | nothing | `Client::new(url)` |
| `{"kind": "bearer"}` | `Authorization: Bearer <token>` | `Client::new(url, token)` |
| `{"kind": "header", "name": "x-tenant", "credential": "tenant"}` | `x-tenant: <value>`, sent verbatim | `Client::new(url, tenant)`, named after `credential` |

A header scheme without `credential` names its constructor argument
`credential`. The OpenAPI document declares the scheme under
`components.securitySchemes` and a top-level `security` requirement. The
four clients put the credential on every request, and the differ tracks
the scheme.

Changing the scheme is breaking in every direction, including removing it.
A client generated against a contract that sends a credential keeps
compiling when the server stops requiring one, but every caller holding it
then sends a header the contract no longer describes, so the differ reports
the change for review.

The runtime does not read `auth`. Authenticating a request is the host's
job, usually in a middleware that seeds a `Principal` (see
[runtime.md](runtime.md#middleware)).

## Actions

Actions model verbs beyond list and get: uploads, deletions, signed URLs,
workflow starts. The contract describes the wire shape, and the service
binds the behavior.

```json
"actions": [
  {
    "name": "issue_url",
    "method": "POST",
    "path": "/{id}/url",
    "input": [
      {
        "name": "ttl_secs",
        "kind": "int",
        "description": "Seconds until the URL stops working."
      }
    ],
    "output": "json",
    "description": "Issue a signed URL."
  },
  { "name": "remove", "method": "DELETE", "path": "/{id}", "output": "none" }
]
```

The route is the resource path plus `path`: `POST /v1/files/{id}/url`. An
empty `path` targets the collection. A literal `{id}` marks an instance
action and becomes a required id parameter on every surface: the OpenAPI
path parameter, the `id: ID!` mutation argument, and the first parameter
of each client method. The generated GraphQL field is camelCase singular
resource plus PascalCase action (`fileIssueUrl`) unless `graphql_field`
overrides it.

`output` says what comes back:

| `output` | OpenAPI | GraphQL | Resolver returns |
| --- | --- | --- | --- |
| `"resource"` | 200 with the resource schema | the resource type | `Some(row)` |
| `"json"` (default) | 200 with a free-form object | `JSON!` | `Some(value)` |
| `"none"` | 204 | `Boolean!` | `None` |

Inputs travel as a JSON request body. `options` closes a string input to a
set of values, which renders as an `enum` in OpenAPI and MCP and as a menu
in the console, and which the dispatcher enforces. `multiple` lets a
caller send several of the options, and it requires `options`. OpenAPI
and MCP publish such an input as an array. The runtime takes a JSON
array, the key repeated in a REST query string, or one comma-separated
string (`"a,b"`), and the resolver reads the comma-separated string
whichever form arrived.

Validation refuses an empty or duplicate action name, a method other than
POST, PUT, DELETE, or PATCH, a non-empty path that does not start with
`/`, duplicate input names, an option listed twice, `options` on a
non-string input, and `multiple` without `options`.

## Content faces

A resource with stored bytes declares them:

```json
"content": { "upload": true, "download": true }
```

These render into the OpenAPI document only, as
`PUT /v1/files/{id}/content` (an `application/octet-stream` body) and
`GET /v1/files/{id}/content` (an octet-stream response). They produce no
GraphQL field, MCP tool, client method, or runtime route. The host serves
the bytes itself. Content faces need an identity, so `"identity": null`
refuses them. Removing a face is breaking; adding one is compatible.

## Sub-resources

A collection that belongs to one instance of a parent is a sub-resource: a
file's versions, an endpoint's deliveries. It lists and pages like a
resource and has no by-id form of its own.

```json
"sub_resources": [
  {
    "name": "versions",
    "table": "file_version",
    "parent_key": "file",
    "fields": [{ "column": "ordinal" }, { "column": "created_at" }],
    "sortable": ["created_at"],
    "max_page_size": 50,
    "description": "Every stored version of this file."
  }
]
```

That produces `GET /v1/files/{id}/versions` in OpenAPI, a
`versions(limit: Int = 50, cursor: String, sort: FileVersionSort)` field on
the `File` GraphQL type, a `file_versions_list` MCP tool, and a
`list_versions_files` method on each client.
The same index rules apply to the sub table, with `parent_key` credited as
equality-bound, so a sort on `created_at` needs an index holding it after
`file`.

The GraphQL type name composes the parent and the sub-resource
(`FileVersion`), so two parents may each carry a `versions` collection.
Filters, sorts, and page ceilings belong to the sub-resource. Reading a
sub-collection requires the parent's `reads_require` scopes and is metered
by the parent's `rate_class`.

## Queries

A query is a read with typed inputs and a free-form answer, for questions
a paged listing cannot express. Search is the usual case: relevance is
not a sort column and a query string is not a filter.

```json
"queries": [
  {
    "name": "search",
    "path": "/v1/search",
    "input": [
      { "name": "q", "kind": "string", "required": true },
      { "name": "limit", "kind": "int" }
    ],
    "requires": ["read"],
    "rate_class": "reads",
    "searches": ["lexical", "vector"],
    "backing": []
  }
]
```

A query renders as a REST `GET` at its declared path, a GraphQL query field
returning `JSON!`, a client method, and an MCP tool. Its inputs are
query-string parameters on REST and arguments on GraphQL. The answer is
free-form JSON, because its shape belongs to the resolver. Queries carry
the same scope and rate declarations every other operation carries.

A path may hold a template such as `/v1/files/{id}/text`. OpenAPI declares
any `{name}` segment that matches an input as a path parameter. The
generated clients substitute `{id}` only. The runtime's REST router reads
path parameters into the query's input, coerced to their declared kinds,
and a path parameter wins over a query-string pair of the same name.

The differ treats queries like actions, with one difference: removing a
query input is breaking. See [Diffing](#diffing).

### Search backings

A listing declares its cost in full, because every filter and sort claim
is index-validated. A backing does the same for a query: it names the
table, column, and index that answer the search, so a schema change that
drops the index fails generation.

```json
"backing": [
  {
    "table": "text_chunk",
    "column": "body",
    "index": "idx_chunk_body",
    "kind": "lexical"
  },
  {
    "table": "text_chunk",
    "column": "embedding",
    "index": "idx_chunk_embedding",
    "kind": "vector",
    "dimension": 768,
    "optional": true
  }
]
```

A `lexical` backing is answered by `@@` through a FULLTEXT index. A
`vector` backing is answered by KNN through an HNSW or DISKANN index. A
fused search (BM25 candidates and vector neighbors rescored together) is
two backings on one query. The fusion itself is resolver
behavior and stays out of the contract.

Validation holds a backing to the mirror image of the listing rules. The
named table, column, and index must exist. The index must hold the column.
The index must be the kind's own machinery: FULLTEXT for lexical, HNSW or
DISKANN for vector. A backing resting on a standard index is
refused with a violation that names the index and its type, the same way a
filter resting on a FULLTEXT index is. Two identical backings on one query
are refused.

Which vector index answers is the schema's choice. Moving a column from
HNSW to DISKANN does not require a contract change.

### The width

`dimension` pins the vector width the search sends, and validation holds
it against the index's own `DIMENSION`. The width changes whenever the
embedding model changes, and a query vector of the wrong width does not
search the index the way the contract describes. Pinning the width turns
a model swap that the schema has not caught up with into a generation
failure. Leave `dimension` unset when the deployment chooses the width.
A lexical backing has no width, and stating one is refused.

### Machinery a deployment configures

`"optional": true` says a deployment may lack the index. A file service
needed this: its HNSW index over `text_chunk.embedding` is created at
startup, only where an embedding model is configured, at that model's
width. A required backing would be false in every deployment without a
model.

Optional relaxes one rule: the index may be absent from the schema. An
index that is present must still hold the column, be the kind's own
machinery, and match the declared width. `kayak verify` reads it the same
way: a plan that misses the index is excused only when the database does
not define that index.

### The declared search

`searches` states which kinds of search the query performs:

```json
"searches": ["lexical", "vector"]
```

Every kind named there must have a backing of that kind, or generation
fails naming the query and the kind. This rule catches an omission: a
resolver that runs a vector search over an unindexed column writes nothing
into the contract, and `searches` gives the gate something to check. It
cannot force anyone to declare a search, which is why `kayak verify`
exists beside the static gate.

`searches` is empty by default, and a backing whose kind is not declared
is allowed and still validated, so a contract can adopt the field one
query at a time. Declaring the same kind twice is refused.

### Where backings show up

A backing does not change the wire shape. REST paths, the SDL, and client
signatures stay the same. The MCP tool carries the backings in its
`annotations.backing`, and the OpenAPI operation description reads
`Search backing: lexical via idx_chunk_body over text_chunk.body; vector
via idx_chunk_embedding over text_chunk.embedding where configured.` The
"where configured" marks an optional backing. `searches` renders nowhere.

## Field guards

A guard is a named visibility policy on one exposed field:

```json
"fields": [
  { "column": "path" },
  { "column": "digest", "guard": "audit_only" }
]
```

The service registers a decision under that name (see
[runtime.md](runtime.md#field-guards)), and the dispatcher applies it to
every row a resolver returns, on every face including subscriptions. Rows
omit denied fields, and a read is never an error. A guarded field renders
nullable on every generated surface, since the dispatcher may omit it, and
its OpenAPI property carries `x-guard` naming the policy. Guard names are
lowercase snake case.

A caller who cannot see a column cannot narrow by it either: filtering or
sorting on a hidden column is refused with `forbidden`.

Any change to a guard is breaking: adding one, swapping it, or removing
it. Adding or swapping a guard takes values away from some callers.
Removing one shows the column to every caller the guard used to deny, in
the API and in the derived engine policy (see
[generators.md](generators.md#the-engine-policy-face)). The differ reports
it so a reviewer decides.

## Rate classes

A rate class is a named consumption budget, defined once and referenced by
the operations it meters:

```json
"rate_classes": [{ "name": "reads", "units_per_minute": 6000 }]
```

A resource, action, or query then names it with `"rate_class": "reads"`.
On a resource, `rate_class` meters its reads: list, get, sub-collections,
and subscription opens. On an action it meters that action, with no
fallback to the resource's class. On a query it meters the query. A
listing costs its clamped row limit and every other operation costs one,
so a caller asking for hundred-row pages spends its budget a hundred times
faster than one fetching single rows. An exhausted budget refuses with
`too_many_requests`, which the caller may retry after waiting.

Class names are lowercase snake case and unique. A reference to an
undefined class is refused at validation. The runtime refuses to build a
dispatcher for a contract that names a class unless a rate store is
supplied.

## Scopes

`reads_require` on a resource names the scopes a caller must hold to list
it, get from it, read its sub-collections, or watch it. `requires` on an
action or query does the same for invoking it. Empty means open to any
caller the middleware admits.

```json
"reads_require": ["files_read"],
"actions": [{ "name": "remove", "method": "DELETE", "path": "/{id}", "requires": ["files_write"] }]
```

The dispatcher checks scopes after the middleware chain and before the
resolver, so an auth layer that resolves the principal mid-chain still
counts and no data is read on a refusal. An anonymous caller against a
declared scope is refused `unauthorized`. An identified caller missing a
scope is refused `forbidden`, naming the scope.

OpenAPI list, get, sub-collection, and action operations carry their
requirements as `x-requires-scopes`. Query operations in the OpenAPI
document do not. MCP tools, queries included, carry them as
`annotations.requiredScopes`. `reads_require` also feeds the engine policy:
`derive_policy` renders it as a select conjunct on the resource's table and
its sub-resource tables (see
[generators.md](generators.md#the-engine-policy-face)).

## Limits

`limits` declares request-cost ceilings, so they appear in the artifacts
and the differ tracks them:

```json
"limits": {
  "max_depth": 10,
  "max_complexity": 500,
  "max_watches_per_principal": 5
}
```

| Key | Effect |
| --- | --- |
| `max_depth` | Maximum selection depth of one GraphQL operation. The live GraphQL schema applies it. |
| `max_complexity` | Maximum field-selection count of one GraphQL operation. The live GraphQL schema applies it. |
| `max_watches_per_principal` | Maximum open subscriptions per principal. Over the ceiling refuses with `too_many_requests`; closing a subscription frees a slot. |

Every key is optional. The OpenAPI document carries the declared limits as
a root `x-limits` object. Introducing or lowering a limit is breaking.
Raising or removing one is compatible.

## Watching

`"watchable": true` opens a resource to subscribers. It adds a GraphQL
Subscription field (`fileChanged` by default) and requires the service to
register a watch resolver. Nothing about REST, MCP, or the clients
changes, because Kayak generates no long-lived HTTP operations. A resource
without a listing cannot be watchable.

Watchers narrow the stream with the same `filterable` columns list callers
use, so a column watchers filter on needs an index like any other filter.
Streams take no limit or cursor.

## GraphQL name overrides

GraphQL names are part of a deployed schema's identity, because fragments
name types and queries name fields. When the derived names need to differ,
override them per resource:

```json
"graphql": {
  "type_name": "StoredFile",
  "list_field": "storedFiles",
  "get_field": "storedFile",
  "watch_field": "storedFileChanged"
}
```

The defaults are the PascalCase singular for the type (`File`), the
camelCase resource name for the list field (`files`), the camelCase
singular for the get field (`file`), and the camelCase singular plus
`Changed` for the watch field (`fileChanged`). Sub-resources take
`type_name` and `field`. Actions and queries take `graphql_field`.

Overrides touch the GraphQL surface only. REST paths, MCP tool names, and
generated clients keep the resource name. Resource overrides are checked
against the GraphQL name grammar, the reserved `__` prefix, the root type
names `Query`, `Mutation`, and `Subscription`, and the SurrealDB v3
reserved names. Sub-resource and action overrides are checked against the
grammar, the `__` prefix, and the reserved names. Effective type names may
not collide across resources and sub-resources. The reserved-word list is
exported as `kayak::is_reserved`.

## Validation

`kayak::validate(&contract, &schema)` returns a list of violations. An
empty list means valid. Every generation target runs it first and refuses
with every violation named. The two library functions that take no
schema, `generate_mcp_tools` and `derive_policy`, cannot run it, so call
`validate` before them or go through `generate_all`, which does (see
[generators.md](generators.md#targets)).

```rust
let violations = kayak::validate(&contract, &schema);
for violation in &violations {
    eprintln!("{violation}");
}
assert!(violations.is_empty());
```

It checks:

- Revision: `ir_revision` is at least 1 and no newer than the newest
  revision this build of Kayak reads (`kayak::ir::IR_REVISION`, currently
  1), since a newer revision can carry a change an older reader would
  misread.
- Tables and columns: every named table and column exists, including
  pins, `pinned_either` columns, filters, sorts, a resource's identity
  column, and `parent_key`.
- Index rules: every filter is covered by a standard or unique index, every
  sort is reachable through such an index's prefix, the server-bound
  columns lead some index, each `pinned_either` branch passes on its own,
  and a claim resting only on a FULLTEXT or vector index names that index.
- Search: backings rest on the right kind of index, over the right column,
  at the declared width, and every declared search has a backing.
- Names: resource and sub-resource names are lowercase snake or kebab
  case; action, action input, query, query input, and rate class names are
  lowercase snake case, as are the scopes in `reads_require` and action
  `requires`; resource field renames and guard names are lowercase snake
  case and renames avoid reserved words; names are unique where they must
  be; GraphQL overrides pass the checks listed under
  [GraphQL name overrides](#graphql-name-overrides); two operations may
  not derive the same client method name.
- Shape: action methods and paths, input options, `api_prefix`, faces
  rules, `filter_options` keys, identity rules, and that a resource which
  renders rows exposes at least one field.
- References: every named rate class is defined.

It does not check:

- `max_page_size`. A value of `0` passes validation and makes the runtime
  panic on the first listing.
- Unknown JSON keys, which serde ignores.
- A query's `graphql_field`, its `requires` scope names, duplicate query
  input names, or query input `options` and `multiple`.
- A sub-resource's identity column, and the renames and guard names on
  sub-resource fields.

Run the gate in the owning service's test suite against the real schema
definitions, so schema drift fails a test that names the column.

### Verifying against a live planner

Static validation proves an index exists. It cannot prove the planner
uses it. That check needs a database client, so it lives behind the
`verify` cargo feature.

`kayak::verify::verify_contract(&client, &contract)` composes one
representative listing per filter claim and per sort claim, for resources
and sub-resources: the pins as equality binds, the claimed filter bound,
the claimed sort ordered, always with a `LIMIT`. A resource with
`pinned_either` gets that set of probes once per branch, with the branch
column bound beside the pins, since the engine answers each branch with
its own seek and one walked branch turns the whole read into a scan. It
composes one probe per
search backing through the backing's own operator (`@@` for lexical, the
`<|k,EF|>` KNN form for vector). It runs each through `EXPLAIN` against a
live database and returns every claim the planner does not serve:

- A listing claim fails when its plan iterates the table.
- A backing fails when its plan does not reach the named index. An
  optional backing is excused when this database does not define the
  index; checking that costs one extra query, spent only on an optional
  backing that already failed.

`kayak::verify::probes(&contract)` returns the composed probes without
running them, so you can read what will be asked before pointing it at a
database. SurrealDB 3.x has removed MTREE, so validation refuses a vector
backing resting on an MTREE-typed index, and verification composes only the
`<|k,EF|>` form that HNSW and DISKANN answer.

The same check runs from the CLI:

```sh
kayak verify --contract contract.json --db ws://localhost:8000 --namespace app --database app --user root --pass secret
```

It exits non-zero when any claim fails, printing each one, so it gates in
CI beside `diff`. See [generators.md](generators.md#kayak-verify).

## Diffing

`kayak::diff(&old, &new)` compares two contracts and returns a list of
`Change` values, each `Breaking` or `Compatible`. `kayak diff old new` on
the CLI prints them and exits 1 when anything is breaking. Additions are
named too, as compatible changes, so a contract that only grew still
shows what it gained.

```rust
let changes = kayak::diff(&old, &new);
let breaking = changes.iter().any(kayak::Change::is_breaking);
for change in &changes {
    println!("{}", change.message());
}
```

Resources, sub-resources, actions, queries, fields, and inputs are matched
by name (fields by API name). Renaming any of them reads as a removal plus
an addition. GraphQL names are compared by their effective value, so
writing out a default is not a change. Identities are compared on the
wire, so writing `"identity": "id"` is not a change either.

### Reported as breaking

- Contract: `auth` added, removed, or switched; `api_prefix` changed; a
  rate class budget lowered; a limit introduced or lowered.
- Resource: removed; a field removed; a field pointed at a different
  column under the same API name; a guard added, swapped, or removed; the
  identity renamed or withdrawn; a filter removed; a sort removed;
  `max_page_size` lowered; `filter_options` introduced on a filter that
  had none; a value removed from a filter's `filter_options`;
  `pinned_either` changed in any way; the list or
  get face withdrawn; the effective GraphQL type, list field, or get field
  renamed; `watchable` turned off; the watch field renamed while
  watchable; a content upload or download face removed; reads newly
  metered by a rate class; a scope added to `reads_require`.
- Sub-resource: removed; a field removed, re-pointed, or its guard moved;
  the identity renamed or withdrawn; a filter or sort removed;
  `max_page_size` lowered; the GraphQL field or type renamed.
- Action: removed; method or path changed; output changed; GraphQL field
  renamed; newly metered; a scope added to `requires`; a required input
  added; an optional input made required; an input's type changed;
  `options` introduced on an input that had none; a value removed from
  `options`.
- Query: removed; path changed; GraphQL field renamed; newly metered; a
  scope added to `requires`; a required input added; an input made
  required; an input's type changed; an input removed; `options`
  introduced or a value removed; a declared search removed; a backing
  removed, or its table, column, index, or kind changed; a backing's width
  changed or no longer pinned; a backing made optional.

Clearing an input's `options` entirely is reported as one breaking change
per removed value, even though the input then accepts anything.

### Reported as compatible

- Contract: a rate class budget raised; a limit raised or removed.
- Resource: added; a field added; an identity gained where rows had none;
  a filter added; a sort added; `max_page_size` raised; a value added to a
  filter's `filter_options`; a filter's `filter_options` removed, so it
  takes any value; a face added back; `watchable` turned on; a content
  face added; a sub-resource added; an action added; reads no longer
  metered; a scope removed from `reads_require`.
- Sub-resource: a field added; an identity gained where rows had none; a
  filter or sort added; `max_page_size` raised.
- Action: an optional input added; any input removed; a value added to
  `options`; no longer metered; a scope removed from `requires`.
- Query: added; an optional input added; a value added to `options`; no
  longer metered; a scope removed from `requires`; a search newly
  declared; a backing added; a width newly pinned; a backing made
  required of every deployment.

Removing an action input is compatible because the dispatcher drops
unknown action input keys, so an old client that still sends the input
keeps working. Queries refuse unknown inputs, which is why removing a
query input is breaking.

### Not reported

The differ emits nothing for these changes:

- Changes to `pinned`, `table`, `parent_key`, `multiple`, or any
  `description`.
- `filter_options` on a filter that is itself added or removed. The
  filter's own line covers it.
- The contract's `name`, `version`, or `ir_revision`.
- An input made optional.
- A rate class added to or removed from `rate_classes`. Changes show up
  through the operations that reference it.
- An operation moved from one rate class to another.

An empty result prints `no contract changes` on the CLI.

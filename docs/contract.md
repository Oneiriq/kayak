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
        graphql: None,
        actions: vec![/* see below */],
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

`filterable` columns become query parameters and GraphQL arguments. Each must
appear in at least one index on the table, because an unindexed filter works
in the demo and becomes a table scan in production.

`sortable` columns become sort options. The rule: some index must hold the
column at a position where every earlier column is pinned or filterable. An
index serves an ORDER BY only from a prefix whose head is equality-bound.
Given `(tenant_id, state, created_at)` with `tenant_id` pinned and `state`
filterable, `created_at` is a valid sort. Declaring a sort no index can serve
is a generation error naming the column.

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

## GraphQL name overrides

GraphQL names are part of a deployed schema's identity, since fragments name
types and queries name fields. When the derived defaults (type `File`, query
fields `files` and `file`, mutation `fileIssueUrl`) need to differ, override
them per resource:

```rust
graphql: Some(GraphqlNames {
    type_name: Some("StoredFile".into()),
    list_field: Some("storedFiles".into()),
    get_field: Some("storedFile".into()),
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
indexed, sorts are reachable through an index prefix, action definitions are
well-formed, chosen names are valid for every surface they reach.

Run the gate in the owning service's tests against the real schema
definitions. Schema drift then fails a test naming the offending column before anything
ships.

## Diffing

`janus::diff(&old, &new)` compares two contracts at the IR level and
classifies every change. Breaking: a removed resource, field, filter, or
sort; a field re-pointed to a different column under the same wire name; a
lowered page ceiling; a moved action; a changed output; an input that became
required or changed type; any effective GraphQL rename. Compatible:
additions, and removal of an optional input.

The CLI exits non-zero on breaking changes (`janus diff old.json new.json`),
which makes the gate one line of CI.

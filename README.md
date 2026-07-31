# Janus

One contract, every face. Janus is the contract layer for
SurrealDB-backed APIs: a serializable intermediate representation
authored over `surql-rs` schema definitions, validated against the
schema's real indexes at build time, and compiled into every API
surface — so the surfaces cannot drift from each other or from the
database.

```text
contract (IR, checked in)  +  schema (surql-rs TableDefinitions)
        |
        |-- validate: fields exist, renames do not collide, filters are
        |   indexed, sorts are reachable through an index prefix whose
        |   earlier columns are pinned or filterable, actions are sane
        |
        |-- openapi.json      OpenAPI 3.1 (list/get + action paths)
        |-- schema.graphql    SDL (types, sort enums, Query, Mutation)
        |-- client.rs         reqwest + serde
        |-- client.ts         fetch, zero dependencies
        |-- client.py         standard library only
        |-- client.go         net/http only
```

Nothing is exposed by default: fields are allowlisted and renameable,
filters and sorts are explicit, and declaring a sortable field no index
can serve is a **generation error naming the field** — the class of
failure that otherwise ships and becomes a production table scan.

Actions model everything beyond list/get — uploads, grants, deletes,
workflow starts — with typed inputs and instance/collection targeting;
they become OpenAPI operations, GraphQL mutations, and client methods
from the same declaration.

GraphQL names are overridable per resource (`graphql.type_name`,
`graphql.list_field`, `graphql.get_field`, and per-action
`graphql_field`) without touching REST paths or generated clients.
Overrides and renames are gated: valid GraphQL grammar, no `__`
prefix, no root-type collisions, and no collisions with SurrealDB
v3 reserved names (the reserved-word list is exported as
`janus::is_reserved` for schema layers to reuse). Changing an
effective GraphQL name is a breaking change to the differ.

## Runtime

The contract is also executable. With the `runtime` feature a service
registers its own resolvers — async closures over its own data access
(surql-rs repositories, caches, other services) — and stacks
middleware around them; the dispatcher enforces the contract before
any resolver runs (limits clamped, filters and sorts allowlisted,
action inputs type-checked, unknown inputs dropped).

With the `graphql` feature the same contract builds an `async-graphql`
dynamic schema: every query and mutation field funnels through the
dispatcher, so middleware and enforcement behave identically across
protocols, and the served schema cannot disagree with the checked-in
SDL artifact — both derive from one contract object.

```rust
let resolvers = Resolvers::new()
    .list("files", |ctx, args| async move { /* your repo call */ })
    .get("files", |ctx, args| async move { /* ... */ })
    .action("files", "issue_url", |ctx, args| async move { /* ... */ });

let dispatcher = Arc::new(Dispatcher::new(contract, resolvers, vec![
    Arc::new(RequireTenant),          // your Middleware impls
])?);
let schema = janus::runtime::graphql::build_schema(&tables, dispatcher)?;
// or schema_builder(...) to attach async-graphql extensions,
// depth/complexity limits, and global data before finishing.
```

Per-request values (tenant, principal) travel in a typed
`JanusContext` injected as request data; middleware reads, enriches,
or rejects. A missing context is an empty context, so
context-requiring middleware fails closed.

## CLI

```
janus generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,client-rs,client-ts,client-py,client-go]
janus diff old-contract.json new-contract.json   # exits non-zero on breaking changes
```

Contracts and schemas travel as data; the schema file is a serialized
`Vec<TableDefinition>` exported by the owning service. `diff` operates
on the IR, not the documents, so it catches what document diffs hide:
a dropped filter, a moved action, an input that became required.

## Testing

Golden files per generator (`JANUS_BLESS=1 cargo test` re-blesses
deliberately), gate refusal tests by name, IR round-trips, and a CLI
integration test that — where the toolchains exist — compiles the
generated Python and parses the generated Go with the real tools.

## License

Apache-2.0.

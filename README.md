# Janus

One contract, every face. Janus is the contract layer for SurrealDB-backed
APIs: a serializable intermediate representation authored over `surql-rs`
schema definitions, validated against the schema's real indexes at build
time, and compiled into every API surface. The surfaces cannot drift from
each other or from the database, because they are the same object.

```text
contract (IR, checked in)  +  schema (surql-rs TableDefinitions)
        |
        |-- validate: fields exist, renames are safe, filters are
        |   indexed, sorts are reachable through an index prefix,
        |   actions and chosen names are well-formed
        |
        |-- openapi.json      OpenAPI 3.1 (page envelopes, action paths)
        |-- schema.graphql    SDL (types, sort enums, Query, Mutation,
        |                     Subscription)
        |-- client.rs         reqwest + serde
        |-- client.ts         fetch, zero dependencies
        |-- client.py         standard library only
        |-- client.go         net/http only
        |
        |-- runtime           the contract, executed: resolvers,
                              middleware, live GraphQL
```

Nothing is exposed by default. Fields are allowlisted and renameable,
filters and sorts are explicit, and declaring a sortable field no index can
serve is a generation error naming the field. That class of failure
otherwise ships quietly and becomes a production table scan.

Actions model everything beyond list and get (uploads, grants, deletes,
workflow starts) with typed inputs and instance or collection targeting.
They become OpenAPI operations, GraphQL mutations, and client methods from
the same declaration.

## Runtime

With the `runtime` feature the contract executes. A service registers its
own resolvers (async closures over its own data access) and stacks
middleware around them. The dispatcher enforces the contract before any
resolver runs: limits clamp and every argument checks against its declaration. Construction refuses, by
name, any declared operation without a resolver.

With the `graphql` feature the same contract builds a live schema on
`async-graphql`. Every field dispatches through the middleware chain, and
the served schema matches the generated SDL by construction. A resource the
contract marks watchable gains a Subscription field over a stream resolver
the service registers; the chain runs once, when the subscription opens. GraphQL name
overrides (type and field names, per resource and per action) are validated
against the GraphQL grammar and the SurrealDB v3 reserved-word list, which
is exported as `janus::is_reserved`. Renaming any effective GraphQL name is
a breaking change to the differ.

See [docs/](docs/README.md) for the contract reference, the runtime guide,
and the generator workflow.

## CLI

```
janus generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,client-rs,client-ts,client-py,client-go]
janus diff old-contract.json new-contract.json   # exits non-zero on breaking changes
```

Contracts and schemas travel as data; the schema file is a serialized
`Vec<TableDefinition>` exported by the owning service. `diff` operates on
the IR, so it catches what document diffs hide: a dropped filter, a moved
action, an input that became required, a field re-pointed at a different
column under the same wire name.

## Testing

Golden files per generator (`JANUS_BLESS=1 cargo test` re-blesses as an
explicit step), gate refusal tests by name, IR round-trips, a runtime suite
covering middleware ordering and enforcement, and a CLI integration test
that compiles the generated Python and parses the generated Go with the
real toolchains where they exist.

## License

Apache-2.0.

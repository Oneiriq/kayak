# Kayak

Steer, don't drift. Kayak is the contract layer for SurrealDB-backed
APIs: a serializable intermediate representation authored over `surql-rs`
schema definitions, validated against the schema's real indexes at build
time, and compiled into every API surface. The surfaces cannot drift from
each other or from the database, because they are the same object.

The name is the job. An API ecosystem left alone drifts the way anything
on water does: the database moves, a handler follows, a client lags, a
document lies. A kayak sits low in the same current and holds its line
anyway. One paddler, one hull, every stroke visible -- which is the
whole design: one small checked-in contract, and every face of the API
follows its course or the build refuses.

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
        |-- mcp-tools.json    MCP manifest (tools with typed inputs,
        |                     scope + rate annotations)
        |-- client.rs         reqwest + serde
        |-- client.ts         fetch, zero dependencies
        |-- client.py         standard library only
        |-- client.go         net/http only
        |                     (each carries resources, actions, and
        |                      queries)
        |
        |-- runtime           the contract, executed: resolvers,
        |                     middleware, live GraphQL
        |-- console           the operator surface, rendered from the
                              same declarations (zero JavaScript)
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
is exported as `kayak::is_reserved`. Renaming any effective GraphQL name is
a breaking change to the differ.

See [docs/](docs/README.md) for the contract reference, the runtime guide,
and the generator workflow.

## CLI

```
kayak scaffold --schema schema.json --out contract.json [--name svc] \
    [--version 0.1.0] [--pinned tenant_id]
kayak generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,client-rs,client-ts,client-py,client-go]
kayak diff old-contract.json new-contract.json   # exits non-zero on breaking changes
```

Contracts and schemas travel as data; the schema file is a serialized
`Vec<TableDefinition>` exported by the owning service. `diff` operates on
the IR, so it catches what document diffs hide: a dropped filter, a moved
action, an input that became required, a field re-pointed at a different
column under the same wire name.

### Starting from a database you already have

`scaffold` reads a schema and writes a contract that validates against it,
which is the step that otherwise means copying every column by hand and
checking every filter and sort against an index by eye. Run against copal's
own 25 tables it exposes 22, derives 39 filters and 28 sorts, and all seven
artifacts generate from the result without an edit.

What it declines to guess is as much of the point. Actions are behavior and
live in the service, so a scaffolded resource has none. Columns whose names
suggest a secret (`key_hash`, `secret_sealed`) are left unexposed and named
on stderr, because a tool that writes API surface should not be how a hash
reaches a client. Tables whose pinned columns no index leads with are
declined whole and named the same way: the pins ride every read, so every
listing of such a table scans it, and both repairs (lead an index with a
pin, or reach the table through a parent as a sub-collection) are the
author's to choose. The three copal tables declined that way are exactly
the ones its hand-written contract never lists at the top level: versions
reached through their file, deliveries through their endpoint, TUS uploads
through their own protocol. And sort claims stay narrower than `validate` would
tolerate: a sort is claimed only where pinned columns cover the whole index
prefix ahead of it. `filterable` describes what a caller may send, nothing
obliges them to send it, and an unfiltered sort down a composite index
scans.

The bias throughout is to claim less, because the differ calls a removed
filter or sort breaking and an added one compatible. A claim the scaffold
invents costs a major version to withdraw; one it omits costs a line to
add. Copal's `created_at` listing sort is the intended example: left
unclaimed, correct to add by hand, compatible when you do.

## Testing

Golden files per generator, gate refusal tests by name, IR round-trips, a
runtime suite
covering middleware ordering and enforcement, and a CLI integration test
that compiles the generated Python and parses the generated Go with the
real toolchains where they exist.

Re-blessing a golden is an explicit step that names what you meant to
change:

```
KAYAK_BLESS=client-go cargo test          one artifact
KAYAK_BLESS=client-go,openapi cargo test  several
KAYAK_BLESS=1 cargo test                  all of them
```

An unrecognised name fails rather than blessing nothing quietly, so a
typo cannot pass for a clean run. Blessing everything is right after a
change that touches every face; it is not a way to read a diff, since a
failing run already prints both sides.

## License

Apache-2.0.

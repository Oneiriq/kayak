# Kayak

Kayak is a contract layer for SurrealDB-backed APIs. You describe what an
API exposes in one small checked-in contract over `surql-rs` schema
definitions. Kayak validates the contract against the schema's real
indexes at build time, then compiles it into every API surface. The
surfaces can't drift from each other or from the database, because they
all come from the same object.

The name: this tool exists to stop schema drift, and a kayak is a small
boat that holds its line in a current. That's the whole metaphor.

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
filters and sorts are explicit, and declaring a sortable field that no
index can serve is a generation error that names the field. Without the
check, that mistake ships quietly and turns into a production table scan.

Actions model everything beyond list and get (uploads, grants, deletes,
workflow starts) with typed inputs and instance or collection targeting.
One declaration becomes the OpenAPI operation, the GraphQL mutation, and
a method on each generated client.

## Runtime

With the `runtime` feature the contract executes. A service registers its
own resolvers (async closures over its own data access) and stacks
middleware around them. The dispatcher enforces the contract before any
resolver runs: limits are clamped and every argument is checked against
its declaration. If the contract declares an operation and no resolver is
registered for it, construction fails and names the operation.

With the `graphql` feature the same contract builds a live schema on
`async-graphql`. Every field dispatches through the middleware chain, and
the served schema matches the generated SDL by construction. A resource
the contract marks watchable gains a Subscription field over a stream
resolver the service registers; the chain runs once, when the
subscription opens. GraphQL name overrides (type and field names, per
resource and per action) are validated against the GraphQL grammar and
the SurrealDB v3 reserved-word list, which is exported as
`kayak::is_reserved`. Renaming any effective GraphQL name is a breaking
change to the differ.

See [docs/](docs/README.md) for the contract reference, the runtime
guide, and the generator workflow.

## CLI

```
kayak scaffold --schema schema.json --out contract.json [--name svc] \
    [--version 0.1.0] [--pinned tenant_id]
kayak generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,mcp,client-rs,client-ts,client-py,client-go]
kayak diff old-contract.json new-contract.json   # exits non-zero on breaking changes
kayak verify --contract contract.json --db ws://localhost:8000 \
    --namespace ns --database db [--user root --pass secret]
```

Contracts and schemas travel as data; the schema file is a serialized
`Vec<TableDefinition>` exported by the owning service. `diff` operates on
the IR, so it catches what document diffs hide: a dropped filter, a moved
action, an input that became required, a field re-pointed at a different
column under the same wire name.

`verify` asks a live database's planner what static validation cannot. It
runs `EXPLAIN` on a representative listing for every filter and sort claim
and on every search backing's own operator, and exits non-zero naming each
claim the plan answers with a table walk or the wrong index. It needs the
`verify` feature, which carries the database client the rest of kayak
leaves out.

### Starting from a database you already have

`scaffold` reads a schema and writes a contract that validates against
it. Without it, that step means copying every column by hand and checking
every filter and sort against an index by eye. Run against a real file
service's 25 tables, it exposes 22, derives 39 filters and 28 sorts, and all seven
artifacts generate from the result without an edit.

The scaffold also refuses to guess a few things, on purpose. Actions are
behavior and live in the service, so a scaffolded resource has none.
Columns whose names suggest a secret (`key_hash`, `secret_sealed`) are
left unexposed and named on stderr, because a tool that writes API
surface shouldn't be how a hash reaches a client. Tables whose pinned
columns don't lead any index are declined whole and named the same way:
the pins apply to every read, so every listing of such a table scans it.
Both repairs (lead an index with a pin, or reach the table through a
parent as a sub-collection) are the author's to choose. The three tables
it declined there are exactly the ones the service's hand-written contract
never lists at the top level: versions reached through their file,
deliveries through their endpoint, TUS uploads through their own
protocol. Sort claims stay narrower than `validate` would tolerate: a
sort is claimed only where pinned columns cover the whole index prefix
ahead of it. `filterable` describes what a caller may send, nothing
obliges them to send it, and an unfiltered sort down a composite index
scans.

Throughout, the scaffold prefers to claim less, because the differ calls
a removed filter or sort breaking and an added one compatible. A claim
the scaffold invents costs a major version to withdraw; one it omits
costs a line to add. That service's `created_at` listing sort is the intended
example: left unclaimed, correct to add by hand, compatible when you do.

## Testing

Golden files per generator, gate refusal tests by name, IR round-trips, a
runtime suite covering middleware ordering and enforcement, and a CLI
integration test that compiles the generated Python and parses the
generated Go with the real toolchains where they exist.

Re-blessing a golden is an explicit step that names what you meant to
change:

```
KAYAK_BLESS=client-go cargo test          one artifact
KAYAK_BLESS=client-go,openapi cargo test  several
KAYAK_BLESS=1 cargo test                  all of them
```

An unrecognized name fails instead of silently blessing nothing, so a
typo can't pass as a clean run. Bless everything after a change that
touches every generator; to see what a change actually did, read the
failing test's output, which already prints both sides.

## License

Apache-2.0.

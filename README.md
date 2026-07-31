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

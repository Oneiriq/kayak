# Generators and the CLI

One contract and one schema produce six artifacts. Every generator validates
first; an invalid contract refuses with each violation named.

| Target | File | Contents |
| --- | --- | --- |
| `openapi` | `openapi.json` | OpenAPI 3.1. List endpoints return the `{Type}Page` envelope; actions merge into their path items. |
| `sdl` | `schema.graphql` | Object types, sort enums, page types, Query and Mutation. Scalars `DateTime` and `JSON` appear only when used. |
| `client-rs` | `client.rs` | Rust client on `reqwest` and `serde`. |
| `client-ts` | `client.ts` | TypeScript client on `fetch`, zero dependencies. |
| `client-py` | `client.py` | Python client, standard library only. |
| `client-go` | `client.go` | Go client, `net/http` only. |

Each client is one self-contained file: typed resources plus a method for
every operation, list and get included. Emission is deterministic, and the OpenAPI document serializes
with canonical key order so artifact bytes never depend on feature
unification in the dependency graph.

## Library use

```rust
let artifacts = janus::generate_all(&contract, &schema, janus::generate::TARGETS)?;
for (filename, content) in &artifacts {
    std::fs::write(out_dir.join(filename), content)?;
}
```

## CLI use

```
janus generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,client-rs]
janus diff old-contract.json new-contract.json
```

Contracts and schemas travel as data. The schema file is a serialized
`Vec<TableDefinition>`, exported by the owning service, so generation needs
no Rust evaluation of the schema source. `diff` prints each change labeled
BREAKING or compatible and exits non-zero when anything breaks, which makes
contract review one CI line.

## The golden workflow

Consumers check the generated artifacts in and gate them by test:

```rust
let artifacts = generate_all(&contract(), &schema, TARGETS)?;
for (filename, content) in &artifacts {
    if std::env::var("MYSERVICE_BLESS").is_ok() {
        std::fs::write(&checked_in_path, content)?;
    }
    assert_eq!(content.trim(), std::fs::read_to_string(&checked_in_path)?.trim());
}
```

A contract or schema change then shows up as a reviewable artifact diff in
the same commit. Re-blessing is an explicit step (`MYSERVICE_BLESS=1 cargo test`);
any other drift fails the build. Janus's own test suite goes one step
further where the toolchains exist: the generated Python compiles under
`py_compile` and the generated Go parses under `gofmt`, so client syntax is
proven by real compilers.

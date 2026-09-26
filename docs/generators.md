# Generators and the CLI

One contract and one schema produce seven artifacts by default, and two
more on request. Every target validates first, and an invalid contract
refuses with each violation named. The MCP manifest and the engine policy
read only the contract, so their own library functions
(`generate_mcp_tools`, `derive_policy`) take no schema and cannot validate.
`generate_all` and the CLI validate before rendering them.

| Target | File | Contents |
| --- | --- | --- |
| `openapi` | `openapi.json` | OpenAPI 3.1. List endpoints return the `{Type}Page` envelope; actions merge into their path items. |
| `sdl` | `schema.graphql` | Object types, sort enums, page types, Query and Mutation. Scalars `DateTime` and `JSON` appear only when used. |
| `mcp` | `mcp-tools.json` | The `tools/list` manifest: one tool per declared operation, with input schemas, and scopes and rate classes as annotations. |
| `client-rs` | `client.rs` | Rust client on `reqwest` and `serde`. |
| `client-rs-blocking` (opt-in) | `client_blocking.rs` | The same client on `reqwest::blocking`, for callers with no runtime to await on. Opt-in because it is a second flavor of a language the default set already covers. |
| `client-ts` | `client.ts` | TypeScript client on `fetch`, zero dependencies. |
| `client-py` | `client.py` | Python client, standard library only. |
| `client-go` | `client.go` | Go client, `net/http` only. |
| `engine-policy` (opt-in) | `policy.json` | The derived engine row-security clauses: select conjuncts from `reads_require`, field redactions from guards. See below. |

Resource routes hang under the contract's `api_prefix` (`/v1` by
default; see `docs/contract.md`), so the paths above are what a contract
that says nothing gets. Query paths are absolute and never prefixed.

Each client is one self-contained file: typed resources plus a method for
every operation, list and get included. Emission is deterministic, and the OpenAPI document serializes
with canonical key order so artifact bytes never depend on feature
unification in the dependency graph.

## Library use

```rust
let artifacts = kayak::generate_all(&contract, &schema, kayak::generate::TARGETS)?;
for (filename, content) in &artifacts {
    std::fs::write(out_dir.join(filename), content)?;
}
```

## CLI use

```
kayak generate --contract contract.json --schema schema.json \
    --out generated [--targets openapi,sdl,client-rs]
kayak diff old-contract.json new-contract.json
```

Contracts and schemas travel as data. The schema file is a serialized
`Vec<TableDefinition>`, exported by the owning service, so generation needs
no Rust evaluation of the schema source. `diff` prints each change labeled
BREAKING or compatible and exits non-zero when anything breaks, which makes
contract review one CI line.

## The engine policy face

A SurrealDB deployment can enforce the contract a second time at the
engine: table `PERMISSIONS` filter rows and field `PERMISSIONS` redact
columns for sessions authenticated as callers rather than as the
service. `kayak::derive_policy` renders those clauses from the
contract, so tightening a scope or guarding a field moves both
enforcement layers in one edit instead of leaving the engine on
yesterday's contract:

```rust
let policy = kayak::derive_policy(&contract, &kayak::ClaimVocabulary::default())?;
// policy.select_conjuncts: (table, "$token.sc CONTAINS 'read'") for
//   every resource whose reads require scopes, sub-resource tables
//   included, since a sub-collection is read under its parent's
//   requirement.
// policy.field_guards: (table, column, clause) for every guarded
//   exposure, resources and sub-resources alike.
```

Which token claims those clauses read is deployment convention rather
than contract content, so it travels as a `ClaimVocabulary`: the claim
carrying the scope list, and the engine clause each named guard
becomes. A guard the contract declares that the vocabulary cannot
render refuses the derivation naming the guard, because rendering
nothing would silently drop the engine layer for that column while
the application layer kept enforcing. That refusal is also why the
CLI target is opt-in (`--targets engine-policy`): the CLI holds only
the default vocabulary, and a deployment with its own guard names
derives through the library API.

Two rules stay with the service on purpose. The mechanical tenancy
floor (tenant-scoped tables admit only their tenant's rows, tables
without the column are closed) must derive from the schema rather
than the contract, or a table left out of the contract would dodge
it. And delete conjuncts, such as retention, are policy the contract
cannot declare yet. Kayak derives only what the contract declares;
the floor and the retention rules are the service's to state.

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
any other drift fails the build. Kayak's own test suite goes one step
further where the toolchains exist: the generated Python compiles under
`py_compile` and the generated Go parses under `gofmt`, so client syntax is
proven by real compilers.

# Generators and the CLI

Generation takes two inputs, a contract and the schema it is written
against, and writes artifacts: an OpenAPI document, a GraphQL SDL, an MCP
tool manifest, typed clients, and optionally an engine row-security
policy. You run it from the `kayak` CLI or call it as a library. This page
covers both, what each artifact contains, and how to keep checked-in
artifacts in sync.

Read [contract.md](contract.md) first for what a contract says.

## Inputs

The contract is one JSON file, or a directory of them (see
[Directory contracts](#directory-contracts)).

The schema is a JSON array of `surql` `TableDefinition` values (crate
`oneiriq-surql`, imported as `surql`), the same definitions the service
applies to its database. Kayak has no command that exports a schema. The
service that owns the definitions writes the file, for example from a
test or a small binary:

```rust
let tables: Vec<surql::schema::TableDefinition> = my_service::schema::tables();
std::fs::write("api/schema.json", serde_json::to_string_pretty(&tables)?)?;
```

Because both inputs are data, generation never compiles or runs the
service's schema code.

## Targets

| Target | File | Default | Validates first | Contents |
| --- | --- | --- | --- | --- |
| `openapi` | `openapi.json` | yes | yes | OpenAPI 3.1 document. |
| `sdl` | `schema.graphql` | yes | yes | GraphQL SDL. |
| `mcp` | `mcp-tools.json` | yes | no | MCP `tools/list` manifest. |
| `client-rs` | `client.rs` | yes | yes | Async Rust client on `reqwest`. |
| `client-ts` | `client.ts` | yes | yes | TypeScript client on `fetch`. |
| `client-py` | `client.py` | yes | yes | Python client on the standard library. |
| `client-go` | `client.go` | yes | yes | Go client on the standard library. |
| `client-rs-blocking` | `client_blocking.rs` | no | yes | The Rust client on `reqwest::blocking`, for callers with no async runtime. |
| `engine-policy` | `policy.json` | no | no | Engine row-security clauses. See [The engine policy face](#the-engine-policy-face). |

The default set is `kayak::generate::TARGETS`. The two opt-in targets are
generated only when named.

A target that validates first refuses an invalid contract with every
violation listed. `mcp` and `engine-policy` do not validate: they render
whatever contract they are given. A default run always includes
`openapi`, which validates, so a default run of an invalid contract fails.
When you generate `mcp` or `engine-policy` on their own, run
`kayak::validate` or a validating target alongside them.

Output is deterministic. The OpenAPI document is written with every
object's keys sorted, so its bytes do not depend on whether some other
crate in the build turned on `serde_json`'s `preserve_order` feature.

## The CLI

```sh
kayak scaffold --schema <file> [--out <file>] [--name <name>] [--version <semver>] [--pinned <columns>]
kayak generate --contract <file-or-dir> --schema <file> --out <dir> [--targets <list>]
kayak diff <old-contract> <new-contract>
kayak verify --contract <file-or-dir> --db <url> --namespace <ns> --database <db> [--user <name> --pass <secret>]
```

Installation is covered in the repository's root README. `verify` needs
a binary built with the `verify` feature.

The argument parser is small and strict about form:

- Flags take their value as the next argument: `--out generated`. The
  `--out=generated` form is not recognized.
- Unknown flags are ignored. If a flag is given twice, the first one
  wins.
- There is no `--help` or `--version`. Running `kayak` with no
  subcommand, or an unknown one, prints the usage text and exits 2.

| Exit code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | The command ran and failed: an unreadable or invalid input, a breaking diff, a claim that failed `verify`, or a write error. |
| 2 | Usage error: a missing required flag or argument, or `verify` in a binary built without the feature. |

### kayak scaffold

Writes a starting contract from a schema. Use it when a service already
has a database and no contract.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--schema <file>` | required | The schema file. |
| `--out <file>` | standard output | Where to write the contract. |
| `--name <name>` | `service` | The contract's `name`. |
| `--version <semver>` | `0.1.0` | The contract's `version`. |
| `--pinned <columns>` | `tenant_id` | Comma-separated server-bound columns. Pass `--pinned ""` for none. |

```sh
kayak scaffold --schema api/schema.json --out api/contract.json --name filestore
```

For each table it writes one resource named with the plural of the table
name. It exposes every column except the pinned ones and any column whose
name contains `secret`, `token`, `password`, `passwd`, `hash`,
`credential`, `private_key`, `api_key`, or `salt`. A pinned column is
applied only to tables that have it. `filterable` gets every exposed
column that appears in a standard or unique index. `sortable` gets every
exposed column that some standard or unique index holds with nothing but
pinned columns ahead of it. That is narrower than validation allows,
which also credits filterable columns: adding a sort later is compatible,
while removing a wrong one is breaking. It declines any table that has a
pinned column but no index leading with one, since every listing of that
table would scan. It writes no actions, queries, sub-resources, guards,
scopes, or rate classes.

Everything besides the contract goes to standard error, so standard
output stays a clean contract when `--out` is omitted:

- `withheld table.column: ...` for each column left out as a likely secret.
- `declined table: ...` for each table left out.
- `needs an edit: <violation>` for anything that still fails validation,
  such as a table name that is not a valid resource name.
- A summary of how many resources, filters, and sorts it derived.

It exits 0 even when it prints `needs an edit` lines. It exits 1 when the
schema cannot be read or defines no tables, and 2 without `--schema`.

### kayak generate

| Flag | Default | Meaning |
| --- | --- | --- |
| `--contract <file-or-dir>` | required | The contract. |
| `--schema <file>` | required | The schema file. |
| `--out <dir>` | required | Output directory, created if missing. |
| `--targets <list>` | the default set | Comma-separated target names. |

```sh
kayak generate --contract api/contract.json --schema api/schema.json --out api/generated
kayak generate --contract api/contract.json --schema api/schema.json --out api/generated \
    --targets openapi,sdl,mcp,client-rs,client-rs-blocking,engine-policy
```

It prints `wrote <dir>/<file>` for each artifact. Every target is
generated before anything is written, so a failing target leaves the
output directory untouched. An unknown target name fails with
`unknown generation target`.

### kayak diff

```sh
kayak diff api/contract-main.json api/contract.json
```

Both arguments are contract files or directories. It prints one line per
change:

```text
BREAKING   files: sort created_at removed
compatible files: field digest added
```

It prints `no contract changes` when there are none. It exits 1 when any
change is breaking, 0 otherwise, 2 with fewer than two paths, and 1 when a
contract cannot be read. The rules behind each classification are in
[contract.md](contract.md#diffing).

### kayak verify

Runs every filter, sort, and search backing claim through `EXPLAIN` on a
live SurrealDB and fails the claims the planner does not serve. See
[contract.md](contract.md#verifying-against-a-live-planner) for what it
probes.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--contract <file-or-dir>` | required | The contract. |
| `--db <url>` | required | A `ws://` or `wss://` URL. |
| `--namespace <ns>` | required | The namespace to use. |
| `--database <db>` | required | The database to use. |
| `--user <name>` | none | Sign-in user. |
| `--pass <secret>` | none | Sign-in password. |

On success it prints `every filter, sort, and backing claim plans on its
index` and exits 0. Otherwise it prints one `SCANS` line per failing claim,
naming the scope, the claim, the plan operation, and the probe query, and
exits 1. It exits 1 when it cannot connect, and 2 when a required flag is
missing or the binary was built without the `verify` feature.

### Directory contracts

Every command that reads a contract (`generate`, `diff`, `verify`) also
accepts a directory holding one entity per file:

```text
api/contract/
  contract.json        name, version, auth, api_prefix, limits, rate_classes
  resources/
    files.json         one resource object
    folders.json
  queries/
    search.json        one query object
```

- `contract.json` holds every contract key except `resources` and
  `queries`. Including either key there is refused.
- Every file under `resources/` and `queries/` must end in `.json`. Any
  other file, or a subdirectory, is refused.
- Files are read in sorted file-name order. That order is the order
  entities appear in the artifacts, so a numeric prefix
  (`10-files.json`) is a way to control it.
- Either directory may be missing, but the contract must declare at least
  one resource or query.
- A UTF-8 byte-order mark at the start of any file is ignored.

## Library use

```rust
use kayak::generate::TARGETS;

let artifacts = kayak::generate_all(&contract, &schema, TARGETS)?;
for (filename, content) in &artifacts {
    std::fs::write(out_dir.join(filename), content)?;
}
```

`generate_all` returns a map from file name to content. Pass your own
slice, such as `&["openapi", "client-ts"]`, to pick targets. The
individual generators are public too:

| Function | Returns |
| --- | --- |
| `kayak::validate(&contract, &schema)` | `Vec<Violation>` |
| `kayak::generate_openapi(&contract, &schema)` | `serde_json::Value` |
| `kayak::generate_sdl(&contract, &schema)` | `String` |
| `kayak::generate_mcp_tools(&contract)` | `serde_json::Value` |
| `kayak::clients::generate_client_rs(&contract, &schema)` | `String` |
| `kayak::clients::generate_client_rs_blocking(&contract, &schema)` | `String` |
| `kayak::clients::generate_client_ts(&contract, &schema)` | `String` |
| `kayak::clients::generate_client_py(&contract, &schema)` | `String` |
| `kayak::clients::generate_client_go(&contract, &schema)` | `String` |
| `kayak::derive_policy(&contract, &vocabulary)` | `EnginePolicy` |
| `kayak::diff(&old, &new)` | `Vec<Change>` |
| `kayak::scaffold::scaffold(name, version, &schema, &pinned)` | `Scaffold { contract, withheld, declined }` |

Generators return `Result<_, GenerateError>`. `GenerateError::Invalid`
carries every violation. `derive_policy` returns `PolicyError`.

## What each artifact contains

### OpenAPI

The document is OpenAPI 3.1.0, titled with the contract's `name` and
versioned with its `version`.

For each resource it declares a component schema named with the
PascalCase singular of the resource (`File`) and a page envelope
(`FilePage`) holding `items` and a nullable `next_cursor`. Sub-resources
get the same pair, named after parent and child (`FileVersion`,
`FileVersionPage`). Property types come from the schema:

| Column type | JSON Schema |
| --- | --- |
| string, record, file | `string` |
| int | `integer` |
| float, decimal, number | `number` |
| bool | `boolean` |
| datetime | `string`, format `date-time` |
| duration | `string`, format `duration` |
| bytes | `string`, format `byte` |
| object, geometry | `object` |
| array | `array` |
| any | no type |

A nullable column becomes a type union with `"null"`. A guarded field
carries `x-guard` and is left out of `required`.

Paths, with the default prefix:

| Path | Present when | Details |
| --- | --- | --- |
| `GET /v1/files` | list face | `limit` (1 to `max_page_size`, default `max_page_size`), `cursor`, one string parameter per filterable column, and `sort` with values `col` and `-col`. |
| `GET /v1/files/{id}` | get face | The path parameter is the identity column. 200 or 404. |
| `GET /v1/files/{id}/versions` | a sub-resource | Paging and filters as a list. `sort` values are `col:asc` and `col:desc`. |
| `<METHOD> /v1/files<action path>` | an action | Merged into the path item. `id` path parameter when the path holds `{id}`. JSON request body when the action has inputs. 200 with the resource or an object, or 204 for `"none"`. |
| `PUT` and `GET /v1/files/{id}/content` | content faces | `application/octet-stream` bodies. |
| `GET <query path>` | a query | Inputs named in the path are path parameters; the rest are query parameters. 200 with an object. |

Inputs with `options` carry an `enum`, and `multiple` inputs are declared
as arrays. Operation ids follow `list_files`, `get_files`,
`list_versions_files`, `issue_url_files`, and the query's own name.

Extensions: list, get, sub-collection, and action operations carry
`x-requires-scopes` when they require scopes. A declared `limits` object
appears at the root as `x-limits`. A `bearer` or `header` auth scheme
appears under `components.securitySchemes` with a top-level `security`
requirement. A query with backings describes them in its operation
`description`.

### GraphQL SDL

For each resource:

- An object type named by `graphql_type_name` (`File`), with the identity
  field as `ID!` unless it is already an exposed field or the identity is
  `null`, then every exposed field under its API name. A field is non-null
  unless its column is nullable or it is guarded.
- A field for each sub-resource, such as
  `versions(limit: Int = 50, cursor: String, sort: FileVersionSort): FileVersionPage!`.
- A `FileSort` enum with `COL_ASC` and `COL_DESC` values when it has sorts.
- A `FilePage` type with `items: [File!]!` and `nextCursor: String`.

The roots:

- `Query` holds each resource's list field when it has a list face
  (`files(limit: Int = 100, cursor: String, state: String, sort: FileSort): FilePage!`),
  its get field when it has a get face (`file(id: ID!): File`, where the
  argument is the identity column), and one `JSON!` field per query.
- `Mutation` appears when any resource has actions. Each action is one
  field returning the resource type, `JSON!`, or `Boolean!`.
- `Subscription` appears when any resource is watchable. Each watchable
  resource gets one field (`fileChanged(state: String): File!`) whose
  arguments are the filterable columns.

Column types map to `String` (string, record, file, duration, bytes),
`Int` (int), `Float` (float, decimal, number), `Boolean` (bool),
`DateTime` (datetime), and `JSON` (object, geometry, array, any). The
`DateTime` and `JSON` scalars are declared only when used. Object fields
keep their API names as written, while arguments are camelCase
(`created_at` as a filter is the `createdAt` argument).

### MCP tool manifest

`mcp-tools.json` is a `tools/list` result: `{"tools": [...]}`. Each tool has
a `name`, a `description`, an `inputSchema` with
`additionalProperties: false`, and `annotations`.

| Tool | Present when | Input |
| --- | --- | --- |
| `{resource}_list` (`files_list`) | list face | `limit`, `cursor`, one string per filterable column, and `sort` (a column name, with a `:desc` suffix for descending) |
| `{singular}_get` (`file_get`) | get face | the identity column, required |
| `{singular}_{action}` (`file_issue_url`) | an action | `id` when the path holds `{id}`, then the inputs |
| `{query}` (`search`) | a query | the inputs |

Inputs with `options` carry an `enum`, and `multiple` inputs are declared
as arrays. `annotations.requiredScopes` lists required scopes,
`annotations.rateClass` names the rate class, and a query's
`annotations.backing` lists its search backings.

There are no tools for sub-resources, subscriptions, or content faces.
Kayak writes the manifest only; serving MCP is up to the host (see
[runtime.md](runtime.md#mcp)).

### Clients

Each client is one self-contained file with a type for every resource and
sub-resource, a page type for each, and one method per operation. Types
carry the identity field (unless the identity is `null`) and every exposed
field; nullable and guarded fields are optional.

| Client | Constructor | Dependencies |
| --- | --- | --- |
| Rust, `client.rs` | `Client::new(base_url, credential)` | `reqwest` with `json`, `serde` with `derive`, and `serde_json` when the file uses `Value` |
| Rust blocking, `client_blocking.rs` | `Client::new(base_url, credential)` | as above, plus the `reqwest` `blocking` feature |
| TypeScript, `client.ts` | `new Client(baseUrl, credential)` | none; uses the global `fetch` and `URLSearchParams.size`, so Node 20 or later or a current browser |
| Python, `client.py` | `Client(base_url, credential)` | standard library only (`json`, `urllib`, `dataclasses`) |
| Go, `client.go` | `NewClient(baseURL, credential)` | standard library only; Go 1.18 or later, since it uses `any` |

The credential argument is named after the contract's `auth` scheme
(`token` for bearer, the `credential` name for a header scheme) and is
absent when `auth` is `none`. The Rust file header states the exact
dependency line to add. The Go package is named after the contract's
`name`.

Method names follow one pattern, cased per language:

| Operation | Pattern | Rust and Python | TypeScript | Go |
| --- | --- | --- | --- | --- |
| List | `list_{resource}` | `list_files(limit, cursor)` | `listFiles(limit?, cursor?)` | `ListFiles(limit, cursor)` |
| Get | `get_{singular}` | `get_file(id)` | `getFile(id)` | `GetFile(id)` |
| Sub-collection | `list_{sub}_{resource}` | `list_versions_files(id, limit, cursor)` | `listVersionsFiles(id, limit?, cursor?)` | `ListVersionsFiles(id, limit, cursor)` |
| Action | `{action}_{singular}` | `issue_url_file(id, input)` | `issueUrlFile(id, input)` | `IssueUrlFile(id, input)` |
| Query | `{query}` | `search(q, limit)` | `search(q, limit?)` | `Search(q, limit)` |

Hyphens in names become underscores. Validation refuses a contract in
which two operations derive the same method name. List and get methods
exist only for the faces a resource exposes.

Details that differ by method:

- An action method takes `id` when its path holds `{id}`, and the input
  object when the action declares inputs: `input: Value` in Rust,
  `input: Record<string, unknown>` in TypeScript, `body` (a dict) in
  Python, and `input map[string]any` in Go. It returns the resource type,
  open JSON, or nothing, per the action's `output`.
- A query method takes typed parameters, required ones first. Optional
  parameters are `Option<T>` in Rust, optional in TypeScript, and default
  to `None` in Python. In Go, an optional string or integer at its zero
  value is not sent. Queries return open JSON.
- An HTTP error status is an error: `reqwest`'s status error in Rust, a
  thrown `Error` naming method, path, and status in TypeScript, urllib's
  `HTTPError` in Python, and an `error` naming method, path, and status
  in Go. The error response body is not parsed.

What the clients do not cover:

- List and sub-collection methods take `limit` and `cursor` only. There
  are no filter or sort parameters.
- There are no methods for content upload or download, and none for
  subscriptions.
- In a query path, the clients substitute `{id}` only.
- The TypeScript interfaces name fields in camelCase (`createdAt`), while
  the server sends the API names (`created_at`) and the client does not
  convert keys. Any field whose API name contains an underscore is typed
  under a name the response does not carry. The Rust, Python, and Go
  clients use the API names.

The Kayak test suite checks generated client syntax with real tools where
they are installed: `py_compile` for Python, `gofmt -e` for Go, and
`rustfmt` for both Rust flavors.

## The engine policy face

A SurrealDB deployment can enforce the contract a second time in the
engine: table `PERMISSIONS` filter rows, and field `PERMISSIONS` redact
columns, for sessions authenticated as callers. `kayak::derive_policy`
renders those clauses from the contract, so tightening a scope or guarding
a field changes both enforcement layers in one edit:

```rust
let policy = kayak::derive_policy(&contract, &kayak::ClaimVocabulary::default())?;
// policy.select_conjuncts: (table, "$token.sc CONTAINS 'read'") for every
//   resource whose reads require scopes, and for its sub-resource tables,
//   since a sub-collection is read under its parent's requirement.
//   Several required scopes join with AND.
// policy.field_guards: (table, column, clause) for every guarded field,
//   on resources and sub-resources alike.
```

Which token claims those clauses read is deployment convention, so it
travels as a `ClaimVocabulary`: the claim that carries the scope list
(`scopes_claim`), and the engine clause each named guard becomes
(`guard_clauses`). The default vocabulary reads scopes from `sc` and knows
two guards:

| Guard | Clause |
| --- | --- |
| `admin_only` | `$token.adm = true` |
| `owner_or_admin` | `$token.adm = true OR created_by = $token.pr` |

A guard the contract declares that the vocabulary cannot render fails the
derivation with `PolicyError::MissingGuardClause`, naming the guard.
Rendering nothing would drop the engine layer for that column while the
application layer kept enforcing it. The `engine-policy` CLI target always
uses the default vocabulary, which is why it is opt-in. A deployment with
its own claims or guard names calls `derive_policy` with its own
vocabulary. The target writes `policy.json` as
`{"field_guards": [[table, column, clause]], "select_conjuncts": [[table, conjunct]]}`.

Two rules stay with the service. The tenancy floor (tenant-scoped tables
admit only their tenant's rows, tables without the column are closed)
must derive from the schema, or a table left out of the contract would
escape it. Delete conjuncts, such as retention rules, are policy the
contract cannot declare yet. Kayak derives only what the contract
declares.

## Keeping artifacts in sync

Check the generated artifacts into the service's repository and compare
them in a test, so a contract or schema change shows up as an artifact
diff in the same commit:

```rust
#[test]
fn generated_artifacts_are_current() {
    let contract: kayak::Contract =
        serde_json::from_str(include_str!("../api/contract.json")).unwrap();
    let schema = my_service::schema::tables();
    let artifacts =
        kayak::generate_all(&contract, &schema, kayak::generate::TARGETS).unwrap();
    for (filename, content) in &artifacts {
        let path = std::path::Path::new("api/generated").join(filename);
        if std::env::var_os("MYSERVICE_BLESS").is_some() {
            std::fs::write(&path, content).unwrap();
        }
        let checked_in = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, &checked_in, "{filename} drifted; rerun with MYSERVICE_BLESS=1");
    }
}
```

Re-blessing is an explicit step (`MYSERVICE_BLESS=1 cargo test`). Any
other drift fails the build. Pair it with `kayak diff` against the
contract on the main branch, so a breaking change fails CI unless someone
chooses to ship it.

Kayak's own golden tests work the same way, under `tests/golden/`, with
`KAYAK_BLESS` naming what to re-bless:

```sh
KAYAK_BLESS=client-go cargo test            # one golden
KAYAK_BLESS=client-go,openapi cargo test    # several
KAYAK_BLESS=all cargo test                  # every golden (1 also works)
```

The names are the target names except `engine-policy`, plus
`copal-files`, a second OpenAPI golden over a different fixture. An
unknown name fails the run.

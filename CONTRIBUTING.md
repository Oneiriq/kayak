# Contributing to Kayak

Thanks for your interest in Kayak. This page covers how to build it, run
the checks CI runs, and update the golden files the tests compare
against.

## Prerequisites

- Rust 1.90 or newer, with `rustfmt` and `clippy`.
- Optional: `python`, `gofmt`, and `rustfmt` on your `PATH`. The CLI test
  uses them to check that the generated Python, Go, and Rust clients
  parse. When a tool is missing, that check is skipped.
- On Windows, the test dependencies build `aws-lc-sys`, which needs
  [NASM](https://www.nasm.us). Install it, or set
  `AWS_LC_SYS_PREBUILT_NASM=1` to use the prebuilt objects.

## Build and test

```sh
cargo build
cargo test
cargo test --all-features
```

Plain `cargo test` builds with default features and runs the validation,
generator, differ, and CLI tests. `--all-features` adds the runtime,
GraphQL, console, and `verify` tests. The `verify` tests use an embedded
in-memory SurrealDB, so no database server is needed.

Before you open a pull request, run what CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
cargo test
```

CI runs both test builds, so a test that needs a feature has to carry a
`#[cfg(feature = "...")]` gate, or the default build fails to compile.

## Golden files

Each generator's output is compared byte for byte with a file in
`tests/golden/`. If you change a generator on purpose, the test prints
both versions. Re-bless the affected files, then read the diff before you
commit:

```sh
KAYAK_BLESS=openapi cargo test
KAYAK_BLESS=client-ts,client-py cargo test
KAYAK_BLESS=all cargo test
```

The names are `openapi`, `sdl`, `mcp`, `client-rs`, `client-rs-blocking`,
`client-ts`, `client-py`, `client-go`, and `copal-files`. An unknown name
fails the run, so a typo can't pass as a clean bless.

## Changes to the contract format

The contract is a public format that other projects check in. When you
change it:

- Keep old contracts loading. New fields need a serde default.
- Teach the differ about the new field, so removing or tightening it is
  reported as breaking.
- Add validation for anything the generators or runtime depend on.
- Update [docs/contract.md](docs/contract.md).

## Continuous integration

CI runs on pushes to `main`, on pull requests, and on demand. It runs on
a self-hosted runner and does not run automatically for pull requests
from forks, so a maintainer will run the checks for you. Please run the
commands above before you ask for a review.

## Pull requests

- Keep each pull request to one change, and describe what it changes and
  why.
- Add or update tests for behavior you change.
- Add a line to `CHANGELOG.md` for user-visible changes.

## License

By contributing, you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE) that covers this repository.

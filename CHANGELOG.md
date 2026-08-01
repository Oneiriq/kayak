# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Janus has not cut a release yet. Everything below is the road to 0.1.0.

## [Unreleased]

### Added

- **Consumption refusals in the error vocabulary.** `PayloadTooLarge` (413)
  and `TooManyRequests` (429), so a protocol face no longer downgrades an
  oversized body to a generic bad request and a metered refusal has a status a
  client can wait on.
- **Contract-declared limits.** Depth and complexity ceilings live in the
  contract, where the served GraphQL schema applies them before any resolver
  runs, the OpenAPI document carries them as `x-limits`, and the differ treats
  introducing or lowering one as a named breaking change.
- **Sub-resources.** A collection belonging to one parent instance (a file's
  versions, an endpoint's deliveries) is declared on the parent and reaches
  every face from that one declaration: `GET /v1/files/{id}/versions`, a field
  on the parent's GraphQL type, an OpenAPI path, and a method on all four
  clients. The index rulebook is shared with resources, with `parent_key`
  credited as equality-bound, and the GraphQL type name composes with the
  parent so two parents may each carry a `versions` collection.
- **Watchable resources and the subscription seam.** A resource may declare
  itself `watchable`, which adds a GraphQL Subscription field over a stream
  resolver the service registers. REST, OpenAPI, and the four generated clients
  are untouched: Janus generates no long-lived HTTP operations. Declaration and
  registration are checked in both directions at build, so a watchable resource
  without a resolver refuses by name, and so does a resolver for a resource the
  contract never opened. Watchers narrow the stream with the same `filterable`
  columns list callers use. The middleware chain runs once, at open.
- **The contract, executed.** The runtime: a resolver registry a service fills
  with its own data access, a middleware chain that is protocol-agnostic, and a
  dispatcher that enforces the contract before any resolver runs. Construction
  refuses, by name, any declared operation without a resolver.
- **The live GraphQL face.** An `async-graphql` dynamic schema built from the
  same IR the SDL generator prints, so the served schema and the checked-in
  artifact cannot disagree. `schema_builder` exposes the underlying builder for
  depth and complexity limits.
- **Generators for every surface**: OpenAPI 3.1, GraphQL SDL, and clients in
  Rust, TypeScript, Python, and Go, all from one contract object.
- **Actions**: verbs beyond list and get, with typed inputs and instance or
  collection targeting, rendered as OpenAPI operations, GraphQL mutations, and
  client methods from one declaration.
- **Validation against the real schema.** Tables and columns must exist,
  renames must not collide, filters must be indexed, and sorts must be
  reachable through an index prefix whose head is equality-bound. A sort no
  index can serve is a generation error naming the column.
- **Name gating.** Chosen names are checked against the GraphQL grammar, the
  `__` introspection prefix, root type names, cross-resource type collisions,
  and the SurrealDB v3 reserved-word list, exported as `janus::is_reserved`.
- **IR-level diffing.** `janus diff old.json new.json` classifies every change
  and exits non-zero on a breaking one, which makes the gate one line of CI.

### Fixed

- **OpenAPI list responses are the page envelope**, matching the SDL and the
  generated clients, rather than a bare array.

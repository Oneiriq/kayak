# Janus

One contract, every face. Janus is the contract layer for
SurrealDB-backed APIs: a serializable intermediate representation
authored over `surql-rs` schema definitions, validated against the
schema's real indexes at build time, and compiled into API surfaces —
OpenAPI 3.1 today; GraphQL SDL, server handlers, and typed clients on
the same IR next.

Nothing is exposed by default. A resource names its table, allowlists
and optionally renames the fields it projects, and declares which
fields may filter and sort. Declaring a sortable field with no backing
index is a **generation error naming the field** — the class of failure
that otherwise ships and becomes a production table scan.

```text
contract (builders)  ->  IR (serialized, versioned, checked in)
                              |- validate against TableDefinition/IndexDefinition
                              |- OpenAPI 3.1
                              |- GraphQL SDL          (next)
                              |- handlers + clients   (next)
```

## Status

Early. IR, validation gate, and the OpenAPI generator are real and
tested; the remaining generators arrive while Copal — the first
consumer — drives the shape.

## License

Apache-2.0.

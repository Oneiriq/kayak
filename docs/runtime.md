# The runtime

Generators compile the contract into artifacts. The runtime executes it. A
service registers resolvers (its own data access), stacks middleware around
them, and the dispatcher enforces the contract before any resolver runs. The
`graphql` feature then serves a live schema built from the same contract
object the SDL generator prints, so the served schema and the checked-in
artifact cannot disagree.

Feature flags keep codegen users free of runtime weight: `runtime` is the
dispatch core with no added dependencies; `graphql` adds the dynamic schema
on `async-graphql`.

## Resolvers

A resolver is an async closure the service registers. Janus never talks to a
database; whatever the closure calls (repositories, outside services)
is the service's business.

```rust
use janus::runtime::{Dispatcher, JanusContext, ListOutput, Resolvers};

let resolvers = Resolvers::new()
    .list("files", |ctx: JanusContext, args| async move {
        // args.limit is clamped, args.filters and args.sort are
        // allowlist-checked before this closure runs
        Ok(ListOutput { items: vec![], next_cursor: None })
    })
    .get("files", |ctx, args| async move {
        Ok(None) // None renders as null / 404
    })
    .action("files", "issue_url", |ctx, args| async move {
        // args.input is type-checked against the declared ActionFields
        Ok(Some(serde_json::json!({ "url": "..." })))
    });
```

Construction is the completeness gate. A contract that declares a resource or
action without a registered resolver refuses to build, at startup, naming the
gap:

```rust
let dispatcher = Dispatcher::new(contract.into(), resolvers, middleware)?;
```

Rows travel as `serde_json::Value` in wire shape. The dispatcher validates
before dispatch: list limits clamp to `max_page_size`, unknown filters and
undeclared sorts refuse, action inputs check against their declared types,
and unknown input keys drop (the differ promises that removing an optional
input is compatible, which only holds if servers ignore unknown fields).

## Middleware

Middleware wraps dispatch. The same chain runs whether an operation arrived
over GraphQL or anything else built on the dispatcher, so policy is written
once.

```rust
use janus::runtime::{BoxFuture, JanusError, Middleware, Next, Operation,
                     Outcome, Payload};

struct RequireTenant;

impl Middleware for RequireTenant {
    fn handle<'a>(
        &'a self,
        operation: Operation,
        ctx: JanusContext,
        payload: Payload,
        next: Next,
    ) -> BoxFuture<'a, Result<Outcome, JanusError>> {
        Box::pin(async move {
            if ctx.get::<Tenant>().is_none() {
                return Err(JanusError::Unauthorized("no tenant".into()));
            }
            next.run(operation, ctx, payload).await
        })
    }
}
```

A layer can short-circuit (return an error), enrich the context before
calling `next.run`, observe, or transform the outcome on the way back out.
Layers execute in registration order, innermost last.

`JanusContext` is a typed extension map seeded per request by the HTTP layer
(a tenant, a principal) and readable in every layer and resolver. A missing
context is an empty context, so context-requiring middleware fails closed.

Errors carry a stable vocabulary (`JanusError::BadRequest`, `Unauthorized`,
`Forbidden`, `NotFound`, `Conflict`, `Internal`) with HTTP status and
machine-readable code mappings. Protocol layers translate; resolvers never
think in protocol terms.

## The GraphQL face

```rust
let schema = janus::runtime::graphql::build_schema(&tables, dispatcher)?;
let response = schema.execute(
    async_graphql::Request::new(query).data(janus_context)
).await;
```

The schema is built dynamically from the contract: object types with
nullability from the schema definitions, sort enums listing only index-backed
orderings, page types, query fields, one mutation field per action. Every
field resolver funnels through the dispatcher, so middleware and contract
enforcement behave identically everywhere. Errors carry their code in
`extensions.code`.

`schema_builder` is the plugin seam. It returns the underlying
`async-graphql` builder before finishing, which is where depth and complexity
limits and any extensions attach:

```rust
let schema = janus::runtime::graphql::schema_builder(&tables, dispatcher)?
    .limit_depth(10)
    .limit_complexity(500)
    .finish()?;
```

Set those limits in any deployment. Without a complexity ceiling, alias
amplification multiplies one page query into thousands of resolver calls.

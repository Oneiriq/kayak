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

## Sub-resources

A declared sub-resource needs its own list resolver, checked at build like
every other declared operation:

```rust
let resolvers = resolvers.sub_list("files", "versions", |ctx, args| async move {
    // args.parent_id is the file; limit, filters, and sort are checked
    // against the SUB-resource's own declarations
    Ok(ListOutput { items: vec![], next_cursor: None })
});
```

It dispatches as its own operation (`OperationKind::SubList`), so middleware
sees the parent listing and the sub-listing separately and can authorize them
separately.

## Field guards

A guard is registered by name and decides over the context,
synchronously:

```rust
let guards = Guards::new().guard("audit_only", |ctx| {
    ctx.get::<Principal>().is_some_and(|p| p.has("audit"))
});
let dispatcher = Dispatcher::with_policies(
    contract.into(), resolvers, middleware, None, guards,
)?;
```

Synchronous on purpose: guards run per operation, and a guard that
performed IO would turn one listing into hundreds of queries.
Anything needing IO belongs in an auth middleware that resolves once
into the context, where the guard can then see it.

The gate runs in both directions: a declared guard nobody registered
refuses to build (it would silently show what it was meant to hide),
and a registered guard nothing references refuses too (dead policy
that reads as live). Visibility is evaluated once per operation, and
the hidden set both refuses filters and sorts before the resolver and
projects rows after it, streamed rows included.

## Rate limiting

The dispatcher is the one layer that can meter GraphQL accurately: a
proxy counts requests, and one POST can carry many aliased operations.
A contract that names a rate class refuses to build without a ledger:

```rust
let dispatcher = Dispatcher::with_rate_store(
    contract.into(), resolvers, middleware,
    Arc::new(MemoryRateStore::new()),
)?;
```

The ledger is charged first, before scope checks and resolvers, so a
caller past its budget learns nothing else about the request and an
unauthorized prober spends budget on its probes. Buckets key on the
principal's subject; anonymous callers share one bucket, which still
bounds what anonymity can extract.

`MemoryRateStore` meters one process over fixed one-minute windows,
which admit up to double the budget across a boundary; the tradeoff
buys a ledger with no background task. A multi-node deployment
implements [`RateStore`] over its shared database instead.

## The principal

A tenant says whose data a request touches; a [`Principal`] says what
this caller may do with it. The protocol layer or an auth middleware
seeds one:

```rust
ctx.insert(Principal::new("key-01", ["files_read".to_owned()]));
```

The dispatcher checks it against the contract's declared scopes at the
terminal: after every middleware layer, so identity resolved mid-chain
counts, and before the resolver, so guarded data is never touched on a
refusal. Declared scope with no principal refuses `unauthorized`;
a principal missing the scope refuses `forbidden`, naming it. A
contract that declares no scopes checks nothing.

## Watching

A resource the contract marks `watchable` registers a fourth kind of
resolver. It is awaited once, when a subscription opens, and returns a
stream of rows that runs until the subscriber drops it:

```rust
let resolvers = resolvers.watch("events", |ctx, args| async move {
    // args.filters is allowlist-checked, same columns list callers filter on
    Ok(Box::pin(my_live_query(ctx, args)) as janus::runtime::RowStream)
});
```

Declaration and registration are checked in both directions. A watchable
resource with no resolver refuses to build, and so does a resolver for a
resource the contract never opened, because that resolver is dead code that
reads as live.

Watchers narrow the stream with the same `filterable` columns list callers
use, so a resource has one filter vocabulary whichever operation reads it.
There is no limit or cursor: a stream is not a page.

The middleware chain runs around the opening call ONLY. Authorization
happens when the subscription starts, and the rows that follow flow from the
resolver to the subscriber without re-entering the chain. A stream that must
stop when a credential is revoked has to check that itself, per row, inside
the resolver. An `Err` item ends the subscription with that error, which is
how a resolver that loses its source should report it rather than closing
silently.

Watching has no REST shape here. Janus generates no long-lived HTTP
operations, so subscriptions appear in the SDL and the served schema and
nowhere else.

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
orderings, page types, query fields, a field on the parent for each
sub-resource, one mutation field per action, one subscription field per
watchable resource. Every field resolver funnels
through the dispatcher, so middleware and contract enforcement behave
identically everywhere. Errors carry their code in `extensions.code`.

The Mutation and Subscription roots appear only when something populates
them, so a read-only contract prints neither.

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

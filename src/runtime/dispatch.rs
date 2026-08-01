//! The dispatcher: contract enforcement in front of every resolver.
//!
//! Construction is the completeness gate: a contract that declares a
//! resource or action without a registered resolver refuses to build,
//! at startup, by name. Dispatch is the enforcement gate: arguments
//! are validated against the contract, then travel the middleware
//! chain, then reach the resolver.

use std::sync::Arc;

use crate::ir::{Action, Contract, Resource, SubResource};
use crate::runtime::args::{
    validate_action, validate_list, validate_sub_list, validate_watch, ActionArgs, GetArgs,
    ListArgs, ListOutput, SubListArgs, WatchArgs,
};
use crate::runtime::context::JanusContext;
use crate::runtime::error::JanusError;
use crate::runtime::guards::Guards;
use crate::runtime::middleware::{
    Middleware, Next, Operation, OperationKind, Outcome, Payload, Terminal,
};
use crate::runtime::principal::Principal;
use crate::runtime::rate::RateStore;
use crate::runtime::resolvers::{Resolvers, RowStream};

/// Why a runtime could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeBuildError {
    #[error("resource {0}: no list resolver registered")]
    MissingList(String),
    #[error("resource {0}: no get resolver registered")]
    MissingGet(String),
    #[error("resource {resource}: action {action}: no resolver registered")]
    MissingAction { resource: String, action: String },
    #[error("resource {resource}: sub-resource {sub}: no list resolver registered")]
    MissingSubList { resource: String, sub: String },
    #[error("resource {0}: watchable, but no watch resolver registered")]
    MissingWatch(String),
    #[error(
        "resource {0}: watch resolver registered, but the contract does not mark it watchable"
    )]
    UnwatchedResolver(String),
    #[error(
        "{scope}: rate class {class} declared, but no rate store is registered; \
         use Dispatcher::with_rate_store"
    )]
    UnmeteredRateClass { scope: String, class: String },
    #[error("{scope}: field {field} names guard {guard}, but none is registered")]
    MissingGuard {
        scope: String,
        field: String,
        guard: String,
    },
    #[error("guard {0} is registered, but no field in the contract names it")]
    UnusedGuard(String),
}

/// The executable heart of the runtime. Cheap to clone via `Arc`.
pub struct Dispatcher {
    contract: Arc<Contract>,
    resolvers: Resolvers,
    middleware: Arc<[Arc<dyn Middleware>]>,
    rate_store: Option<Arc<dyn RateStore>>,
    guards: Guards,
    /// Open subscriptions per principal subject, kept only when the
    /// contract declares a ceiling.
    watch_counts: Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
}

impl Dispatcher {
    /// Assemble a dispatcher, refusing if any declared operation lacks
    /// a resolver.
    pub fn new(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
    ) -> Result<Self, RuntimeBuildError> {
        Self::build(contract, resolvers, middleware, None, Guards::new())
    }

    /// Assemble a dispatcher with a consumption ledger. Required
    /// whenever the contract names a rate class: a declared budget
    /// with nothing keeping the ledger would be policy that silently
    /// meters nobody.
    pub fn with_rate_store(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
        rate_store: Arc<dyn RateStore>,
    ) -> Result<Self, RuntimeBuildError> {
        Self::build(
            contract,
            resolvers,
            middleware,
            Some(rate_store),
            Guards::new(),
        )
    }

    /// Assemble a dispatcher with the full policy set: a consumption
    /// ledger and field guards. Either may be empty when the contract
    /// declares nothing that needs it; the gate below refuses the
    /// mismatches by name.
    pub fn with_policies(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
        rate_store: Option<Arc<dyn RateStore>>,
        guards: Guards,
    ) -> Result<Self, RuntimeBuildError> {
        Self::build(contract, resolvers, middleware, rate_store, guards)
    }

    fn build(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
        rate_store: Option<Arc<dyn RateStore>>,
        guards: Guards,
    ) -> Result<Self, RuntimeBuildError> {
        if rate_store.is_none() {
            for resource in &contract.resources {
                if let Some(class) = &resource.rate_class {
                    return Err(RuntimeBuildError::UnmeteredRateClass {
                        scope: format!("resource {}", resource.name),
                        class: class.clone(),
                    });
                }
                for action in &resource.actions {
                    if let Some(class) = &action.rate_class {
                        return Err(RuntimeBuildError::UnmeteredRateClass {
                            scope: format!("resource {} action {}", resource.name, action.name),
                            class: class.clone(),
                        });
                    }
                }
            }
        }
        // Guards are gated in both directions, like watch resolvers.
        // A declared guard nobody registered would silently show what
        // it was meant to hide, so it refuses instead; a registered
        // guard nothing references is dead policy that reads as live.
        for resource in &contract.resources {
            let subs = resource.sub_resources.iter().flat_map(|s| {
                s.fields
                    .iter()
                    .map(move |f| (format!("resource {}.{}", resource.name, s.name), f))
            });
            let own = resource
                .fields
                .iter()
                .map(|f| (format!("resource {}", resource.name), f));
            for (scope, exposure) in own.chain(subs) {
                if let Some(guard) = &exposure.guard {
                    if !guards.map.contains_key(guard) {
                        return Err(RuntimeBuildError::MissingGuard {
                            scope,
                            field: exposure.api_name().to_owned(),
                            guard: guard.clone(),
                        });
                    }
                }
            }
        }
        for name in guards.map.keys() {
            let referenced = contract.resources.iter().any(|r| {
                r.fields.iter().any(|f| f.guard.as_deref() == Some(name))
                    || r.sub_resources
                        .iter()
                        .any(|s| s.fields.iter().any(|f| f.guard.as_deref() == Some(name)))
            });
            if !referenced {
                return Err(RuntimeBuildError::UnusedGuard(name.to_owned()));
            }
        }

        for resource in &contract.resources {
            if !resolvers.list.contains_key(&resource.name) {
                return Err(RuntimeBuildError::MissingList(resource.name.clone()));
            }
            if !resolvers.get.contains_key(&resource.name) {
                return Err(RuntimeBuildError::MissingGet(resource.name.clone()));
            }
            for sub in &resource.sub_resources {
                let key = (resource.name.clone(), sub.name.clone());
                if !resolvers.sub_list.contains_key(&key) {
                    return Err(RuntimeBuildError::MissingSubList {
                        resource: resource.name.clone(),
                        sub: sub.name.clone(),
                    });
                }
            }
            if resource.watchable && !resolvers.watch.contains_key(&resource.name) {
                return Err(RuntimeBuildError::MissingWatch(resource.name.clone()));
            }
            for action in &resource.actions {
                let key = (resource.name.clone(), action.name.clone());
                if !resolvers.action.contains_key(&key) {
                    return Err(RuntimeBuildError::MissingAction {
                        resource: resource.name.clone(),
                        action: action.name.clone(),
                    });
                }
            }
        }
        // The converse, which only watching can get wrong: list and get
        // are always declared, so a stray resolver for them is
        // impossible. A watch resolver on a resource nobody may watch is
        // dead code that reads as live.
        for name in resolvers.watch.keys() {
            if !contract
                .resources
                .iter()
                .any(|r| &r.name == name && r.watchable)
            {
                return Err(RuntimeBuildError::UnwatchedResolver(name.clone()));
            }
        }
        Ok(Self {
            contract,
            resolvers,
            middleware: middleware.into(),
            rate_store,
            guards,
            watch_counts: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    /// The contract this dispatcher enforces.
    pub fn contract(&self) -> &Arc<Contract> {
        &self.contract
    }

    fn resource(&self, name: &str) -> Result<&Resource, JanusError> {
        self.contract
            .resources
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| JanusError::BadRequest(format!("unknown resource {name}")))
    }

    fn action_of<'a>(resource: &'a Resource, name: &str) -> Result<&'a Action, JanusError> {
        resource
            .actions
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| {
                JanusError::BadRequest(format!(
                    "unknown action {name} on resource {}",
                    resource.name,
                ))
            })
    }

    fn chain(&self) -> Next {
        let resolvers = self.resolvers.clone();
        let contract = self.contract.clone();
        let rate_store = self.rate_store.clone();
        let guards = self.guards.clone();
        let watch_counts = self.watch_counts.clone();
        let terminal: Terminal = Arc::new(move |operation: Operation, ctx, payload| {
            let resolvers = resolvers.clone();
            let contract = contract.clone();
            let rate_store = rate_store.clone();
            let guards = guards.clone();
            let watch_counts = watch_counts.clone();
            Box::pin(async move {
                // The ledger is charged first: a caller past its
                // budget learns nothing else about the request, and an
                // unauthorized prober spends budget on its probes.
                charge_rate(&contract, &operation, &ctx, rate_store.as_deref(), &payload).await?;
                // Scopes are checked here, after the whole middleware
                // chain, so an auth layer that resolves the principal
                // mid-chain still counts, and checked before the
                // resolver, so no guarded data is touched on a refusal.
                enforce_scopes(&contract, &operation, &ctx)?;
                // Guard evaluation happens once per operation: which
                // fields this caller sees does not vary by row. The
                // hidden set refuses filters and sorts BEFORE the
                // resolver, because narrowing by a value is reading
                // it, and projects rows AFTER, so a guarded value
                // cannot leave through any face.
                let hidden = hidden_fields(&contract, &operation, &ctx, &guards);
                refuse_hidden_narrowing(&hidden, &payload)?;
                let outcome = match (payload, operation.kind) {
                    (Payload::List(args), OperationKind::List) => {
                        let resolver = resolvers
                            .list
                            .get(&operation.resource)
                            .expect("completeness-checked at build")
                            .clone();
                        resolver(ctx, args).await.map(Outcome::List)
                    }
                    (Payload::Get(args), OperationKind::Get) => {
                        let resolver = resolvers
                            .get
                            .get(&operation.resource)
                            .expect("completeness-checked at build")
                            .clone();
                        resolver(ctx, args).await.map(Outcome::Get)
                    }
                    (Payload::Action(args), OperationKind::Action) => {
                        let action = operation
                            .action
                            .clone()
                            .expect("action operations carry the action name");
                        let resolver = resolvers
                            .action
                            .get(&(operation.resource.clone(), action))
                            .expect("completeness-checked at build")
                            .clone();
                        resolver(ctx, args).await.map(Outcome::Action)
                    }
                    (Payload::SubList(args), OperationKind::SubList) => {
                        let sub = operation
                            .sub
                            .clone()
                            .expect("sub-list operations carry the sub-resource name");
                        let resolver = resolvers
                            .sub_list
                            .get(&(operation.resource.clone(), sub))
                            .expect("completeness-checked at build")
                            .clone();
                        resolver(ctx, args).await.map(Outcome::List)
                    }
                    (Payload::Watch(args), OperationKind::Watch) => {
                        let resolver = resolvers
                            .watch
                            .get(&operation.resource)
                            .expect("completeness-checked at build")
                            .clone();
                        // The slot is taken before the resolver runs,
                        // so a refused open never starts a live query,
                        // and it rides the stream so dropping the
                        // subscription frees it.
                        let slot = acquire_watch_slot(&contract, &ctx, &watch_counts)?;
                        match resolver(ctx, args).await {
                            Ok(stream) => Ok(Outcome::Watch(match slot {
                                Some(slot) => Box::pin(SlottedStream {
                                    inner: stream,
                                    _slot: slot,
                                }),
                                None => stream,
                            })),
                            Err(error) => Err(error),
                        }
                    }
                    _ => Err(JanusError::Internal(
                        "payload does not match operation kind".into(),
                    )),
                }?;
                Ok(project_hidden(outcome, hidden))
            })
        });
        Next {
            chain: self.middleware.clone(),
            index: 0,
            terminal,
        }
    }

    /// Dispatch a list operation.
    pub async fn list(
        &self,
        resource: &str,
        ctx: JanusContext,
        mut args: ListArgs,
    ) -> Result<ListOutput, JanusError> {
        validate_list(self.resource(resource)?, &mut args)?;
        let operation = Operation {
            resource: resource.to_owned(),
            kind: OperationKind::List,
            action: None,
            sub: None,
        };
        match self
            .chain()
            .run(operation, ctx, Payload::List(args))
            .await?
        {
            Outcome::List(output) => Ok(output),
            _ => Err(JanusError::Internal(
                "resolver returned a mismatched outcome".into(),
            )),
        }
    }

    /// Dispatch a get operation.
    pub async fn get(
        &self,
        resource: &str,
        ctx: JanusContext,
        args: GetArgs,
    ) -> Result<Option<serde_json::Value>, JanusError> {
        self.resource(resource)?;
        let operation = Operation {
            resource: resource.to_owned(),
            kind: OperationKind::Get,
            action: None,
            sub: None,
        };
        match self.chain().run(operation, ctx, Payload::Get(args)).await? {
            Outcome::Get(row) => Ok(row),
            _ => Err(JanusError::Internal(
                "resolver returned a mismatched outcome".into(),
            )),
        }
    }

    /// Dispatch an action.
    pub async fn action(
        &self,
        resource: &str,
        action: &str,
        ctx: JanusContext,
        mut args: ActionArgs,
    ) -> Result<Option<serde_json::Value>, JanusError> {
        let declared = Self::action_of(self.resource(resource)?, action)?;
        validate_action(declared, &mut args)?;
        let operation = Operation {
            resource: resource.to_owned(),
            kind: OperationKind::Action,
            action: Some(action.to_owned()),
            sub: None,
        };
        match self
            .chain()
            .run(operation, ctx, Payload::Action(args))
            .await?
        {
            Outcome::Action(value) => Ok(value),
            _ => Err(JanusError::Internal(
                "resolver returned a mismatched outcome".into(),
            )),
        }
    }

    fn sub_of<'a>(resource: &'a Resource, name: &str) -> Result<&'a SubResource, JanusError> {
        resource
            .sub_resources
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| {
                JanusError::BadRequest(format!(
                    "unknown sub-resource {name} on resource {}",
                    resource.name,
                ))
            })
    }

    /// Dispatch a sub-resource listing: one parent instance's
    /// collection, validated against that collection's own
    /// declarations rather than the parent's.
    pub async fn sub_list(
        &self,
        resource: &str,
        sub: &str,
        ctx: JanusContext,
        mut args: SubListArgs,
    ) -> Result<ListOutput, JanusError> {
        let declared = Self::sub_of(self.resource(resource)?, sub)?;
        validate_sub_list(declared, &mut args)?;
        let operation = Operation {
            resource: resource.to_owned(),
            kind: OperationKind::SubList,
            action: None,
            sub: Some(sub.to_owned()),
        };
        match self
            .chain()
            .run(operation, ctx, Payload::SubList(args))
            .await?
        {
            Outcome::List(output) => Ok(output),
            _ => Err(JanusError::Internal(
                "resolver returned a mismatched outcome".into(),
            )),
        }
    }

    /// Open a subscription. The chain runs once, here; the rows that
    /// follow flow straight from the resolver to the subscriber.
    pub async fn watch(
        &self,
        resource: &str,
        ctx: JanusContext,
        args: WatchArgs,
    ) -> Result<RowStream, JanusError> {
        validate_watch(self.resource(resource)?, &args)?;
        let operation = Operation {
            resource: resource.to_owned(),
            kind: OperationKind::Watch,
            action: None,
            sub: None,
        };
        match self
            .chain()
            .run(operation, ctx, Payload::Watch(args))
            .await?
        {
            Outcome::Watch(stream) => Ok(stream),
            _ => Err(JanusError::Internal(
                "resolver returned a mismatched outcome".into(),
            )),
        }
    }
}

/// One held subscription slot; dropping it frees the count, whether
/// the stream ended, errored, or the client walked away.
struct WatchSlot {
    counts: Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
    subject: String,
}

impl Drop for WatchSlot {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.counts.lock() {
            if let Some(open) = counts.get_mut(&self.subject) {
                *open = open.saturating_sub(1);
                if *open == 0 {
                    counts.remove(&self.subject);
                }
            }
        }
    }
}

/// A row stream carrying its slot, so the count and the subscription
/// share a lifetime exactly.
struct SlottedStream {
    inner: crate::runtime::resolvers::RowStream,
    _slot: WatchSlot,
}

impl futures_core::Stream for SlottedStream {
    type Item = Result<serde_json::Value, JanusError>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_next(cx)
    }
}

/// Take a subscription slot for this caller, when the contract caps
/// them. Over the ceiling refuses with the retryable code: closing a
/// subscription is what frees a slot.
fn acquire_watch_slot(
    contract: &Contract,
    ctx: &JanusContext,
    counts: &Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
) -> Result<Option<WatchSlot>, JanusError> {
    let Some(ceiling) = contract.limits.and_then(|l| l.max_watches_per_principal) else {
        return Ok(None);
    };
    let subject = ctx
        .get::<Principal>()
        .map(|p| p.subject.clone())
        .unwrap_or_else(|| "anonymous".to_owned());
    let mut counts_map = counts
        .lock()
        .map_err(|_| JanusError::Internal("watch ledger poisoned".into()))?;
    let open = counts_map.entry(subject.clone()).or_insert(0);
    if *open >= ceiling {
        return Err(JanusError::TooManyRequests(format!(
            "watch ceiling of {ceiling} reached; close a subscription to open another",
        )));
    }
    *open += 1;
    drop(counts_map);
    Ok(Some(WatchSlot {
        counts: counts.clone(),
        subject,
    }))
}

/// The wire names and columns this caller may NOT see for the
/// operation's row shape, evaluated once per operation.
fn hidden_fields(
    contract: &Contract,
    operation: &Operation,
    ctx: &JanusContext,
    guards: &Guards,
) -> Vec<(String, String)> {
    let Some(resource) = contract
        .resources
        .iter()
        .find(|r| r.name == operation.resource)
    else {
        return Vec::new();
    };
    let fields: &[crate::ir::FieldExposure] = match operation.kind {
        OperationKind::SubList => operation
            .sub
            .as_deref()
            .and_then(|name| resource.sub_resources.iter().find(|s| s.name == name))
            .map(|s| s.fields.as_slice())
            .unwrap_or(&[]),
        // An action that returns the resource row shares the
        // resource's shape, so it shares its projection. A Json
        // action is free-form: its keys are not the resource's, so a
        // coincidental name must not be stripped.
        OperationKind::Action => {
            let returns_resource = operation
                .action
                .as_deref()
                .and_then(|name| resource.actions.iter().find(|a| a.name == name))
                .is_some_and(|a| a.output == crate::ir::ActionOutput::Resource);
            if returns_resource {
                &resource.fields
            } else {
                &[]
            }
        }
        _ => &resource.fields,
    };
    crate::runtime::guards::hidden_in(fields, guards, ctx)
        .into_iter()
        .map(|hidden| (hidden.api_name, hidden.column))
        .collect()
}

/// Refuse filters and sorts over columns the caller cannot see.
/// Narrowing by a value is reading it: a caller filtering a hidden
/// column to a guessed value would learn the value from which rows
/// come back.
fn refuse_hidden_narrowing(
    hidden: &[(String, String)],
    payload: &Payload,
) -> Result<(), JanusError> {
    if hidden.is_empty() {
        return Ok(());
    }
    let hidden_column = |column: &str| hidden.iter().any(|(_, c)| c == column);
    let (filters, sort) = match payload {
        Payload::List(args) => (&args.filters, &args.sort),
        Payload::SubList(args) => (&args.filters, &args.sort),
        Payload::Watch(args) => (&args.filters, &None),
        _ => return Ok(()),
    };
    for column in filters.keys() {
        if hidden_column(column) {
            return Err(JanusError::Forbidden(format!(
                "filtering on {column} requires permission to see it",
            )));
        }
    }
    if let Some((column, _)) = sort {
        if hidden_column(column) {
            return Err(JanusError::Forbidden(format!(
                "sorting on {column} requires permission to see it",
            )));
        }
    }
    Ok(())
}

/// Strip the hidden fields from whatever the resolver returned. Rows
/// omit the keys rather than nulling them, so a redacted value and a
/// stored null are distinguishable to the service and identical on
/// wire faces that render omissions as null.
fn project_hidden(outcome: Outcome, hidden: Vec<(String, String)>) -> Outcome {
    if hidden.is_empty() {
        return outcome;
    }
    let strip = move |row: &mut serde_json::Value| {
        if let Some(object) = row.as_object_mut() {
            for (api_name, _) in &hidden {
                object.remove(api_name);
            }
        }
    };
    match outcome {
        Outcome::List(mut output) => {
            for row in &mut output.items {
                strip(row);
            }
            Outcome::List(output)
        }
        Outcome::Get(mut row) => {
            if let Some(row) = row.as_mut() {
                strip(row);
            }
            Outcome::Get(row)
        }
        Outcome::Action(mut value) => {
            if let Some(value) = value.as_mut() {
                strip(value);
            }
            Outcome::Action(value)
        }
        Outcome::Watch(stream) => {
            use futures_core::Stream;
            use std::pin::Pin;
            use std::task::{Context, Poll};

            struct Projected {
                inner: crate::runtime::resolvers::RowStream,
                strip: Box<dyn FnMut(&mut serde_json::Value) + Send>,
            }
            impl Stream for Projected {
                type Item = Result<serde_json::Value, JanusError>;
                fn poll_next(
                    self: Pin<&mut Self>,
                    cx: &mut Context<'_>,
                ) -> Poll<Option<Self::Item>> {
                    let this = self.get_mut();
                    match Pin::new(&mut this.inner).poll_next(cx) {
                        Poll::Ready(Some(Ok(mut row))) => {
                            (this.strip)(&mut row);
                            Poll::Ready(Some(Ok(row)))
                        }
                        other => other,
                    }
                }
            }
            Outcome::Watch(Box::pin(Projected {
                inner: stream,
                strip: Box::new(strip),
            }))
        }
    }
}

/// Charge the operation's cost against its declared rate class, when
/// one is declared and a ledger is present. Listings cost their
/// clamped row limit; everything else costs one, which is the
/// proportionality a per-request count cannot express.
async fn charge_rate(
    contract: &Contract,
    operation: &Operation,
    ctx: &JanusContext,
    store: Option<&dyn RateStore>,
    payload: &Payload,
) -> Result<(), JanusError> {
    let Some(store) = store else { return Ok(()) };
    let Some(resource) = contract
        .resources
        .iter()
        .find(|r| r.name == operation.resource)
    else {
        return Ok(());
    };
    let class_name = match operation.kind {
        OperationKind::List
        | OperationKind::Get
        | OperationKind::SubList
        | OperationKind::Watch => resource.rate_class.as_deref(),
        OperationKind::Action => operation
            .action
            .as_deref()
            .and_then(|name| resource.actions.iter().find(|a| a.name == name))
            .and_then(|a| a.rate_class.as_deref()),
    };
    let Some(class_name) = class_name else {
        return Ok(());
    };
    let Some(class) = contract.rate_classes.iter().find(|c| c.name == class_name) else {
        // Validation refuses this shape at generation; a runtime that
        // reaches it anyway fails closed rather than metering nobody.
        return Err(JanusError::Internal(format!(
            "rate class {class_name} is not defined",
        )));
    };
    let units = match payload {
        Payload::List(args) => u64::from(args.limit).max(1),
        Payload::SubList(args) => u64::from(args.limit).max(1),
        Payload::Get(_) | Payload::Action(_) | Payload::Watch(_) => 1,
    };
    // Anonymous callers share one bucket by design: without an
    // identity there is nothing fairer to key on, and the shared
    // bucket still bounds what anonymity can extract.
    let subject = ctx
        .get::<Principal>()
        .map(|p| p.subject.as_str())
        .unwrap_or("anonymous");
    let bucket = format!("{class_name}:{subject}");
    if store.charge(&bucket, units, class.units_per_minute).await? {
        Ok(())
    } else {
        Err(JanusError::TooManyRequests(format!(
            "rate class {class_name} exhausted; retry next minute",
        )))
    }
}

/// The scopes `operation` demands, per the contract: reads check the
/// resource's requirement, actions their own.
fn required_scopes<'a>(contract: &'a Contract, operation: &Operation) -> &'a [String] {
    let Some(resource) = contract
        .resources
        .iter()
        .find(|r| r.name == operation.resource)
    else {
        return &[];
    };
    match operation.kind {
        OperationKind::List
        | OperationKind::Get
        | OperationKind::SubList
        | OperationKind::Watch => &resource.reads_require,
        OperationKind::Action => operation
            .action
            .as_deref()
            .and_then(|name| resource.actions.iter().find(|a| a.name == name))
            .map(|a| a.requires.as_slice())
            .unwrap_or(&[]),
    }
}

/// Refuse an operation whose declared scopes the caller does not hold.
/// No declaration checks nothing; an anonymous caller against a
/// declared scope is `Unauthorized`; an identified caller missing one
/// is `Forbidden`, naming the scope.
fn enforce_scopes(
    contract: &Contract,
    operation: &Operation,
    ctx: &JanusContext,
) -> Result<(), JanusError> {
    let required = required_scopes(contract, operation);
    if required.is_empty() {
        return Ok(());
    }
    let Some(principal) = ctx.get::<Principal>() else {
        return Err(JanusError::Unauthorized(format!(
            "scope {} requires an identified caller",
            required.join(", "),
        )));
    };
    for scope in required {
        if !principal.has(scope) {
            return Err(JanusError::Forbidden(format!("scope {scope} required")));
        }
    }
    Ok(())
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("contract", &self.contract.name)
            .field("middleware", &self.middleware.len())
            .finish()
    }
}

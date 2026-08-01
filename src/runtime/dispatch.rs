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
}

/// The executable heart of the runtime. Cheap to clone via `Arc`.
pub struct Dispatcher {
    contract: Arc<Contract>,
    resolvers: Resolvers,
    middleware: Arc<[Arc<dyn Middleware>]>,
    rate_store: Option<Arc<dyn RateStore>>,
}

impl Dispatcher {
    /// Assemble a dispatcher, refusing if any declared operation lacks
    /// a resolver.
    pub fn new(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
    ) -> Result<Self, RuntimeBuildError> {
        Self::build(contract, resolvers, middleware, None)
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
        Self::build(contract, resolvers, middleware, Some(rate_store))
    }

    fn build(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
        rate_store: Option<Arc<dyn RateStore>>,
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
        let terminal: Terminal = Arc::new(move |operation: Operation, ctx, payload| {
            let resolvers = resolvers.clone();
            let contract = contract.clone();
            let rate_store = rate_store.clone();
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
                match (payload, operation.kind) {
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
                        resolver(ctx, args).await.map(Outcome::Watch)
                    }
                    _ => Err(JanusError::Internal(
                        "payload does not match operation kind".into(),
                    )),
                }
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

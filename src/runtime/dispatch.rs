//! The dispatcher: contract enforcement in front of every resolver.
//!
//! Construction is the completeness gate: a contract that declares a
//! resource or action without a registered resolver refuses to build,
//! at startup, by name. Dispatch is the enforcement gate: arguments
//! are validated against the contract, then travel the middleware
//! chain, then reach the resolver.

use std::sync::Arc;

use crate::ir::{Action, Contract, Resource};
use crate::runtime::args::{
    validate_action, validate_list, ActionArgs, GetArgs, ListArgs, ListOutput,
};
use crate::runtime::context::JanusContext;
use crate::runtime::error::JanusError;
use crate::runtime::middleware::{
    Middleware, Next, Operation, OperationKind, Outcome, Payload, Terminal,
};
use crate::runtime::resolvers::Resolvers;

/// Why a runtime could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeBuildError {
    #[error("resource {0}: no list resolver registered")]
    MissingList(String),
    #[error("resource {0}: no get resolver registered")]
    MissingGet(String),
    #[error("resource {resource}: action {action}: no resolver registered")]
    MissingAction { resource: String, action: String },
}

/// The executable heart of the runtime. Cheap to clone via `Arc`.
pub struct Dispatcher {
    contract: Arc<Contract>,
    resolvers: Resolvers,
    middleware: Arc<[Arc<dyn Middleware>]>,
}

impl Dispatcher {
    /// Assemble a dispatcher, refusing if any declared operation lacks
    /// a resolver.
    pub fn new(
        contract: Arc<Contract>,
        resolvers: Resolvers,
        middleware: Vec<Arc<dyn Middleware>>,
    ) -> Result<Self, RuntimeBuildError> {
        for resource in &contract.resources {
            if !resolvers.list.contains_key(&resource.name) {
                return Err(RuntimeBuildError::MissingList(resource.name.clone()));
            }
            if !resolvers.get.contains_key(&resource.name) {
                return Err(RuntimeBuildError::MissingGet(resource.name.clone()));
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
        Ok(Self {
            contract,
            resolvers,
            middleware: middleware.into(),
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
        let terminal: Terminal = Arc::new(move |operation: Operation, ctx, payload| {
            let resolvers = resolvers.clone();
            Box::pin(async move {
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
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("contract", &self.contract.name)
            .field("middleware", &self.middleware.len())
            .finish()
    }
}

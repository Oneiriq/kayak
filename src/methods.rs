//! The method names the generated clients hang on one `Client` type.
//!
//! Every client language puts every operation on a single client:
//! listings, gets, sub-listings, actions, and top-level queries all
//! become methods side by side. So the names have to be unique across
//! the whole contract, not merely within a resource -- and nothing
//! checked that. An action named `get` on a `files` resource derives
//! `get_file`, which is exactly the name the generated getter already
//! took, and four clients would emit a duplicate method that does not
//! compile.
//!
//! The list lives here, derived through the same helpers the
//! generators use, so the validator refuses what the generators would
//! have emitted rather than approximating it.

use crate::ir::Contract;
use crate::naming::{singular, snake};

/// A method the clients will generate, and the contract element that
/// asked for it -- so a collision can name both sides.
pub(crate) struct Method {
    pub name: String,
    pub source: String,
}

/// Every method name the generated clients will carry, in the order
/// the generators emit them.
pub(crate) fn client_methods(contract: &Contract) -> Vec<Method> {
    let mut methods = Vec::new();
    for resource in &contract.resources {
        let one = snake(&singular(&resource.name));
        // Only the faces the resource actually exposes, or a resource
        // with no listing would reserve `list_x` against an action
        // that is free to take it.
        if resource.faces.list {
            methods.push(Method {
                name: format!("list_{}", snake(&resource.name)),
                source: format!("the {} listing", resource.name),
            });
        }
        if resource.faces.get {
            methods.push(Method {
                name: format!("get_{one}"),
                source: format!("the {} getter", resource.name),
            });
        }
        for sub in &resource.sub_resources {
            methods.push(Method {
                // Mirrors clients::sub_method_stem.
                name: format!("list_{}_{}", snake(&sub.name), snake(&resource.name)),
                source: format!("sub-resource {} of {}", sub.name, resource.name),
            });
        }
        for action in &resource.actions {
            methods.push(Method {
                name: format!("{}_{one}", snake(&action.name)),
                source: format!("action {} on {}", action.name, resource.name),
            });
        }
    }
    for query in &contract.queries {
        methods.push(Method {
            name: snake(&query.name),
            source: format!("query {}", query.name),
        });
    }
    methods
}

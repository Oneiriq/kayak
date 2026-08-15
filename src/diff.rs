//! IR-level breaking-change detection.
//!
//! Diffing at the IR level catches changes the rendered documents
//! hide: a filter removed from an allowlist, a sort claim
//! dropped, an action's method changed, a required input added. The
//! rule of thumb: anything a deployed client could be relying on is
//! breaking; pure additions are compatible. Field guards are the one
//! deliberate exception to reading the rule caller-side only: a guard
//! moving in ANY direction is breaking, because who sees a field is
//! surface for whoever the guard protects even when no caller loses
//! a thing.

use crate::ir::{Action, ActionField, Contract, FieldExposure, Resource, SearchBacking};

/// One observed change between two contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A deployed client could break.
    Breaking(String),
    /// Additive or otherwise safe.
    Compatible(String),
}

impl Change {
    pub fn is_breaking(&self) -> bool {
        matches!(self, Self::Breaking(_))
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Breaking(m) | Self::Compatible(m) => m,
        }
    }
}

/// How a diff message names an authentication scheme: the header, when
/// there is one, because that is the part a caller has to change.
fn auth_word(auth: &crate::ir::AuthScheme) -> String {
    match auth {
        crate::ir::AuthScheme::None => "none".to_owned(),
        crate::ir::AuthScheme::Bearer => "bearer token".to_owned(),
        crate::ir::AuthScheme::Header { name, .. } => format!("the {name} header"),
    }
}

/// Compare `old` to `new`, returning every observed change.
pub fn diff(old: &Contract, new: &Contract) -> Vec<Change> {
    let mut changes = Vec::new();

    // How a caller authenticates, and every direction of moving it is
    // breaking. Changing the scheme leaves every deployed client sending
    // a credential the service no longer reads; ADDING one to an open API
    // locks out callers that sent nothing; and REMOVING one is breaking in
    // the sense field guards are — the API stops refusing what it used to
    // refuse, which is surface for whoever the credential kept out, even
    // though no caller loses a call.
    if old.auth != new.auth {
        changes.push(Change::Breaking(format!(
            "authentication changed: {} -> {}",
            auth_word(&old.auth),
            auth_word(&new.auth),
        )));
    }

    // The prefix moves every resource route at once, so changing it is
    // the broadest break a contract can express: every deployed client
    // calls a path that is no longer served. Queries keep their own
    // absolute paths and are unaffected, which is why this is stated as
    // being about the resource faces rather than about the API.
    if old.prefix() != new.prefix() {
        changes.push(Change::Breaking(format!(
            "the resource path prefix changed: {:?} -> {:?}; every resource route moves",
            old.prefix(),
            new.prefix(),
        )));
    }

    // Rate classes: shrinking a budget refuses callers that used to
    // pass; growing one refuses nothing. A class appearing or leaving
    // matters only through the operations that reference it, which the
    // per-resource rules below catch.
    for new_class in &new.rate_classes {
        if let Some(old_class) = old.rate_classes.iter().find(|c| c.name == new_class.name) {
            if new_class.units_per_minute < old_class.units_per_minute {
                changes.push(Change::Breaking(format!(
                    "rate class {}: budget lowered {} -> {}",
                    new_class.name, old_class.units_per_minute, new_class.units_per_minute,
                )));
            } else if new_class.units_per_minute > old_class.units_per_minute {
                changes.push(Change::Compatible(format!(
                    "rate class {}: budget raised {} -> {}",
                    new_class.name, old_class.units_per_minute, new_class.units_per_minute,
                )));
            }
        }
    }

    // Ceilings: introducing or lowering one refuses operations that
    // used to run. Raising or removing one refuses nothing.
    let old_limits = old.limits.unwrap_or_default();
    let new_limits = new.limits.unwrap_or_default();
    for (label, before, after) in [
        ("max_depth", old_limits.max_depth, new_limits.max_depth),
        (
            "max_complexity",
            old_limits.max_complexity,
            new_limits.max_complexity,
        ),
        (
            "max_watches_per_principal",
            old_limits.max_watches_per_principal,
            new_limits.max_watches_per_principal,
        ),
    ] {
        match (before, after) {
            (None, Some(introduced)) => changes.push(Change::Breaking(format!(
                "limits: {label} introduced at {introduced}",
            ))),
            (Some(b), Some(a)) if a < b => changes.push(Change::Breaking(format!(
                "limits: {label} lowered {b} -> {a}",
            ))),
            (Some(b), Some(a)) if a > b => changes.push(Change::Compatible(format!(
                "limits: {label} raised {b} -> {a}",
            ))),
            (Some(removed), None) => changes.push(Change::Compatible(format!(
                "limits: {label} removed (was {removed})",
            ))),
            _ => {}
        }
    }

    for old_resource in &old.resources {
        match new.resources.iter().find(|r| r.name == old_resource.name) {
            None => changes.push(Change::Breaking(format!(
                "resource {} removed",
                old_resource.name,
            ))),
            Some(new_resource) => diff_resource(old_resource, new_resource, &mut changes),
        }
    }
    for new_resource in &new.resources {
        if !old.resources.iter().any(|r| r.name == new_resource.name) {
            changes.push(Change::Compatible(format!(
                "resource {} added",
                new_resource.name,
            )));
        }
    }
    // Queries answer the same rules actions do: removal and renaming
    // break, tightening what a caller must hold breaks, and gaining a
    // required input breaks.
    for old_query in &old.queries {
        match new.queries.iter().find(|q| q.name == old_query.name) {
            None => changes.push(Change::Breaking(format!(
                "query {} removed",
                old_query.name,
            ))),
            Some(new_query) => diff_query(old_query, new_query, &mut changes),
        }
    }
    for new_query in &new.queries {
        if !old.queries.iter().any(|q| q.name == new_query.name) {
            changes.push(Change::Compatible(format!(
                "query {} added",
                new_query.name,
            )));
        }
    }

    changes
}

fn diff_query(old: &crate::ir::Query, new: &crate::ir::Query, changes: &mut Vec<Change>) {
    let name = &old.name;
    if old.graphql_field_name() != new.graphql_field_name() {
        changes.push(Change::Breaking(format!(
            "query {name} graphql field renamed {} -> {}",
            old.graphql_field_name(),
            new.graphql_field_name(),
        )));
    }
    if old.path != new.path {
        changes.push(Change::Breaking(format!(
            "query {name} moved ({} -> {})",
            old.path, new.path,
        )));
    }
    match (&old.rate_class, &new.rate_class) {
        (None, Some(class)) => changes.push(Change::Breaking(format!(
            "query {name} now metered by rate class {class}",
        ))),
        (Some(class), None) => changes.push(Change::Compatible(format!(
            "query {name} no longer metered (was {class})",
        ))),
        _ => {}
    }
    for required in &new.requires {
        if !old.requires.contains(required) {
            changes.push(Change::Breaking(format!(
                "query {name} now requires scope {required}",
            )));
        }
    }
    for required in &old.requires {
        if !new.requires.contains(required) {
            changes.push(Change::Compatible(format!(
                "query {name} no longer requires scope {required}",
            )));
        }
    }
    for field in &new.input {
        match old.input.iter().find(|f| f.name == field.name) {
            None if field.required => changes.push(Change::Breaking(format!(
                "query {name} gained required input {}",
                field.name,
            ))),
            None => changes.push(Change::Compatible(format!(
                "query {name} gained optional input {}",
                field.name,
            ))),
            Some(previous) => {
                if !previous.required && field.required {
                    changes.push(Change::Breaking(format!(
                        "query {name} input {} became required",
                        field.name,
                    )));
                }
                if previous.kind != field.kind {
                    changes.push(Change::Breaking(format!(
                        "query {name} input {} changed type",
                        field.name,
                    )));
                }
                diff_options(&format!("query {name}"), previous, field, changes);
            }
        }
    }
    for field in &old.input {
        if !new.input.iter().any(|f| f.name == field.name) {
            changes.push(Change::Breaking(format!(
                "query {name} lost input {}",
                field.name,
            )));
        }
    }
    // What the query says it does. A declared search that goes away
    // takes a capability from everyone who was calling for it, and the
    // resolver behind it degrades to whatever it degrades to; one that
    // appears promises more over the same wire surface.
    for kind in &old.searches {
        if !new.searches.contains(kind) {
            changes.push(Change::Breaking(format!(
                "query {name} no longer performs a {kind} search",
            )));
        }
    }
    for kind in &new.searches {
        if !old.searches.contains(kind) {
            changes.push(Change::Compatible(format!(
                "query {name} now performs a {kind} search",
            )));
        }
    }
    // Backings: a promise about what answers the query, on the same
    // wire surface either way. Losing one is breaking — a caller (or
    // an operator's capacity plan) relying on indexed search is handed
    // whatever the resolver degrades to. A backing has no name of its
    // own, so WHERE the machinery is (table, column, index, kind) is
    // its identity, and re-pointing any of the four reads as the old
    // promise gone (breaking) and a new one made (compatible), which
    // is the honest description of what happened. What it promises
    // ABOUT that machinery — the width, and whether the deployment is
    // required to have it — moves under a fixed identity, so those
    // read as one change each rather than a loss and a gain; a
    // backing that merely became guaranteed is not a backing anyone
    // lost.
    for backing in &old.backing {
        let Some(now) = new
            .backing
            .iter()
            .find(|b| b.machinery() == backing.machinery())
        else {
            changes.push(Change::Breaking(format!(
                "query {name} lost the {} backing {}.{} via {}",
                backing.kind, backing.table, backing.column, backing.index,
            )));
            continue;
        };
        diff_backing_promise(name, backing, now, changes);
    }
    for backing in &new.backing {
        if !old
            .backing
            .iter()
            .any(|b| b.machinery() == backing.machinery())
        {
            changes.push(Change::Compatible(format!(
                "query {name} gained the {} backing {}.{} via {}",
                backing.kind, backing.table, backing.column, backing.index,
            )));
        }
    }
}

/// How one backing's promise moved while pointing at the same
/// machinery.
///
/// A width that changes is the embedding model changing underneath,
/// which is not a slower search but a different one; a width that
/// appears tightens a claim the contract was not making; a width that
/// disappears gives up a guarantee. Optional runs the same way: a
/// backing the deployment may now skip is a backing callers can no
/// longer count on, and one that stopped being skippable is a promise
/// strengthened.
fn diff_backing_promise(
    name: &str,
    old: &SearchBacking,
    new: &SearchBacking,
    changes: &mut Vec<Change>,
) {
    let what = format!("{} backing {}.{}", old.kind, old.table, old.column);
    match (old.dimension, new.dimension) {
        (Some(before), Some(after)) if before != after => changes.push(Change::Breaking(format!(
            "query {name}: the {what} now searches at {after} dimensions (was {before})",
        ))),
        (Some(before), None) => changes.push(Change::Breaking(format!(
            "query {name}: the {what} no longer pins its width (was {before})",
        ))),
        (None, Some(after)) => changes.push(Change::Compatible(format!(
            "query {name}: the {what} now pins its width at {after}",
        ))),
        _ => {}
    }
    match (old.optional, new.optional) {
        (false, true) => changes.push(Change::Breaking(format!(
            "query {name}: the {what} is now optional, and a deployment may not have it",
        ))),
        (true, false) => changes.push(Change::Compatible(format!(
            "query {name}: the {what} is now required of every deployment",
        ))),
        _ => {}
    }
}

/// How a field's closed set moved.
///
/// Taking a value away refuses callers who were sending it, and so
/// does introducing a set where anything used to pass. Adding a value
/// accepts more than before, which breaks nobody.
fn diff_options(scope: &str, old: &ActionField, new: &ActionField, changes: &mut Vec<Change>) {
    if old.options.is_empty() && !new.options.is_empty() {
        changes.push(Change::Breaking(format!(
            "{scope}: input {} now takes only {}",
            new.name,
            new.options.join(", "),
        )));
        return;
    }
    for option in &old.options {
        if !new.options.contains(option) {
            changes.push(Change::Breaking(format!(
                "{scope}: input {} no longer takes {option}",
                old.name,
            )));
        }
    }
    for option in &new.options {
        if !old.options.contains(option) {
            changes.push(Change::Compatible(format!(
                "{scope}: input {} also takes {option}",
                new.name,
            )));
        }
    }
}

/// Field-exposure changes, shared by resources and sub-resources.
///
/// The exposure list is the same shape at both levels, and the engine
/// policy derives guards from both, so the rules cannot be allowed to
/// differ by nesting depth: a guard the differ watches on a resource
/// but not on its sub-collection is a silent hole exactly where
/// copal's one guarded field actually lives.
fn diff_fields(
    scope: &str,
    old: &[FieldExposure],
    new: &[FieldExposure],
    changes: &mut Vec<Change>,
) {
    for exposure in old {
        let api = exposure.api_name();
        match new.iter().find(|f| f.api_name() == api) {
            None => changes.push(Change::Breaking(format!("{scope}: field {api} removed",))),
            Some(current) => {
                // Who sees a field is contract surface in BOTH
                // directions. Guarding an open field takes values away
                // from deployed callers, and swapping guards changes
                // which callers those are. Removing a guard refuses
                // nobody, but it takes away the redaction itself: the
                // column becomes visible to every caller the guard
                // used to deny, on the API faces and in the derived
                // engine PERMISSIONS alike. Wider disclosure is not
                // additive for whoever the guard protected, so every
                // guard movement is named breaking and review decides.
                match (&exposure.guard, &current.guard) {
                    (None, Some(guard)) => changes.push(Change::Breaking(format!(
                        "{scope}: field {api} now guarded by {guard}",
                    ))),
                    (Some(before), Some(after)) if before != after => {
                        changes.push(Change::Breaking(format!(
                            "{scope}: field {api} guard changed {before} -> {after}",
                        )))
                    }
                    (Some(guard), None) => changes.push(Change::Breaking(format!(
                        "{scope}: field {api} no longer guarded (was {guard})",
                    ))),
                    _ => {}
                }
                if current.column != exposure.column {
                    // Same wire name over a different column: the
                    // value's meaning (and possibly type) changed
                    // under the client.
                    changes.push(Change::Breaking(format!(
                        "{scope}: field {api} now reads column {} (was {})",
                        current.column, exposure.column,
                    )));
                }
            }
        }
    }
    for exposure in new {
        if !old.iter().any(|f| f.api_name() == exposure.api_name()) {
            changes.push(Change::Compatible(format!(
                "{scope}: field {} added",
                exposure.api_name(),
            )));
        }
    }
}

fn diff_resource(old: &Resource, new: &Resource, changes: &mut Vec<Change>) {
    let scope = &old.name;

    diff_fields(scope, &old.fields, &new.fields, changes);

    for column in &old.filterable {
        if !new.filterable.contains(column) {
            changes.push(Change::Breaking(format!(
                "{scope}: filter {column} removed",
            )));
        }
    }
    for column in &old.sortable {
        if !new.sortable.contains(column) {
            changes.push(Change::Breaking(format!("{scope}: sort {column} removed",)));
        }
    }
    if new.max_page_size < old.max_page_size {
        changes.push(Change::Breaking(format!(
            "{scope}: max_page_size lowered {} -> {}",
            old.max_page_size, new.max_page_size,
        )));
    }

    // GraphQL names are part of a deployed schema's identity: fragments
    // name types, queries name fields. Effective names are compared so
    // an override equal to the derived default is a no-op.
    for (label, before, after) in [
        ("type", old.graphql_type_name(), new.graphql_type_name()),
        (
            "list field",
            old.graphql_list_field(),
            new.graphql_list_field(),
        ),
        (
            "get field",
            old.graphql_get_field(),
            new.graphql_get_field(),
        ),
    ] {
        if before != after {
            changes.push(Change::Breaking(format!(
                "{scope}: graphql {label} renamed {before} -> {after}",
            )));
        }
    }

    // Sub-collections: adding one is additive, removing one takes a
    // path and a GraphQL field away from deployed clients.
    for old_sub in &old.sub_resources {
        match new.sub_resources.iter().find(|s| s.name == old_sub.name) {
            None => changes.push(Change::Breaking(format!(
                "{scope}: sub-resource {} removed",
                old_sub.name,
            ))),
            Some(new_sub) => {
                diff_fields(
                    &format!("{scope}.{}", old_sub.name),
                    &old_sub.fields,
                    &new_sub.fields,
                    changes,
                );
                for column in &old_sub.filterable {
                    if !new_sub.filterable.contains(column) {
                        changes.push(Change::Breaking(format!(
                            "{scope}.{}: filter {column} removed",
                            old_sub.name,
                        )));
                    }
                }
                for column in &old_sub.sortable {
                    if !new_sub.sortable.contains(column) {
                        changes.push(Change::Breaking(format!(
                            "{scope}.{}: sort {column} removed",
                            old_sub.name,
                        )));
                    }
                }
                if new_sub.max_page_size < old_sub.max_page_size {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: max_page_size lowered {} -> {}",
                        old_sub.name, old_sub.max_page_size, new_sub.max_page_size,
                    )));
                }
                let before = old_sub.graphql_field();
                let after = new_sub.graphql_field();
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: graphql field renamed {before} -> {after}",
                        old_sub.name,
                    )));
                }
                let before = old_sub.graphql_type_name(old);
                let after = new_sub.graphql_type_name(new);
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}.{}: graphql type renamed {before} -> {after}",
                        old_sub.name,
                    )));
                }
            }
        }
    }
    for new_sub in &new.sub_resources {
        if !old.sub_resources.iter().any(|s| s.name == new_sub.name) {
            changes.push(Change::Compatible(format!(
                "{scope}: sub-resource {} added",
                new_sub.name,
            )));
        }
    }

    // Metering: attaching a class to unmetered reads introduces
    // refusals; detaching one removes them.
    match (&old.rate_class, &new.rate_class) {
        (None, Some(class)) => changes.push(Change::Breaking(format!(
            "{scope}: reads now metered by rate class {class}",
        ))),
        (Some(class), None) => changes.push(Change::Compatible(format!(
            "{scope}: reads no longer metered (was {class})",
        ))),
        _ => {}
    }

    // Scopes: a new requirement refuses callers that used to pass.
    for required in &new.reads_require {
        if !old.reads_require.contains(required) {
            changes.push(Change::Breaking(format!(
                "{scope}: reads now require scope {required}",
            )));
        }
    }
    for required in &old.reads_require {
        if !new.reads_require.contains(required) {
            changes.push(Change::Compatible(format!(
                "{scope}: reads no longer require scope {required}",
            )));
        }
    }

    // Watching: opening one is additive, closing one strands every
    // deployed subscriber, and the field name matters only while the
    // subscription exists.
    match (old.watchable, new.watchable) {
        (false, true) => changes.push(Change::Compatible(format!("{scope}: became watchable"))),
        (true, false) => changes.push(Change::Breaking(format!("{scope}: no longer watchable",))),
        (true, true) => {
            let before = old.graphql_watch_field();
            let after = new.graphql_watch_field();
            if before != after {
                changes.push(Change::Breaking(format!(
                    "{scope}: graphql watch field renamed {before} -> {after}",
                )));
            }
        }
        (false, false) => {}
    }

    {
        let before = old.content.unwrap_or_default();
        let after = new.content.unwrap_or_default();
        if before.upload && !after.upload {
            changes.push(Change::Breaking(format!(
                "{scope}: the content upload face was removed",
            )));
        }
        if before.download && !after.download {
            changes.push(Change::Breaking(format!(
                "{scope}: the content download face was removed",
            )));
        }
        if (!before.upload && after.upload) || (!before.download && after.download) {
            changes.push(Change::Compatible(format!(
                "{scope}: a content face was added"
            )));
        }
    }
    for old_action in &old.actions {
        match new.actions.iter().find(|a| a.name == old_action.name) {
            None => changes.push(Change::Breaking(format!(
                "{scope}: action {} removed",
                old_action.name,
            ))),
            Some(new_action) => {
                let before = old_action.graphql_field_name(old);
                let after = new_action.graphql_field_name(new);
                if before != after {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {} graphql field renamed {before} -> {after}",
                        old_action.name,
                    )));
                }
                diff_action(scope, old_action, new_action, changes);
            }
        }
    }
    for new_action in &new.actions {
        if !old.actions.iter().any(|a| a.name == new_action.name) {
            changes.push(Change::Compatible(format!(
                "{scope}: action {} added",
                new_action.name,
            )));
        }
    }
}

fn diff_action(scope: &str, old: &Action, new: &Action, changes: &mut Vec<Change>) {
    let name = &old.name;
    match (&old.rate_class, &new.rate_class) {
        (None, Some(class)) => changes.push(Change::Breaking(format!(
            "{scope}: action {name} now metered by rate class {class}",
        ))),
        (Some(class), None) => changes.push(Change::Compatible(format!(
            "{scope}: action {name} no longer metered (was {class})",
        ))),
        _ => {}
    }
    for required in &new.requires {
        if !old.requires.contains(required) {
            changes.push(Change::Breaking(format!(
                "{scope}: action {name} now requires scope {required}",
            )));
        }
    }
    for required in &old.requires {
        if !new.requires.contains(required) {
            changes.push(Change::Compatible(format!(
                "{scope}: action {name} no longer requires scope {required}",
            )));
        }
    }
    if old.method != new.method || old.path != new.path {
        changes.push(Change::Breaking(format!(
            "{scope}: action {name} moved ({} {} -> {} {})",
            old.method, old.path, new.method, new.path,
        )));
    }
    if old.output != new.output {
        changes.push(Change::Breaking(format!(
            "{scope}: action {name} output changed",
        )));
    }
    for field in &new.input {
        let previous = old.input.iter().find(|f| f.name == field.name);
        match previous {
            None if field.required => changes.push(Change::Breaking(format!(
                "{scope}: action {name} gained required input {}",
                field.name,
            ))),
            None => changes.push(Change::Compatible(format!(
                "{scope}: action {name} gained optional input {}",
                field.name,
            ))),
            Some(previous) => {
                if field.required && !previous.required {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {name} input {} became required",
                        field.name,
                    )));
                }
                if field.kind != previous.kind {
                    changes.push(Change::Breaking(format!(
                        "{scope}: action {name} input {} changed type",
                        field.name,
                    )));
                }
                diff_options(&format!("{scope}: action {name}"), previous, field, changes);
            }
        }
    }
    for field in &old.input {
        if !new.input.iter().any(|f| f.name == field.name) {
            // Removing an input a client may still send: servers should
            // ignore unknown fields, so this is compatible-with-note.
            changes.push(Change::Compatible(format!(
                "{scope}: action {name} input {} removed",
                field.name,
            )));
        }
    }
}

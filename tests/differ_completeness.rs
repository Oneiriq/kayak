//! Every change that takes something away from a caller is named.
//!
//! A differ that misses one is the worst thing in the toolchain: the
//! gate goes green and the break reaches whoever was calling. So
//! rather than checking the cases the differ already knows it makes,
//! this states the changes that are breaking by definition, applies
//! each to a contract, and insists each is reported.

use kayak::diff::diff;
use kayak::{
    Action, ActionField, ActionOutput, Contract, FieldExposure, Query, RateClass, Resource,
    SearchBacking, SearchKind, SubResource, TypeRef,
};

fn base() -> Contract {
    Contract {
        name: "probe".into(),
        version: "1.0.0".into(),
        ir_revision: 1,
        api_prefix: "/v1".into(),
        limits: None,
        rate_classes: vec![RateClass {
            name: "reads".into(),
            units_per_minute: 1_000,
        }],
        auth: kayak::AuthScheme::Bearer,
        resources: vec![Resource {
            name: "files".into(),
            table: "file".into(),
            identity: Default::default(),
            fields: vec![
                FieldExposure::column("path"),
                FieldExposure::column("state"),
                FieldExposure::renamed("size_bytes", "size"),
                FieldExposure::column("digest").with_guard("audit_only"),
            ],
            pinned: vec!["tenant_id".into()],
            pinned_either: vec![],
            filterable: vec!["state".into()],
            filter_options: [("state".to_owned(), vec!["live".into(), "deleted".into()])]
                .into_iter()
                .collect(),
            faces: Default::default(),
            sortable: vec!["created_at".into()],
            max_page_size: 100,
            watchable: true,
            actions: vec![Action {
                name: "issue_url".into(),
                method: "POST".into(),
                path: "/{id}/url".into(),
                input: vec![ActionField {
                    name: "ttl_secs".into(),
                    kind: TypeRef::Int,
                    required: false,
                    multiple: false,
                    options: Vec::new(),
                    description: None,
                }],
                output: ActionOutput::Json,
                description: None,
                graphql_field: None,
                requires: vec!["read".into()],
                rate_class: None,
            }],
            content: None,
            sub_resources: vec![SubResource {
                name: "versions".into(),
                table: "file_version".into(),
                parent_key: "file".into(),
                identity: Default::default(),
                fields: vec![
                    FieldExposure::column("ordinal"),
                    FieldExposure::column("created_by").with_guard("owner_or_admin"),
                ],
                pinned: vec![],
                filterable: vec![],
                sortable: vec!["created_at".into()],
                description: None,
                max_page_size: 50,
                graphql: None,
            }],
            rate_class: Some("reads".into()),
            reads_require: vec!["read".into()],
            graphql: None,
        }],
        queries: vec![Query {
            name: "search".into(),
            path: "/v1/search".into(),
            input: vec![ActionField {
                name: "q".into(),
                kind: TypeRef::String,
                required: true,
                multiple: false,
                options: Vec::new(),
                description: None,
            }],
            description: None,
            graphql_field: None,
            requires: vec!["search".into()],
            rate_class: None,
            searches: vec![SearchKind::Lexical, SearchKind::Vector],
            backing: vec![
                SearchBacking {
                    table: "text_chunk".into(),
                    column: "body".into(),
                    index: "idx_chunk_body".into(),
                    kind: SearchKind::Lexical,
                    dimension: None,
                    optional: false,
                },
                // The vector half carries both riders set, so the
                // mutations below can move each one in the direction
                // that takes something away.
                SearchBacking {
                    table: "text_chunk".into(),
                    column: "embedding".into(),
                    index: "idx_chunk_embedding".into(),
                    kind: SearchKind::Vector,
                    dimension: Some(768),
                    optional: false,
                },
            ],
        }],
    }
}

/// One change, and what it takes away said in words.
type Mutation = (&'static str, Box<dyn Fn(&mut Contract)>);

/// Each entry takes something away that a caller could have been
/// using. Every one has to come back Breaking.
fn taking_something_away() -> Vec<Mutation> {
    vec![
        (
            "a resource is gone",
            Box::new(|c: &mut Contract| c.resources.clear()),
        ),
        (
            "a query is gone",
            Box::new(|c: &mut Contract| c.queries.clear()),
        ),
        (
            "an action is gone",
            Box::new(|c: &mut Contract| c.resources[0].actions.clear()),
        ),
        (
            "a sub-collection is gone",
            Box::new(|c: &mut Contract| c.resources[0].sub_resources.clear()),
        ),
        (
            "an exposed field is gone",
            Box::new(|c: &mut Contract| {
                c.resources[0].fields.pop();
            }),
        ),
        (
            "a filter is gone",
            Box::new(|c: &mut Contract| c.resources[0].filterable.clear()),
        ),
        (
            "a sort is gone",
            Box::new(|c: &mut Contract| c.resources[0].sortable.clear()),
        ),
        (
            "the page ceiling is lower",
            Box::new(|c: &mut Contract| c.resources[0].max_page_size = 10),
        ),
        (
            "a filter's closed set lost a value",
            Box::new(|c: &mut Contract| {
                c.resources[0]
                    .filter_options
                    .get_mut("state")
                    .unwrap()
                    .retain(|v| v == "live")
            }),
        ),
        (
            "a sub-collection sort is gone",
            Box::new(|c: &mut Contract| c.resources[0].sub_resources[0].sortable.clear()),
        ),
        (
            "a sub-collection page ceiling is lower",
            Box::new(|c: &mut Contract| c.resources[0].sub_resources[0].max_page_size = 10),
        ),
        (
            "a resource stopped being watchable",
            Box::new(|c: &mut Contract| c.resources[0].watchable = false),
        ),
        (
            "reading takes another scope",
            Box::new(|c: &mut Contract| c.resources[0].reads_require.push("admin".into())),
        ),
        (
            "an action takes another scope",
            Box::new(|c: &mut Contract| c.resources[0].actions[0].requires.push("admin".into())),
        ),
        (
            "a query takes another scope",
            Box::new(|c: &mut Contract| c.queries[0].requires.push("admin".into())),
        ),
        (
            "an action gained a required input",
            Box::new(|c: &mut Contract| {
                c.resources[0].actions[0].input.push(ActionField {
                    name: "reason".into(),
                    kind: TypeRef::String,
                    required: true,
                    multiple: false,
                    options: Vec::new(),
                    description: None,
                })
            }),
        ),
        (
            "an optional input became required",
            Box::new(|c: &mut Contract| c.resources[0].actions[0].input[0].required = true),
        ),
        (
            "an input changed type",
            Box::new(|c: &mut Contract| c.resources[0].actions[0].input[0].kind = TypeRef::String),
        ),
        (
            "an action moved",
            Box::new(|c: &mut Contract| c.resources[0].actions[0].path = "/{id}/elsewhere".into()),
        ),
        (
            "a budget shrank",
            Box::new(|c: &mut Contract| c.rate_classes[0].units_per_minute = 10),
        ),
        (
            "an operation started being metered",
            Box::new(|c: &mut Contract| {
                c.resources[0].actions[0].rate_class = Some("reads".into())
            }),
        ),
        (
            "a ceiling appeared",
            Box::new(|c: &mut Contract| {
                c.limits = Some(kayak::ContractLimits {
                    max_depth: Some(3),
                    max_complexity: None,
                    max_watches_per_principal: None,
                })
            }),
        ),
        (
            "an action's answer changed shape",
            Box::new(|c: &mut Contract| c.resources[0].actions[0].output = ActionOutput::None),
        ),
        // Guards move in four ways and every one changes who sees the
        // field, so every one has to come back breaking: adding takes
        // values from callers, swapping changes which callers, and
        // REMOVING takes away the redaction itself -- the column shows
        // to callers the guard refused, on the API faces and in the
        // derived engine PERMISSIONS alike.
        (
            "an open field is now guarded",
            Box::new(|c: &mut Contract| c.resources[0].fields[0].guard = Some("admin_only".into())),
        ),
        (
            "a field's guard swapped",
            Box::new(|c: &mut Contract| c.resources[0].fields[3].guard = Some("admin_only".into())),
        ),
        (
            "a field's guard is gone",
            Box::new(|c: &mut Contract| c.resources[0].fields[3].guard = None),
        ),
        (
            "a sub-collection field's guard is gone",
            Box::new(|c: &mut Contract| c.resources[0].sub_resources[0].fields[1].guard = None),
        ),
        (
            "a sub-collection field is gone",
            Box::new(|c: &mut Contract| {
                c.resources[0].sub_resources[0].fields.pop();
            }),
        ),
        (
            "a sub-collection field reads a different column",
            Box::new(|c: &mut Contract| {
                c.resources[0].sub_resources[0].fields[0] =
                    FieldExposure::renamed("legacy_ordinal", "ordinal")
            }),
        ),
        // The identity is the name callers address instances by --
        // in rows, in the by-instance path, in the GraphQL argument.
        // Renaming it moves all three; withdrawing it removes a field
        // deployed callers read.
        (
            "the identity is renamed",
            Box::new(|c: &mut Contract| {
                c.resources[0].identity = kayak::Identity::Column("path".into())
            }),
        ),
        (
            "the rows lost their identity",
            Box::new(|c: &mut Contract| c.resources[0].identity = kayak::Identity::Absent),
        ),
        (
            "a sub-collection's rows lost their identity",
            Box::new(|c: &mut Contract| {
                c.resources[0].sub_resources[0].identity = kayak::Identity::Absent
            }),
        ),
        // A capability the query stops performing is a capability its
        // callers stop getting, whatever the resolver falls back to.
        (
            "a query stopped searching semantically",
            Box::new(|c: &mut Contract| c.queries[0].searches.retain(|k| *k != SearchKind::Vector)),
        ),
        // A backing has no name of its own: WHERE the machinery is --
        // table, column, index, kind -- is its identity, so each of the
        // four is re-pointed separately here and every re-point must
        // come back breaking, or a schema change could move the
        // machinery out from under a promised search with the gate
        // green. The width and the optional flag move under a fixed
        // identity and are exercised beside them.
        (
            "a search backing is gone",
            Box::new(|c: &mut Contract| c.queries[0].backing.clear()),
        ),
        (
            "a backing searches a different table",
            Box::new(|c: &mut Contract| c.queries[0].backing[0].table = "file_text".into()),
        ),
        (
            "a backing searches a different column",
            Box::new(|c: &mut Contract| c.queries[0].backing[0].column = "digest".into()),
        ),
        (
            "a backing rests on a different index",
            Box::new(|c: &mut Contract| c.queries[0].backing[0].index = "idx_other".into()),
        ),
        (
            "a backing changed kind",
            Box::new(|c: &mut Contract| c.queries[0].backing[0].kind = SearchKind::Vector),
        ),
        (
            "a backing searches at a different width",
            Box::new(|c: &mut Contract| c.queries[0].backing[1].dimension = Some(1536)),
        ),
        (
            "a backing stopped pinning its width",
            Box::new(|c: &mut Contract| c.queries[0].backing[1].dimension = None),
        ),
        (
            "a backing the deployment had to have is now optional",
            Box::new(|c: &mut Contract| c.queries[0].backing[1].optional = true),
        ),
        // Every direction of moving authentication takes something away.
        // Switching it strands every deployed client on a credential the
        // service stopped reading; adding one locks out callers that sent
        // nothing; and dropping one stops refusing whoever it kept out --
        // the same reasoning that makes a REMOVED field guard breaking.
        (
            "authentication switched to another scheme",
            Box::new(|c: &mut Contract| {
                c.auth = kayak::AuthScheme::Header {
                    name: "x-api-key".into(),
                    credential: "api_key".into(),
                }
            }),
        ),
        (
            "authentication is gone",
            Box::new(|c: &mut Contract| c.auth = kayak::AuthScheme::None),
        ),
    ]
}

#[test]
fn every_change_that_takes_something_away_is_breaking() {
    let mut missed = Vec::new();
    for (what, apply) in taking_something_away() {
        let before = base();
        let mut after = base();
        apply(&mut after);
        let changes = diff(&before, &after);
        if !changes.iter().any(kayak::diff::Change::is_breaking) {
            missed.push(format!("{what}  ->  reported {changes:?}"));
        }
    }
    assert!(
        missed.is_empty(),
        "the differ let these through:\n  {}",
        missed.join("\n  "),
    );
}

/// A contract compared with itself has nothing to say.
///
/// A differ that invents a change on an unchanged contract turns the
/// gate into noise, and noise gets waved through.
#[test]
fn a_contract_has_no_quarrel_with_itself() {
    assert_eq!(diff(&base(), &base()), vec![]);

    // Spelling the default out loud is not a change: identities are
    // compared on the wire, where `id` and silence are the same name.
    let mut spelled = base();
    spelled.resources[0].identity = kayak::Identity::Column("id".into());
    assert_eq!(diff(&base(), &spelled), vec![]);
}

/// Adding is not taking away.
#[test]
fn additions_are_compatible() {
    let mut before = base();
    let mut after = base();
    // Rows that gained a name where they had none: additive, the way
    // a new field is. Everything a caller read is still there.
    before.resources[0].identity = kayak::Identity::Absent;
    after.resources[0].filterable.push("path".into());
    after.resources[0].max_page_size = 500;
    after.rate_classes[0].units_per_minute = 10_000;
    after.resources[0].actions[0].input.push(ActionField {
        name: "note".into(),
        kind: TypeRef::String,
        required: false,
        multiple: false,
        options: Vec::new(),
        description: None,
    });
    // A backing added to an existing query promises MORE about the
    // same wire surface, which takes nothing from anyone.
    after.queries[0].backing.push(SearchBacking {
        table: "text_chunk".into(),
        column: "title".into(),
        index: "idx_chunk_title".into(),
        kind: SearchKind::Lexical,
        dimension: None,
        optional: false,
    });
    let changes = diff(&before, &after);
    let breaking: Vec<_> = changes
        .iter()
        .filter(|c| c.is_breaking())
        .map(kayak::diff::Change::message)
        .collect();
    assert!(
        breaking.is_empty(),
        "additions read as breaking: {breaking:?}"
    );
    assert!(!changes.is_empty(), "and they are still reported");
}

/// Each entry adds something and takes nothing away.
fn adding_something() -> Vec<Mutation> {
    vec![
        (
            "a filter is added",
            Box::new(|c: &mut Contract| c.resources[0].filterable.push("path".into())),
        ),
        (
            "a sort is added",
            Box::new(|c: &mut Contract| c.resources[0].sortable.push("path".into())),
        ),
        (
            "the page ceiling is higher",
            Box::new(|c: &mut Contract| c.resources[0].max_page_size = 500),
        ),
        (
            "a sub-collection filter is added",
            Box::new(|c: &mut Contract| {
                c.resources[0].sub_resources[0]
                    .filterable
                    .push("ordinal".into())
            }),
        ),
        (
            "a sub-collection sort is added",
            Box::new(|c: &mut Contract| {
                c.resources[0].sub_resources[0]
                    .sortable
                    .push("ordinal".into())
            }),
        ),
        (
            "a sub-collection page ceiling is higher",
            Box::new(|c: &mut Contract| c.resources[0].sub_resources[0].max_page_size = 200),
        ),
        (
            "a filter's closed set gained a value",
            Box::new(|c: &mut Contract| {
                c.resources[0]
                    .filter_options
                    .get_mut("state")
                    .unwrap()
                    .push("archived".into())
            }),
        ),
        (
            "a filter's closed set was lifted",
            Box::new(|c: &mut Contract| c.resources[0].filter_options.clear()),
        ),
        (
            "a field is added",
            Box::new(|c: &mut Contract| {
                c.resources[0]
                    .fields
                    .push(FieldExposure::column("created_at"))
            }),
        ),
        (
            "a budget grew",
            Box::new(|c: &mut Contract| c.rate_classes[0].units_per_minute = 10_000),
        ),
        (
            "reading takes one scope fewer",
            Box::new(|c: &mut Contract| c.resources[0].reads_require.clear()),
        ),
    ]
}

/// Every addition is named, and none is named breaking.
///
/// `additions_are_compatible` applies its additions together, so one
/// the differ reports covers for one it drops. `kayak diff` stayed
/// silent on an added filter or sort, and on a raised page ceiling,
/// for exactly that reason: the CLI said "no contract changes" about a
/// contract that had grown. Each addition is applied alone here.
#[test]
fn every_addition_is_named_and_none_breaks() {
    let mut problems = Vec::new();
    for (what, apply) in adding_something() {
        let before = base();
        let mut after = base();
        apply(&mut after);
        let changes = diff(&before, &after);
        if changes.is_empty() {
            problems.push(format!("{what}  ->  reported nothing"));
        } else if changes.iter().any(kayak::diff::Change::is_breaking) {
            problems.push(format!("{what}  ->  reported breaking: {changes:?}"));
        }
    }
    assert!(
        problems.is_empty(),
        "the differ misread these additions:\n  {}",
        problems.join("\n  "),
    );
}

/// A closed set appearing on a filter that took anything refuses every
/// caller sending a value outside it, as it does on an input.
#[test]
fn a_filter_set_appearing_is_breaking() {
    let mut open = base();
    open.resources[0].filter_options.clear();
    let changes = diff(&open, &base());
    assert!(
        changes.iter().any(
            |c| matches!(c, kayak::diff::Change::Breaking(m) if m.contains("filter state now takes only live, deleted"))
        ),
        "{changes:?}",
    );
}

/// A backing that promises MORE about machinery it already pointed at
/// is not a backing anyone lost.
///
/// This is the case matching by identity exists for. Compared member
/// by member, a width appearing or an optional backing becoming
/// guaranteed reads as the old backing gone and a new one arrived --
/// breaking, on a change that took nothing from anybody, which is the
/// kind of false alarm that teaches people to wave the gate through.
#[test]
fn a_strengthened_backing_promise_is_compatible() {
    let mut before = base();
    before.queries[0].backing[1].dimension = None;
    before.queries[0].backing[1].optional = true;
    let mut after = before.clone();
    after.queries[0].backing[1].dimension = Some(768);
    after.queries[0].backing[1].optional = false;

    let changes = diff(&before, &after);
    let breaking: Vec<_> = changes
        .iter()
        .filter(|c| c.is_breaking())
        .map(kayak::diff::Change::message)
        .collect();
    assert!(breaking.is_empty(), "read as breaking: {breaking:?}");
    let said: Vec<_> = changes.iter().map(kayak::diff::Change::message).collect();
    assert!(
        said.iter().any(|m| m.contains("pins its width at 768")),
        "{said:?}",
    );
    assert!(
        said.iter()
            .any(|m| m.contains("required of every deployment")),
        "{said:?}",
    );
}

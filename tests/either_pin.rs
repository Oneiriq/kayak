//! Pinning to one of several columns.
//!
//! A symmetric relationship stores one row per unordered pair, so the
//! caller is EITHER endpoint and their rows are `a = me OR b = me`.
//! An ordinary pin is an AND and cannot say that; pinning one side
//! would credit an index for half the rows.
//!
//! The rule these assert was measured against SurrealDB 3 rather than
//! reasoned about: the engine answers the disjunction as a
//! UnionIndexScan, one seek per branch, so every branch needs an index
//! whose prefix is its own column. The friend table of a social
//! service is the shape that motivated it.

use kayak::diff::{diff, Change};
use kayak::validate::validate;
use kayak::{Contract, FieldExposure, Resource, ResourceFaces};
use surql::schema::{index, string_field, table_schema, TableDefinition, TableMode};

/// The friend table: one row per unordered pair, `state` alongside.
fn schema(indexes: Vec<surql::schema::IndexDefinition>) -> Vec<TableDefinition> {
    let built = |b: surql::schema::FieldBuilder| b.build_unchecked().unwrap();
    vec![table_schema("friend")
        .with_mode(TableMode::Schemafull)
        .with_fields([
            built(string_field("a")),
            built(string_field("b")),
            built(string_field("state")),
        ])
        .with_indexes(indexes)]
}

/// Both branches served, each carrying `state` for the filter.
fn composites() -> Vec<surql::schema::IndexDefinition> {
    vec![
        index("friend_a_state_idx", ["a", "state"]),
        index("friend_b_state_idx", ["b", "state"]),
    ]
}

fn contract(either: &[&str], filterable: &[&str]) -> Contract {
    Contract {
        name: "probe".into(),
        version: "0.1.0".into(),
        ir_revision: 1,
        api_prefix: String::new(),
        limits: None,
        rate_classes: vec![],
        auth: kayak::AuthScheme::None,
        resources: vec![Resource {
            name: "friends".into(),
            table: "friend".into(),
            identity: Default::default(),
            faces: ResourceFaces::LIST_ONLY,
            fields: vec![FieldExposure::column("state")],
            pinned: vec![],
            pinned_either: either.iter().map(|c| (*c).to_owned()).collect(),
            filterable: filterable.iter().map(|c| (*c).to_owned()).collect(),
            filter_options: Default::default(),
            sortable: vec![],
            max_page_size: 100,
            graphql: None,
            watchable: false,
            reads_require: vec![],
            rate_class: None,
            sub_resources: vec![],
            actions: vec![],
            content: None,
        }],
        queries: vec![],
    }
}

#[test]
fn both_branches_indexed_is_accepted() {
    // The shape a social service's friend table actually has.
    assert_eq!(
        validate(&contract(&["a", "b"], &["state"]), &schema(composites())),
        vec![],
    );
}

#[test]
fn a_branch_with_no_index_behind_it_is_refused() {
    // Only `a` is covered. The engine would seek one side and scan the
    // other, which is the whole read scanning.
    let half = vec![index("friend_a_state_idx", ["a", "state"])];
    let violations = validate(&contract(&["a", "b"], &["state"]), &schema(half));
    assert!(!violations.is_empty(), "a half-served union was accepted");
    // The message has to name the branch, or the author cannot tell
    // which index is missing.
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("pinned on b")),
        "{violations:?}",
    );
}

#[test]
fn single_column_indexes_do_not_serve_a_filtered_union() {
    // `a` and `b` indexed alone: each branch seeks, but `state` then
    // filters. Measured: with a bare state index present the planner
    // takes THAT and the pair test degrades to a residual filter,
    // reading every row of the narrowed state.
    let singles = vec![index("friend_a_idx", ["a"]), index("friend_b_idx", ["b"])];
    let violations = validate(&contract(&["a", "b"], &["state"]), &schema(singles));
    assert!(!violations.is_empty(), "{violations:?}");
}

#[test]
fn an_unfiltered_union_needs_only_the_leading_column() {
    // No filter claim, so each branch is a bare equality seek and a
    // single-column index on each side serves it.
    let singles = vec![index("friend_a_idx", ["a"]), index("friend_b_idx", ["b"])];
    assert_eq!(
        validate(&contract(&["a", "b"], &[]), &schema(singles)),
        vec![]
    );
}

#[test]
fn one_alternative_is_a_pin_and_is_refused_as_a_choice() {
    let violations = validate(&contract(&["a"], &[]), &schema(composites()));
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("fewer than two columns")),
        "{violations:?}",
    );
}

#[test]
fn a_column_cannot_be_both_always_bound_and_a_branch() {
    let mut both = contract(&["a", "b"], &[]);
    both.resources[0].pinned = vec!["a".into()];
    let violations = validate(&both, &schema(composites()));
    assert!(
        violations
            .iter()
            .any(|v| v.to_string().contains("both pinned and an either-of")),
        "{violations:?}",
    );
}

#[test]
fn an_alternative_naming_no_column_is_refused() {
    let violations = validate(&contract(&["a", "nope"], &[]), &schema(composites()));
    assert!(
        violations.iter().any(|v| v.to_string().contains("nope")),
        "{violations:?}",
    );
}

#[test]
fn changing_what_the_server_binds_is_breaking_in_both_directions() {
    // Narrowing hides rows a caller used to see; widening shows rows
    // they did not. One costs results, the other is a disclosure.
    let wide = contract(&["a", "b"], &[]);
    let mut narrow = wide.clone();
    narrow.resources[0].pinned_either = vec!["a".into(), "b".into(), "state".into()];

    for (before, after) in [(&wide, &narrow), (&narrow, &wide)] {
        let changes = diff(before, after);
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::Breaking(m) if m.contains("either-of pin"))),
            "{changes:?}",
        );
    }
}

#[test]
fn the_default_stays_out_of_a_serialized_contract() {
    let plain = contract(&[], &[]);
    let rendered = serde_json::to_string(&plain).unwrap();
    assert!(!rendered.contains("pinned_either"), "{rendered}");

    let either = serde_json::to_string(&contract(&["a", "b"], &[])).unwrap();
    assert!(either.contains("pinned_either"), "{either}");
    let parsed: Contract = serde_json::from_str(&either).unwrap();
    assert_eq!(parsed.resources[0].pinned_either, vec!["a", "b"]);
}

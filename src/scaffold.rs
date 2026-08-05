//! A contract, started from the schema that will back it.
//!
//! Janus asks for a contract before it can do anything, and writing
//! one by hand against an existing database is the work that stops
//! people from trying it at all: every field copied, every filter and
//! sort claim checked against an index by eye. The schema already
//! knows all of that. This reads it.
//!
//! What comes out is a starting point that [`validate`](crate::validate)
//! accepts against the same schema, which is the property worth having:
//! the filter and sort claims are index-backed by construction rather
//! than by hope, so the first generation succeeds and the editing that
//! follows is about API shape rather than about repairing claims.
//!
//! Two things it will not guess. Actions are behavior and live in the
//! service, so a scaffolded resource has none. Columns whose names
//! suggest they hold a secret are left out, because a tool that writes
//! API surface should never be the reason a hash reaches a client; the
//! caller is told which ones were skipped and can expose them
//! deliberately.
//!
//! Where it has a choice it claims less. The differ calls a removed
//! filter or sort breaking and an added one compatible, so a claim the
//! scaffold invents costs a major version to withdraw while one it
//! omits costs a line to add. That asymmetry is why sortable comes out
//! narrower than the validator would tolerate.

use surql::schema::{IndexType, TableDefinition};

use crate::ir::{Contract, FieldExposure, Resource};

/// What a scaffold produced, and what it declined to.
#[derive(Debug, Clone)]
pub struct Scaffold {
    pub contract: Contract,
    /// Columns left unexposed because their names suggest a secret,
    /// as `table.column`. Expose any of them deliberately.
    pub withheld: Vec<String>,
}

/// Column-name fragments that make a field a poor thing to publish by
/// default. A name is a weak signal, and the cost of the two mistakes
/// is not symmetric: a withheld column is noticed the first time
/// somebody wants it, and a published hash is noticed later.
const SECRET_BEARING: &[&str] = &[
    "secret",
    "token",
    "password",
    "passwd",
    "hash",
    "credential",
    "private_key",
    "api_key",
    "salt",
];

fn secret_bearing(column: &str) -> bool {
    let lowered = column.to_ascii_lowercase();
    SECRET_BEARING.iter().any(|mark| lowered.contains(mark))
}

/// Plural for a resource name, since a collection reads better than a
/// row and the generators derive the singular back for method names.
fn plural(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    if lowered.ends_with('s')
        || lowered.ends_with('x')
        || lowered.ends_with('z')
        || lowered.ends_with("ch")
        || lowered.ends_with("sh")
    {
        return format!("{name}es");
    }
    match lowered.chars().last() {
        Some('y') => {
            let stem: String = name.chars().take(name.chars().count() - 1).collect();
            let vowel_before = lowered
                .chars()
                .nth(lowered.chars().count().saturating_sub(2))
                .is_some_and(|c| "aeiou".contains(c));
            if vowel_before {
                format!("{name}s")
            } else {
                format!("{stem}ies")
            }
        }
        _ => format!("{name}s"),
    }
}

/// Indexes that answer equality and ordering. A full-text or vector
/// index covers a column without serving either, so a claim resting on
/// one would read as covered and behave like a scan.
fn ordering_indexes(table: &TableDefinition) -> Vec<&Vec<String>> {
    table
        .indexes
        .iter()
        .filter(|index| matches!(index.index_type, IndexType::Standard | IndexType::Unique))
        .map(|index| &index.columns)
        .collect()
}

fn resource_from(
    table: &TableDefinition,
    pinned: &[String],
    withheld: &mut Vec<String>,
) -> Resource {
    let columns: Vec<&str> = table.fields.iter().map(|f| f.name.as_str()).collect();
    let bound: Vec<String> = pinned
        .iter()
        .filter(|column| columns.contains(&column.as_str()))
        .cloned()
        .collect();

    let mut fields = Vec::new();
    for column in &columns {
        if bound.iter().any(|b| b == column) {
            continue;
        }
        if secret_bearing(column) {
            withheld.push(format!("{}.{}", table.name, column));
            continue;
        }
        fields.push(FieldExposure::column(*column));
    }

    let indexes = ordering_indexes(table);
    let exposed = |column: &str| fields.iter().any(|f| f.column == column);

    // A filter needs the column to appear in an index at all.
    let filterable: Vec<String> = columns
        .iter()
        .filter(|column| exposed(column))
        .filter(|column| indexes.iter().any(|c| c.iter().any(|i| i == *column)))
        .map(|column| (*column).to_owned())
        .collect();

    // A sort needs every column ahead of it in some index bound, and
    // here the bar is pinning. The validator also accepts a filterable
    // prefix, because a caller who does filter on it gets an
    // index-served order. A scaffold cannot assume they will. `filterable` describes what a caller may send, nothing
    // obliges them to send it, and an unfiltered sort down a composite
    // index scans. So this claims only what the server guarantees on
    // every request, which is what it pins. Copal's `created_at` sort is the case
    // worth naming: it sits behind `state` and is genuinely the listing
    // order that index exists for, and it is still the right thing to
    // add by hand, because adding a sort is a compatible change and
    // removing one the scaffold guessed wrong is breaking.
    let boundable = |column: &str| bound.iter().any(|b| b == column);
    let sortable: Vec<String> = columns
        .iter()
        .filter(|column| exposed(column))
        .filter(|column| {
            indexes.iter().any(|index| {
                index
                    .iter()
                    .position(|c| c == *column)
                    .is_some_and(|k| index[..k].iter().all(|earlier| boundable(earlier)))
            })
        })
        .map(|column| (*column).to_owned())
        .collect();

    Resource {
        name: plural(&table.name),
        table: table.name.clone(),
        fields,
        pinned: bound,
        filterable,
        sortable,
        max_page_size: 100,
        actions: Vec::new(),
        content: None,
        sub_resources: Vec::new(),
        rate_class: None,
        reads_require: Vec::new(),
        watchable: false,
        graphql: None,
    }
}

/// Read a schema and answer with a contract it will validate against.
///
/// `pinned` names columns the server always binds before any caller
/// input, tenancy being the usual one. They are never exposed as
/// fields, and they are what a sort claim rests on: given an index
/// `(tenant_id, state, created_at)`, pinning `tenant_id` is what makes
/// `state` sortable, and `created_at` stays unclaimed because nothing
/// obliges a caller to bind `state`.
pub fn scaffold(
    name: &str,
    version: &str,
    schema: &[TableDefinition],
    pinned: &[String],
) -> Scaffold {
    let mut withheld = Vec::new();
    let resources = schema
        .iter()
        .map(|table| resource_from(table, pinned, &mut withheld))
        .collect();
    Scaffold {
        contract: Contract {
            name: name.to_owned(),
            version: version.to_owned(),
            ir_revision: crate::ir::default_ir_revision(),
            rate_classes: Vec::new(),
            limits: None,
            resources,
            queries: Vec::new(),
        },
        withheld,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surql::schema::{
        datetime_field, index, string_field, table_schema, unique_index, FieldBuilder, TableMode,
    };

    fn built(builder: FieldBuilder) -> surql::schema::FieldDefinition {
        builder.build_unchecked().unwrap()
    }

    fn file_table() -> TableDefinition {
        table_schema("file")
            .with_mode(TableMode::Schemafull)
            .with_fields([
                built(string_field("tenant_id")),
                built(string_field("path")),
                built(string_field("state")),
                built(string_field("secret_sealed")),
                built(datetime_field("created_at")),
            ])
            .with_indexes([
                unique_index("uniq_path", ["tenant_id", "path"]),
                index("idx_listing", ["tenant_id", "state", "created_at"]),
            ])
    }

    /// The property the whole thing rests on: what comes out is
    /// accepted by the validator against the schema it came from.
    #[test]
    fn a_scaffold_validates_against_its_own_schema() {
        let schema = vec![file_table()];
        let made = scaffold("demo", "0.1.0", &schema, &["tenant_id".to_owned()]);
        assert_eq!(
            crate::validate(&made.contract, &schema),
            vec![],
            "a scaffold that needs repair before it generates is worth nothing",
        );
    }

    #[test]
    fn claims_follow_the_indexes() {
        let schema = vec![file_table()];
        let made = scaffold("demo", "0.1.0", &schema, &["tenant_id".to_owned()]);
        let resource = &made.contract.resources[0];

        assert_eq!(resource.name, "files", "a collection reads plural");
        assert_eq!(resource.table, "file");
        assert_eq!(resource.pinned, vec!["tenant_id".to_owned()]);

        // A filter reaches any indexed column, because a caller who
        // wants the composite can send the earlier columns too.
        assert!(resource.filterable.contains(&"state".to_owned()));
        assert!(resource.filterable.contains(&"path".to_owned()));
        assert!(resource.filterable.contains(&"created_at".to_owned()));

        // A sort reaches only what the pinned prefix covers. `state`
        // sits behind tenant_id alone and qualifies; `created_at` sits
        // behind `state` as well, and nothing makes a caller bind it,
        // so the scaffold leaves that one to be added deliberately.
        assert!(resource.sortable.contains(&"state".to_owned()));
        assert!(resource.sortable.contains(&"path".to_owned()));
        assert!(
            !resource.sortable.contains(&"created_at".to_owned()),
            "an unbound prefix is a scan, and withdrawing the claim later is breaking",
        );
    }

    /// Widening is the edit the scaffold expects, so what it omits must
    /// still be legal to add. `created_at` is the case: left out by
    /// choice, accepted by the validator when an operator puts it back.
    #[test]
    fn what_the_scaffold_omits_is_still_addable() {
        let schema = vec![file_table()];
        let mut made = scaffold("demo", "0.1.0", &schema, &["tenant_id".to_owned()]);
        made.contract.resources[0]
            .sortable
            .push("created_at".to_owned());
        assert_eq!(crate::validate(&made.contract, &schema), vec![]);
    }

    #[test]
    fn a_column_that_sounds_like_a_secret_is_withheld() {
        let schema = vec![file_table()];
        let made = scaffold("demo", "0.1.0", &schema, &["tenant_id".to_owned()]);
        let resource = &made.contract.resources[0];
        assert!(
            !resource.fields.iter().any(|f| f.column == "secret_sealed"),
            "a tool that writes API surface must not publish a sealed secret",
        );
        assert_eq!(made.withheld, vec!["file.secret_sealed".to_owned()]);
        assert!(
            !resource.fields.iter().any(|f| f.column == "tenant_id"),
            "a server-bound column is not a field",
        );
    }

    /// A full-text index covers a column without ordering it, so a
    /// claim resting on one would read as covered and scan.
    #[test]
    fn a_search_index_does_not_make_a_claim() {
        let schema = vec![table_schema("note")
            .with_mode(TableMode::Schemafull)
            .with_fields([built(string_field("body"))])
            .with_indexes([surql::schema::search_index("idx_body", ["body"])])];
        let made = scaffold("demo", "0.1.0", &schema, &[]);
        let resource = &made.contract.resources[0];
        assert!(resource.filterable.is_empty());
        assert!(resource.sortable.is_empty());
        assert_eq!(crate::validate(&made.contract, &schema), vec![]);
    }

    #[test]
    fn plurals_read_like_english() {
        assert_eq!(plural("file"), "files");
        assert_eq!(plural("box"), "boxes");
        assert_eq!(plural("batch"), "batches");
        assert_eq!(plural("entry"), "entries");
        assert_eq!(plural("day"), "days");
        assert_eq!(plural("address"), "addresses");
    }
}

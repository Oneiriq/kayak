//! The build-time gate: a contract only compiles against the schema it
//! actually has.
//!
//! Every violation is a generation failure, not a runtime surprise. The
//! two index rules encode the operational lesson this library exists to
//! enforce: an unindexed filter or sort ships fine, works in the demo,
//! and becomes a table scan (or a hard 5xx on multi-field ORDER BY) in
//! production.
//!
//! Rules:
//! - pinned column: must exist on the table, and the bound set as a
//!   whole must reach an index: some ordering index leads with a bound
//!   column, or the listing that binds them all on every read scans.
//!   No single pin owes an index of its own; the set does.
//! - filterable column: must appear in at least one index on the table.
//! - sortable column: some index must contain it at a position where
//!   every EARLIER column is pinned or filterable; an index serves an
//!   ORDER BY only from a prefix whose head is equality-bound. A bare
//!   leading column is the degenerate case.
//! - search backing: the named index must exist on the named table,
//!   hold the named column, be the kind's own machinery — FULLTEXT
//!   for a lexical backing, HNSW, MTREE, or DISKANN for a vector one —
//!   and, where the backing pins a width, be defined over that width.
//!   The mirror
//!   image of the index-type rule: there a filter rested on a search
//!   index that cannot narrow, here a search rests on a b-tree that
//!   cannot match terms or walk neighbours.
//! - declared search: a kind the query says it performs must have a
//!   backing of that kind behind it. This is the one rule that catches
//!   an ABSENCE rather than a mistake, and absence is how unindexed
//!   search ships: every other rule here reads something the author
//!   wrote and holds it to the schema, while a semantic search over a
//!   column with no vector index writes nothing at all. Declaring the
//!   capability is what gives the gate something to refuse.
//!
//! Both index rules read [`crate::indexes`] rather than the table's
//! index list, because only a standard or unique index counts toward
//! either. A FULLTEXT or vector index covers a column without narrowing
//! an equality on it or ordering by it, and a claim resting on one is
//! the failure this file exists to catch wearing the disguise of the
//! thing that would have caught it.

use surql::schema::{IndexDefinition, IndexType, TableDefinition};

use crate::indexes::{ordering_indexes, seekable_through, serves_ordering, serves_search};
use crate::ir::{Contract, FieldExposure, Query, Resource, SearchKind, SubResource};

/// One listing surface's index-relevant claims, so a resource and a
/// sub-resource are checked by exactly one rulebook.
struct Listing<'a> {
    /// How the surface names itself in a violation.
    scope: String,
    table: &'a str,
    fields: &'a [FieldExposure],
    /// Columns the server equality-binds; credited to sort prefixes.
    bound: Vec<&'a str>,
    filterable: &'a [String],
    sortable: &'a [String],
    /// Whether this surface projects rows at all.
    ///
    /// A resource with no collection face and no action answering with
    /// one never renders its type, so it has nothing to expose and
    /// requiring a field would be requiring a projection that is never
    /// built. That is the shape of an RPC-only domain: `friends` is
    /// five verbs over a table whose rows no caller ever receives.
    projects: bool,
}

/// A single contract-vs-schema violation, formatted for humans in
/// generator output.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Violation {
    #[error("resource {resource}: table {table} does not exist in the schema")]
    UnknownTable { resource: String, table: String },

    #[error("resource {resource}: column {column} does not exist on table {table}")]
    UnknownColumn {
        resource: String,
        table: String,
        column: String,
    },

    #[error("resource {resource}: exposes no fields")]
    NoFields { resource: String },

    #[error("resource {resource}: duplicate API field name {name} (rename collision)")]
    DuplicateApiName { resource: String, name: String },

    #[error(
        "resource {resource}: field {name} shadows the id every resource carries; \
         rename it or leave it unexposed"
    )]
    ShadowsId { resource: String, name: String },

    #[error(
        "{first} and {second} both generate the client method {name}; \
         rename one of them"
    )]
    DuplicateMethod {
        name: String,
        first: String,
        second: String,
    },

    #[error("resource {resource}: action {action}: {problem}")]
    InvalidAction {
        resource: String,
        action: String,
        problem: String,
    },

    #[error("{scope}: name {name:?} {problem}")]
    InvalidName {
        scope: String,
        name: String,
        problem: String,
    },

    #[error(
        "resource {resource}: filterable column {column} is not covered by any \
         index on {table}; filtering on it would scan the table"
    )]
    UnindexedFilter {
        resource: String,
        table: String,
        column: String,
    },

    #[error(
        "resource {resource}: sortable column {column} is not reachable as an \
         index sort suffix on {table}; some index must hold it with every \
         earlier column pinned or filterable, or ORDER BY falls off the index"
    )]
    UnindexedSort {
        resource: String,
        table: String,
        column: String,
    },

    #[error(
        "resource {resource}: {claim} column {column} is indexed on {table}, but \
         only by the {index_type} index {index}, which serves neither an equality \
         filter nor an ORDER BY; add a standard index holding {column}, or drop \
         the claim"
    )]
    WrongIndexType {
        resource: String,
        table: String,
        column: String,
        /// `filterable` or `sortable`: which claim the index fails to
        /// answer, so one sentence serves both.
        claim: String,
        index: String,
        index_type: IndexType,
    },

    #[error("query {query}: backing table {table} does not exist in the schema")]
    UnknownBackingTable { query: String, table: String },

    #[error("query {query}: backing column {column} does not exist on table {table}")]
    UnknownBackingColumn {
        query: String,
        table: String,
        column: String,
    },

    #[error("query {query}: backing index {index} does not exist on table {table}")]
    UnknownBackingIndex {
        query: String,
        table: String,
        index: String,
    },

    #[error(
        "query {query}: the {kind} backing on {table}.{column} names the index \
         {index}, which does not hold {column}; an index answers a search only \
         over the columns it covers"
    )]
    BackingIndexElsewhere {
        query: String,
        table: String,
        column: String,
        kind: SearchKind,
        index: String,
    },

    #[error(
        "query {query}: the {kind} backing on {table}.{column} rests on the \
         {} index {index}, which cannot answer it; a lexical backing needs a \
         FULLTEXT index over the column and a vector backing needs an HNSW, \
         MTREE, or DISKANN one, so back the query with one of those, or drop \
         the backing",
        index_kind_word(*.index_type)
    )]
    WrongBackingIndexType {
        query: String,
        table: String,
        column: String,
        kind: SearchKind,
        index: String,
        index_type: IndexType,
    },

    #[error(
        "query {query}: performs a {kind} search and names no {kind} backing; a \
         declared search with nothing behind it is the table scan this gate exists \
         to refuse — name the table, column, and index that answer it, or stop \
         declaring the search"
    )]
    UnbackedSearch { query: String, kind: SearchKind },

    #[error(
        "query {query}: the vector backing on {table}.{column} searches at \
         {declared} dimensions and {index} is defined over {}; a vector of the \
         wrong width is not a slower search, it is a different one, so pin the \
         backing to the index's width or redefine the index at the width the \
         search sends",
        width_word(*.actual)
    )]
    BackingWidthMismatch {
        query: String,
        table: String,
        column: String,
        index: String,
        declared: u32,
        /// The index's own `DIMENSION`, absent on a definition that
        /// states none.
        actual: Option<u32>,
    },

    #[error(
        "resource {resource}: the server binds {} on every read of {table}, \
         and no index leads with any of them; the plain listing, nothing \
         filtered and nothing sorted, scans the whole table. Lead an index \
         with one of these columns, or reach the rows through a parent key \
         that leads one",
        .bound.join(", ")
    )]
    UnreachableListing {
        resource: String,
        table: String,
        /// The server-bound columns that exist on the table: the pins,
        /// and for a sub-resource the parent key.
        bound: Vec<String>,
    },
}

/// How a violation says what an index is. `DEFINE INDEX` has a
/// keyword for every kind except the plain b-tree, whose keyword IS
/// `INDEX`, and "the INDEX index" reads like a typo where "the
/// FULLTEXT index" reads like a diagnosis.
fn index_kind_word(index_type: IndexType) -> &'static str {
    match index_type {
        IndexType::Standard => "plain",
        other => other.as_str(),
    }
}

/// How a violation says what width an index was defined over. A
/// vector index always states one; a definition that does not is a
/// hand-built oddity, and saying so beats printing `None`.
fn width_word(dimension: Option<u32>) -> String {
    dimension.map_or_else(|| "no stated width".to_owned(), |width| width.to_string())
}

/// Validate a contract against schema definitions; empty means valid.
pub fn validate(contract: &Contract, schema: &[TableDefinition]) -> Vec<Violation> {
    let mut violations = Vec::new();
    validate_api_prefix(contract, &mut violations);
    validate_client_methods(contract, &mut violations);
    // Two resources under one name would claim the same REST prefix
    // and the same GraphQL field, and the one written second would
    // win without saying so.
    let mut resource_names = std::collections::BTreeSet::new();
    for resource in &contract.resources {
        if !resource_names.insert(resource.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: resource.name.clone(),
                problem: "duplicate resource".into(),
            });
        }
        validate_names(resource, &mut violations);
        validate_faces(resource, &mut violations);
        validate_filter_options(resource, &mut violations);
        match schema.iter().find(|t| t.name == resource.table) {
            Some(table) => validate_resource(resource, table, &mut violations),
            None => violations.push(Violation::UnknownTable {
                resource: resource.name.clone(),
                table: resource.table.clone(),
            }),
        }
    }

    // Rate classes: names sound and unique, references resolvable. A
    // reference to a class the contract never defines would meter
    // against a budget nobody wrote down.
    let mut class_names = std::collections::BTreeSet::new();
    for class in &contract.rate_classes {
        if !is_wire_ident(&class.name, false) {
            violations.push(Violation::InvalidName {
                scope: format!("rate class {}", class.name),
                name: class.name.clone(),
                problem: "must be lowercase snake case".into(),
            });
        }
        if !class_names.insert(class.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("rate class {}", class.name),
                name: class.name.clone(),
                problem: "duplicate rate class".into(),
            });
        }
    }
    let class_exists = |name: &str| contract.rate_classes.iter().any(|c| c.name == name);
    for resource in &contract.resources {
        if let Some(class) = &resource.rate_class {
            if !class_exists(class) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}", resource.name),
                    name: class.clone(),
                    problem: "references an undefined rate class".into(),
                });
            }
        }
        for action in &resource.actions {
            if let Some(class) = &action.rate_class {
                if !class_exists(class) {
                    violations.push(Violation::InvalidName {
                        scope: format!("resource {} action {}", resource.name, action.name),
                        name: class.clone(),
                        problem: "references an undefined rate class".into(),
                    });
                }
            }
        }
    }

    // GraphQL type names are schema-global; two surfaces landing on
    // the same effective type name would shadow each other.
    let mut type_names = std::collections::BTreeMap::new();
    for resource in &contract.resources {
        if let Some(previous) =
            type_names.insert(resource.graphql_type_name(), resource.name.clone())
        {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: resource.graphql_type_name(),
                problem: format!("collides with the GraphQL type name of resource {previous}"),
            });
        }
        let mut sub_names = std::collections::BTreeSet::new();
        for sub in &resource.sub_resources {
            validate_sub_resource(resource, sub, schema, &mut violations);
            if !sub_names.insert(sub.name.clone()) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}", resource.name),
                    name: sub.name.clone(),
                    problem: "duplicate sub-resource name".into(),
                });
            }
            let owner = format!("{}.{}", resource.name, sub.name);
            if let Some(previous) = type_names.insert(sub.graphql_type_name(resource), owner) {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}.{}", resource.name, sub.name),
                    name: sub.graphql_type_name(resource),
                    problem: format!("collides with the GraphQL type name of {previous}"),
                });
            }
        }
    }
    // Queries: names sound and unique, rate classes resolvable, and
    // paths absolute. A query is a wire surface like any other, so it
    // answers to the same naming rules.
    let mut query_names = std::collections::BTreeSet::new();
    for query in &contract.queries {
        if !is_wire_ident(&query.name, false) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.name.clone(),
                problem: "must be lowercase snake case".into(),
            });
        }
        if !query_names.insert(query.name.clone()) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.name.clone(),
                problem: "duplicate query".into(),
            });
        }
        if !query.path.starts_with('/') {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: query.path.clone(),
                problem: "path must be absolute".into(),
            });
        }
        if let Some(class) = &query.rate_class {
            if !class_exists(class) {
                violations.push(Violation::InvalidName {
                    scope: format!("query {}", query.name),
                    name: class.clone(),
                    problem: "references an undefined rate class".into(),
                });
            }
        }
        for field in &query.input {
            if !is_wire_ident(&field.name, false) {
                violations.push(Violation::InvalidName {
                    scope: format!("query {} input", query.name),
                    name: field.name.clone(),
                    problem: "must be lowercase snake case".into(),
                });
            }
        }
        validate_searches(query, &mut violations);
        validate_backing(query, schema, &mut violations);
    }

    violations
}

/// The declared-search rule: what the query says it does has to have
/// something behind it.
///
/// Every other rule in this file starts from something the author
/// wrote — a filterable column, a sort, a backing — and holds it to
/// the schema. This one starts from an absence, because an absence is
/// how unindexed search reaches production: nobody writes down that
/// the neighbour search has no index, they simply write the resolver.
/// So the contract is given a way to state the capability, and the
/// statement is what the gate can then refuse. It costs a line to
/// declare and turns a whole class of silent table scan into a build
/// failure; what it cannot do is make an author declare, which is the
/// same limit every claim in a declaration language has.
/// Whether a resource ever renders its row type.
///
/// Either collection face does, and so does an action answering with
/// the resource. A resource with none of those exists to carry verbs
/// and sub-resources, and has nothing of its own to project.
fn projects_rows(resource: &Resource) -> bool {
    !resource.faces.is_none()
        || resource
            .actions
            .iter()
            .any(|action| matches!(action.output, crate::ir::ActionOutput::Resource))
}

/// A resource that exposes no listing cannot honour the declarations
/// that only a listing has: what may be filtered, what may be sorted,
/// and how large a page may be are claims about an endpoint that does
/// not exist. Refusing them is the same rule as refusing a filter no
/// index serves -- a claim that cannot be true should not survive to
/// the artifacts, where a reader would take it for a promise.
///
/// A resource with no face at all and nothing hanging off it is also
/// refused: it produces no output, so it is a declaration that does
/// nothing, which is more likely a mistake than an intent.
fn validate_faces(resource: &Resource, violations: &mut Vec<Violation>) {
    if !resource.faces.list {
        for (what, empty) in [
            ("filterable", resource.filterable.is_empty()),
            ("sortable", resource.sortable.is_empty()),
            ("filter_options", resource.filter_options.is_empty()),
        ] {
            if !empty {
                violations.push(Violation::InvalidName {
                    scope: format!("resource {}", resource.name),
                    name: what.to_owned(),
                    problem: "declared on a resource with no listing".into(),
                });
            }
        }
        if resource.watchable {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: "watchable".into(),
                problem: "a watch streams the listing, which this resource does not expose".into(),
            });
        }
    }
    if resource.faces.is_none() && resource.actions.is_empty() && resource.sub_resources.is_empty()
    {
        violations.push(Violation::InvalidName {
            scope: format!("resource {}", resource.name),
            name: resource.name.clone(),
            problem: "exposes no listing, no getter, no action and no sub-resource".into(),
        });
    }
}

/// The prefix is pasted straight in front of every resource route, so
/// a malformed one produces paths that are wrong in every artifact at
/// once and wrong in the same way -- which is exactly the kind of
/// mistake that looks deliberate on review.
fn validate_api_prefix(contract: &Contract, violations: &mut Vec<Violation>) {
    let prefix = &contract.api_prefix;
    let problem = if prefix.is_empty() || prefix == "/" {
        // The root is a legitimate choice: a service whose routes are
        // already `/accounts` says so with an empty prefix.
        None
    } else if !prefix.starts_with('/') {
        Some("must start with '/'")
    } else if prefix.contains("//") {
        Some("contains an empty path segment")
    } else if prefix.contains(char::is_whitespace) {
        Some("contains whitespace")
    } else {
        None
    };
    if let Some(problem) = problem {
        violations.push(Violation::InvalidName {
            scope: "contract".into(),
            name: prefix.clone(),
            problem: format!("api_prefix {problem}"),
        });
    }
}

/// Every operation in the contract becomes a method on one generated
/// `Client`, so two that derive the same name emit a duplicate method
/// in the same impl. The clients cannot resolve that -- they would
/// simply not compile -- so the contract is refused instead, naming
/// both sides of the collision.
fn validate_client_methods(contract: &Contract, violations: &mut Vec<Violation>) {
    let mut claimed: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for method in crate::methods::client_methods(contract) {
        match claimed.get(&method.name) {
            Some(first) => violations.push(Violation::DuplicateMethod {
                name: method.name.clone(),
                first: first.clone(),
                second: method.source,
            }),
            None => {
                claimed.insert(method.name, method.source);
            }
        }
    }
}

fn validate_searches(query: &Query, violations: &mut Vec<Violation>) {
    let mut declared = std::collections::BTreeSet::new();
    for kind in &query.searches {
        if !declared.insert(*kind) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: kind.as_str().to_owned(),
                problem: "declares the same search twice".into(),
            });
            continue;
        }
        if !query.backing.iter().any(|backing| backing.kind == *kind) {
            violations.push(Violation::UnbackedSearch {
                query: query.name.clone(),
                kind: *kind,
            });
        }
    }
}

/// The backing rulebook: the mirror image of the listing index rules.
///
/// A filter claim needs an index that can narrow an equality, and the
/// WrongIndexType rule refuses one resting on a FULLTEXT index. A
/// backing claim needs the FULLTEXT (or vector) index, and this rule
/// refuses one resting on a b-tree — the same mistake with the two
/// index families swapped, so it gets the same treatment: name the
/// index the author was looking at, say what it turned out to be, and
/// say what the claim actually needs. Unknown names are reported and
/// sit the capability question out, because repairing the name comes
/// first; a resolved column and a resolved index answer for coverage
/// and kind independently of one another.
///
/// An optional backing relaxes exactly one of these rules — the index
/// may be absent — and no others. A deployment that configured the
/// machinery is not a deployment that gets to configure it wrong, so
/// an index that IS there answers for its column, its kind, and its
/// width the way any other does.
fn validate_backing(query: &Query, schema: &[TableDefinition], violations: &mut Vec<Violation>) {
    let mut seen = std::collections::BTreeSet::new();
    for backing in &query.backing {
        // Two identical backings would promise the same thing twice
        // and make every later diff of them ambiguous.
        if !seen.insert((
            backing.table.clone(),
            backing.column.clone(),
            backing.index.clone(),
            backing.kind.as_str(),
        )) {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: backing.index.clone(),
                problem: "declares the same backing twice".into(),
            });
            continue;
        }
        // A width is a vector's business. On a lexical backing it is
        // not a wrong number, it is a number about nothing, and left
        // to the width rule below it would be reported as a mismatch
        // against a FULLTEXT index that was never going to state one.
        if backing.kind == SearchKind::Lexical && backing.dimension.is_some() {
            violations.push(Violation::InvalidName {
                scope: format!("query {}", query.name),
                name: backing.index.clone(),
                problem: "a lexical backing has no vector width".into(),
            });
        }
        let Some(table) = schema.iter().find(|t| t.name == backing.table) else {
            violations.push(Violation::UnknownBackingTable {
                query: query.name.clone(),
                table: backing.table.clone(),
            });
            continue;
        };
        if !table.fields.iter().any(|f| f.name == backing.column) {
            violations.push(Violation::UnknownBackingColumn {
                query: query.name.clone(),
                table: backing.table.clone(),
                column: backing.column.clone(),
            });
            continue;
        }
        let Some(index) = table.indexes.iter().find(|i| i.name == backing.index) else {
            // The one rule optional relaxes: a deployment that never
            // configured the machinery is not a contract that lied.
            if backing.optional {
                continue;
            }
            violations.push(Violation::UnknownBackingIndex {
                query: query.name.clone(),
                table: backing.table.clone(),
                index: backing.index.clone(),
            });
            continue;
        };
        if !index.columns.iter().any(|c| c == &backing.column) {
            violations.push(Violation::BackingIndexElsewhere {
                query: query.name.clone(),
                table: backing.table.clone(),
                column: backing.column.clone(),
                kind: backing.kind,
                index: backing.index.clone(),
            });
            continue;
        }
        if !serves_search(index, backing.kind) {
            violations.push(Violation::WrongBackingIndexType {
                query: query.name.clone(),
                table: backing.table.clone(),
                column: backing.column.clone(),
                kind: backing.kind,
                index: backing.index.clone(),
                index_type: index.index_type,
            });
            continue;
        }
        // Width, last, and only once the index is known to be vector
        // machinery over the right column: a width mismatch reported
        // against an index that was the wrong kind to begin with
        // sends the author to fix the smaller of two problems. The
        // definition's own `DIMENSION` is read here rather than
        // through `crate::indexes`, because it is a field, not an
        // interpretation of one — nothing about it can drift between
        // readers the way index-type reasoning did.
        if let (SearchKind::Vector, Some(declared)) = (backing.kind, backing.dimension) {
            if index.dimension != Some(declared) {
                violations.push(Violation::BackingWidthMismatch {
                    query: query.name.clone(),
                    table: backing.table.clone(),
                    column: backing.column.clone(),
                    index: backing.index.clone(),
                    declared,
                    actual: index.dimension,
                });
            }
        }
    }
}

/// The index an author most likely mistook for coverage: one that holds
/// `column` and cannot order it, reported only when nothing that CAN
/// order it holds it at all.
///
/// The distinction matters because the two mistakes want different
/// repairs. Where a standard index does hold the column, the index type
/// is not what went wrong, and saying it was would send the author off
/// to define an index they already have; the prefix is the problem, and
/// [`Violation::UnindexedSort`] already explains prefixes.
fn misleading_index<'a>(table: &'a TableDefinition, column: &str) -> Option<&'a IndexDefinition> {
    let holds = |index: &IndexDefinition| index.columns.iter().any(|c| c == column);
    if table
        .indexes
        .iter()
        .any(|index| serves_ordering(index) && holds(index))
    {
        return None;
    }
    table.indexes.iter().find(|index| holds(index))
}

/// The index rulebook, applied identically to a resource and to a
/// sub-resource. Only the scope string and the set of server-bound
/// columns differ between them.
fn validate_listing(
    listing: &Listing<'_>,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    let column_exists = |column: &str| table.fields.iter().any(|f| f.name == column);
    let push_unknown = |column: &str, violations: &mut Vec<Violation>| {
        violations.push(Violation::UnknownColumn {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.to_owned(),
        });
    };

    if listing.projects && listing.fields.is_empty() {
        violations.push(Violation::NoFields {
            resource: listing.scope.clone(),
        });
    }

    let mut seen_api_names = std::collections::BTreeSet::new();
    for exposure in listing.fields {
        if !column_exists(&exposure.column) {
            push_unknown(&exposure.column, violations);
        }
        // Every generated resource type carries an `id` the contract
        // never declares, so an exposure that lands on that name is a
        // duplicate field the author cannot see in their own contract.
        // A rename to `id` is already refused by the reserved gate;
        // this catches the plain column, which is not reserved because
        // column names belong to the schema layer.
        if exposure.api_name() == "id" {
            violations.push(Violation::ShadowsId {
                resource: listing.scope.clone(),
                name: exposure.api_name().to_owned(),
            });
        }
        if !seen_api_names.insert(exposure.api_name().to_owned()) {
            violations.push(Violation::DuplicateApiName {
                resource: listing.scope.clone(),
                name: exposure.api_name().to_owned(),
            });
        }
    }

    let ordering = ordering_indexes(table);
    // An author who reached a claim through an index that turns out not
    // to serve it is in a different position from one who claimed
    // against nothing: they looked, they found something, and they were
    // right about everything except the one property that mattered. The
    // violation says which index they were looking at.
    let wrong_index = |column: &String, claim: &str, violations: &mut Vec<Violation>| {
        let Some(index) = misleading_index(table, column) else {
            return false;
        };
        violations.push(Violation::WrongIndexType {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
            claim: claim.to_owned(),
            index: index.name.clone(),
            index_type: index.index_type,
        });
        true
    };

    for column in listing.filterable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let covered = ordering
            .iter()
            .any(|index| index.columns.iter().any(|c| c == column));
        if covered {
            continue;
        }
        if wrong_index(column, "filterable", violations) {
            continue;
        }
        violations.push(Violation::UnindexedFilter {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
        });
    }

    // Pins are not claims. Filterable describes what a caller MAY send
    // and sortable what they may ask for; the bound columns are what
    // the server DOES, on every read, with nothing optional about it.
    // So the reachability question is not whether a caller can compose
    // a bad query but whether the default one is already bad: list,
    // nothing filtered, nothing sorted. That query seeks only if some
    // ordering index leads with a bound column, and the whole set
    // shares one answer, which is why this is one violation naming all
    // of them rather than one per pin. Columns the table does not have
    // are reported as unknown and sit the reachability question out;
    // repairing the name comes first.
    let mut bound_present: Vec<&str> = Vec::new();
    for column in &listing.bound {
        if column_exists(column) {
            bound_present.push(column);
        } else {
            push_unknown(column, violations);
        }
    }
    if !bound_present.is_empty() && !seekable_through(table, &bound_present) {
        violations.push(Violation::UnreachableListing {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            bound: bound_present.iter().map(|c| (*c).to_owned()).collect(),
        });
    }

    // A column earlier in an index than the sort column must be
    // equality-boundable, or the index cannot serve the ORDER BY.
    let boundable = |column: &str| {
        listing.bound.contains(&column) || listing.filterable.iter().any(|c| c == column)
    };
    for column in listing.sortable {
        if !column_exists(column) {
            push_unknown(column, violations);
            continue;
        }
        let reachable = ordering.iter().any(|index| {
            index
                .columns
                .iter()
                .position(|c| c == column)
                .is_some_and(|k| index.columns[..k].iter().all(|earlier| boundable(earlier)))
        });
        if reachable {
            continue;
        }
        if wrong_index(column, "sortable", violations) {
            continue;
        }
        violations.push(Violation::UnindexedSort {
            resource: listing.scope.clone(),
            table: listing.table.to_owned(),
            column: column.clone(),
        });
    }
}

/// Sub-resources: the same rulebook over the sub table, with
/// `parent_key` credited as server-bound, plus the name checks.
fn validate_sub_resource(
    parent: &Resource,
    sub: &SubResource,
    schema: &[TableDefinition],
    violations: &mut Vec<Violation>,
) {
    let scope = format!("{}.{}", parent.name, sub.name);
    if !is_wire_ident(&sub.name, true) {
        violations.push(Violation::InvalidName {
            scope: format!("resource {scope}"),
            name: sub.name.clone(),
            problem: "must be lowercase snake or kebab case".into(),
        });
    }
    for (label, name) in [
        (
            "graphql.type_name",
            sub.graphql.as_ref().and_then(|g| g.type_name.as_deref()),
        ),
        (
            "graphql.field",
            sub.graphql.as_ref().and_then(|g| g.field.as_deref()),
        ),
    ] {
        let Some(name) = name else { continue };
        if !is_graphql_name(name) || name.starts_with("__") {
            violations.push(Violation::InvalidName {
                scope: format!("resource {scope} {label}"),
                name: name.to_owned(),
                problem: "is not a valid GraphQL name".into(),
            });
        }
        if crate::reserved::is_reserved(name) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {scope} {label}"),
                name: name.to_owned(),
                problem: "collides with a SurrealDB v3 reserved name".into(),
            });
        }
    }

    let Some(table) = schema.iter().find(|t| t.name == sub.table) else {
        violations.push(Violation::UnknownTable {
            resource: scope,
            table: sub.table.clone(),
        });
        return;
    };
    validate_listing(
        &Listing {
            scope,
            table: &sub.table,
            fields: &sub.fields,
            bound: sub.bound_columns(),
            filterable: &sub.filterable,
            sortable: &sub.sortable,
            projects: true,
        },
        table,
        violations,
    );
}

/// `[a-z][a-z0-9_]*`, with `-` also allowed when `kebab` is set.
fn is_wire_ident(name: &str, kebab: bool) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || (kebab && c == '-')
        })
}

/// The GraphQL `Name` grammar: `[_A-Za-z][_0-9A-Za-z]*`.
fn is_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Names the contract author CHOSE must be valid for every surface
/// they reach; names that OVERRIDE something (field renames and
/// GraphQL overrides) additionally must never collide with a
/// SurrealDB v3 reserved name. Action, input, and resource names are
/// exempt from the reserved gate (verbs like `remove` or `update` are
/// legitimate action names and never reach the database); column names
/// belong to the schema layer, which handles its own escaping.
fn validate_names(resource: &Resource, violations: &mut Vec<Violation>) {
    let mut push = |scope: String, name: &str, problem: String| {
        violations.push(Violation::InvalidName {
            scope,
            name: name.to_owned(),
            problem,
        });
    };
    let scope = |suffix: &str| format!("resource {}{suffix}", resource.name);

    if !is_wire_ident(&resource.name, true) {
        push(
            scope(""),
            &resource.name,
            "must be lowercase snake or kebab case".into(),
        );
    }

    for exposure in &resource.fields {
        if let Some(guard) = &exposure.guard {
            if !is_wire_ident(guard, false) {
                push(
                    scope(""),
                    guard,
                    "guard names must be lowercase snake case".into(),
                );
            }
        }
        if let Some(rename) = &exposure.rename {
            if !is_wire_ident(rename, false) {
                push(
                    scope(""),
                    rename,
                    "rename must be lowercase snake case".into(),
                );
            }
            if crate::reserved::is_reserved(rename) {
                push(
                    scope(""),
                    rename,
                    "rename collides with a SurrealDB v3 reserved name".into(),
                );
            }
        }
    }

    let graphql_overrides = resource.graphql.as_ref().map_or(Vec::new(), |g| {
        [
            ("graphql.type_name", g.type_name.as_deref()),
            ("graphql.list_field", g.list_field.as_deref()),
            ("graphql.get_field", g.get_field.as_deref()),
            ("graphql.watch_field", g.watch_field.as_deref()),
        ]
        .into_iter()
        .filter_map(|(label, name)| name.map(|n| (label, n)))
        .collect()
    });
    for (label, name) in graphql_overrides {
        if !is_graphql_name(name) {
            push(
                scope(&format!(" {label}")),
                name,
                "is not a valid GraphQL name".into(),
            );
        }
        if name.starts_with("__") {
            push(
                scope(&format!(" {label}")),
                name,
                "GraphQL reserves the __ prefix for introspection".into(),
            );
        }
        if matches!(name, "Query" | "Mutation" | "Subscription") {
            push(
                scope(&format!(" {label}")),
                name,
                "collides with a GraphQL root type name".into(),
            );
        }
        if crate::reserved::is_reserved(name) {
            push(
                scope(&format!(" {label}")),
                name,
                "collides with a SurrealDB v3 reserved name".into(),
            );
        }
    }

    for required in &resource.reads_require {
        if !is_wire_ident(required, false) {
            push(
                scope(" reads_require"),
                required,
                "scope names must be lowercase snake case".into(),
            );
        }
    }

    for action in &resource.actions {
        let action_scope = || scope(&format!(" action {}", action.name));
        if !action.name.is_empty() && !is_wire_ident(&action.name, false) {
            push(
                action_scope(),
                &action.name,
                "must be lowercase snake case".into(),
            );
        }
        if let Some(field) = &action.graphql_field {
            if !is_graphql_name(field) || field.starts_with("__") {
                push(
                    action_scope(),
                    field,
                    "graphql_field is not a valid GraphQL name".into(),
                );
            }
            if crate::reserved::is_reserved(field) {
                push(
                    action_scope(),
                    field,
                    "graphql_field collides with a SurrealDB v3 reserved name".into(),
                );
            }
        }
        for input in &action.input {
            if !input.name.is_empty() && !is_wire_ident(&input.name, false) {
                push(
                    action_scope(),
                    &input.name,
                    "input name must be lowercase snake case".into(),
                );
            }
        }
        for required in &action.requires {
            if !is_wire_ident(required, false) {
                push(
                    action_scope(),
                    required,
                    "scope names must be lowercase snake case".into(),
                );
            }
        }
    }
}

/// Filter options describe columns a caller may narrow by, so a key
/// that is not filterable describes nothing and would render a menu
/// beside a filter the server refuses.
fn validate_filter_options(resource: &Resource, violations: &mut Vec<Violation>) {
    for (column, options) in &resource.filter_options {
        if !resource.filterable.iter().any(|f| f == column) {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: column.clone(),
                problem: "filter options name a column that is not filterable".into(),
            });
        }
        if options.is_empty() {
            violations.push(Violation::InvalidName {
                scope: format!("resource {}", resource.name),
                name: column.clone(),
                problem: "filter options are empty, which offers a menu of nothing".into(),
            });
        }
    }
}

/// The shape rules for an either-of pin, before its branches are
/// checked as listings.
fn validate_either_pins(
    resource: &Resource,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    let invalid = |name: &str, text: &str| Violation::InvalidName {
        scope: format!("resource {}", resource.name),
        name: name.to_owned(),
        problem: text.to_owned(),
    };
    if resource.pinned_either.len() < 2 {
        violations.push(invalid(
            "pinned_either",
            "names fewer than two columns; one alternative is a pin, and              saying it this way only hides that",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for column in &resource.pinned_either {
        if !table.fields.iter().any(|field| &field.name == column) {
            violations.push(Violation::UnknownColumn {
                resource: resource.name.clone(),
                table: resource.table.clone(),
                column: column.clone(),
            });
        }
        if resource.pinned.contains(column) {
            violations.push(invalid(
                column,
                "is both pinned and an either-of alternative; a column the                  server always binds is not a choice between branches",
            ));
        }
        if !seen.insert(column) {
            violations.push(invalid(
                column,
                "is named twice among the either-of alternatives",
            ));
        }
    }
}

fn validate_resource(
    resource: &Resource,
    table: &TableDefinition,
    violations: &mut Vec<Violation>,
) {
    // An identity naming a column the table does not have would emit a
    // field the service can never send, in every artifact at once. That
    // is the failure this whole option exists to prevent, so it is
    // checked here rather than discovered by a client that cannot parse
    // a successful response.
    if let Some(identity) = &resource.identity {
        if !table.fields.iter().any(|field| &field.name == identity) {
            violations.push(Violation::UnknownColumn {
                resource: resource.name.clone(),
                table: resource.table.clone(),
                column: identity.clone(),
            });
        }
    }
    let pinned: Vec<&str> = resource.pinned.iter().map(String::as_str).collect();
    let projects = projects_rows(resource);
    if resource.pinned_either.is_empty() {
        validate_listing(
            &Listing {
                scope: resource.name.clone(),
                table: &resource.table,
                fields: &resource.fields,
                bound: pinned,
                filterable: &resource.filterable,
                sortable: &resource.sortable,
                projects,
            },
            table,
            violations,
        );
    } else {
        // The engine answers a disjunction as a union of one seek per
        // branch, so EVERY branch has to be servable: one without an
        // index behind it drags the whole read back to a scan. Each
        // alternative is checked as though it were an ordinary pin,
        // which is exactly what it is within its own branch.
        validate_either_pins(resource, table, violations);
        for alternative in &resource.pinned_either {
            let mut bound = pinned.clone();
            bound.push(alternative);
            validate_listing(
                &Listing {
                    scope: format!("{} (pinned on {alternative})", resource.name),
                    table: &resource.table,
                    fields: &resource.fields,
                    bound,
                    filterable: &resource.filterable,
                    sortable: &resource.sortable,
                    projects,
                },
                table,
                violations,
            );
        }
    }

    let mut action_names = std::collections::BTreeSet::new();
    for action in &resource.actions {
        let mut problem = |text: String| {
            violations.push(Violation::InvalidAction {
                resource: resource.name.clone(),
                action: action.name.clone(),
                problem: text,
            });
        };
        if action.name.is_empty() {
            problem("empty action name".into());
        }
        if !action_names.insert(action.name.clone()) {
            problem("duplicate action name".into());
        }
        if !matches!(action.method.as_str(), "POST" | "PUT" | "DELETE" | "PATCH") {
            problem(format!(
                "method must be POST, PUT, DELETE, or PATCH, not {:?}",
                action.method,
            ));
        }
        if !action.path.is_empty() && !action.path.starts_with('/') {
            problem(format!("path {:?} must start with '/'", action.path));
        }
        let mut input_names = std::collections::BTreeSet::new();
        for field in &action.input {
            if field.name.is_empty() {
                problem("empty input field name".into());
            }
            if !input_names.insert(field.name.clone()) {
                problem(format!("duplicate input field {:?}", field.name));
            }
            // A closed set of one repeated value refuses inputs by
            // accident, and a set on a field whose type cannot hold a
            // string never matches anything.
            let mut seen = std::collections::BTreeSet::new();
            for option in &field.options {
                if !seen.insert(option) {
                    problem(format!(
                        "input {:?} lists option {option:?} twice",
                        field.name,
                    ));
                }
            }
            if field.multiple && field.options.is_empty() {
                problem(format!(
                    "input {:?} takes several values but lists none",
                    field.name,
                ));
            }
            if !field.options.is_empty() && field.kind != crate::ir::TypeRef::String {
                problem(format!(
                    "input {:?} lists options but is not a string",
                    field.name,
                ));
            }
        }
    }
}

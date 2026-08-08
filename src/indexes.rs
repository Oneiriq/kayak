//! What an index is actually able to answer.
//!
//! SurrealDB spells five kinds of index with one `DEFINE INDEX`, and
//! only two of them have a b-tree behind them. FULLTEXT answers `@@`
//! against an analyzer's terms; HNSW and MTREE answer nearest-neighbour
//! over a vector. None of the three narrows an equality or supplies an
//! order, so a column they cover is, as far as a filter or an ORDER BY
//! is concerned, uncovered.
//!
//! This sits apart from both readers because it had two of them and
//! they disagreed. The scaffold filtered on index type; the validator
//! matched on column membership alone. That is the worse half to get
//! wrong: the generated contract was careful and the hand-written one
//! was believed, so a claim resting on a BM25 index passed the gate and
//! scanned the table in production. One predicate, read by both, is the
//! only arrangement in which they cannot drift apart again.

use surql::schema::{IndexDefinition, IndexType, TableDefinition};

/// Whether an index can narrow an equality and supply an order.
pub(crate) fn serves_ordering(index: &IndexDefinition) -> bool {
    matches!(index.index_type, IndexType::Standard | IndexType::Unique)
}

/// The indexes on `table` that can, in declaration order.
pub(crate) fn ordering_indexes(table: &TableDefinition) -> Vec<&IndexDefinition> {
    table
        .indexes
        .iter()
        .filter(|index| serves_ordering(index))
        .collect()
}

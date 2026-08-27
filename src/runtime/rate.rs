//! Rate limiting at the one layer that can meter GraphQL accurately.
//!
//! A proxy counts requests, and one POST can carry many aliased
//! operations, so proxy counting undercounts exactly the face that is
//! cheapest to amplify. The dispatcher sees every operation on every
//! face, which makes it the honest metering point.
//!
//! Consumption is charged in units rather than requests: a listing
//! costs its row limit, everything else costs one. A caller that asks
//! for hundred-row pages spends its budget a hundred times faster
//! than one probing single rows, which is the proportionality a
//! per-request count cannot express.
//!
//! The store is pluggable because the right ledger depends on the
//! deployment: the in-memory store meters one process; a multi-node
//! deployment brings a shared counter by implementing [`RateStore`]
//! over its own database.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runtime::error::KayakError;
use crate::runtime::resolvers::BoxFuture;

/// A consumption ledger. `charge` adds `units` against `bucket` for
/// the current window and answers whether the budget still holds;
/// refusals must not record the units, so a throttled caller is not
/// pushed further into debt by its own refusals.
pub trait RateStore: Send + Sync {
    fn charge<'a>(
        &'a self,
        bucket: &'a str,
        units: u64,
        per_minute: u64,
    ) -> BoxFuture<'a, Result<bool, KayakError>>;
}

/// A single-process ledger over fixed one-minute windows.
///
/// Fixed windows admit up to double the budget across a boundary,
/// which is documented rather than hidden; the tradeoff buys a ledger
/// with no background task and no clock arithmetic a reader has to
/// verify. Multi-node deployments meter in their shared database
/// instead.
#[derive(Default)]
pub struct MemoryRateStore {
    windows: Mutex<HashMap<String, (u64, u64)>>,
}

impl MemoryRateStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn minute() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 60)
            .unwrap_or(0)
    }
}

impl RateStore for MemoryRateStore {
    fn charge<'a>(
        &'a self,
        bucket: &'a str,
        units: u64,
        per_minute: u64,
    ) -> BoxFuture<'a, Result<bool, KayakError>> {
        Box::pin(async move {
            let minute = Self::minute();
            let mut windows = self
                .windows
                .lock()
                .map_err(|_| KayakError::Internal("rate ledger poisoned".into()))?;
            let entry = windows.entry(bucket.to_owned()).or_insert((minute, 0));
            if entry.0 != minute {
                *entry = (minute, 0);
            }
            if entry.1 + units > per_minute {
                return Ok(false);
            }
            entry.1 += units;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn budgets_hold_within_a_window_and_buckets_are_separate() {
        let store = MemoryRateStore::new();
        assert!(store.charge("a", 6, 10).await.unwrap());
        assert!(store.charge("a", 4, 10).await.unwrap());
        // The refusal spends nothing: the budget is exactly used up,
        // and a subsequent charge that fits still passes.
        assert!(!store.charge("a", 1, 10).await.unwrap());
        assert!(!store.charge("a", 1, 10).await.unwrap());
        assert!(store.charge("a", 0, 10).await.unwrap());
        // Another bucket has its own ledger.
        assert!(store.charge("b", 10, 10).await.unwrap());
    }

    #[tokio::test]
    async fn an_oversized_single_charge_refuses_outright() {
        let store = MemoryRateStore::new();
        assert!(!store.charge("a", 11, 10).await.unwrap());
    }
}

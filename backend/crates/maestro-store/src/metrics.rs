//! Near-zero-overhead operation latency recorder.
//!
//! Each op name owns a fixed array of log2 buckets of microseconds backed by
//! `AtomicU64`s: recording a sample is a read-lock map lookup plus three
//! relaxed atomic ops. `snapshot()` estimates p50/p99 from the buckets
//! (upper-bound estimate, clamped to the exact observed max).

use maestro_types::api::OpMetrics;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

/// Bucket `i` counts samples with `micros in [2^i, 2^(i+1))` (bucket 0 also
/// takes 0µs). 40 buckets cover up to ~2^40 µs ≈ 12.7 days — far beyond any op.
const NUM_BUCKETS: usize = 40;

struct OpHist {
    buckets: [AtomicU64; NUM_BUCKETS],
    count: AtomicU64,
    max: AtomicU64,
}

impl OpHist {
    fn new() -> Self {
        OpHist {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            max: AtomicU64::new(0),
        }
    }
}

/// Per-store latency recorder. Cheap to share (interior `Arc`s), safe to call
/// from any thread.
pub struct Metrics {
    ops: RwLock<HashMap<&'static str, Arc<OpHist>>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

fn bucket_index(micros: u64) -> usize {
    // floor(log2(max(micros, 1))), clamped to the last bucket.
    let idx = (63 - (micros | 1).leading_zeros()) as usize;
    idx.min(NUM_BUCKETS - 1)
}

impl Metrics {
    pub fn new() -> Self {
        Metrics {
            ops: RwLock::new(HashMap::new()),
        }
    }

    fn hist(&self, op: &'static str) -> Arc<OpHist> {
        if let Some(h) = self
            .ops
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(op)
        {
            return Arc::clone(h);
        }
        let mut map = self.ops.write().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry(op).or_insert_with(|| Arc::new(OpHist::new())))
    }

    /// Record one sample of `micros` microseconds for `op`.
    pub fn record(&self, op: &'static str, micros: u64) {
        let h = self.hist(op);
        h.buckets[bucket_index(micros)].fetch_add(1, Ordering::Relaxed);
        h.count.fetch_add(1, Ordering::Relaxed);
        h.max.fetch_max(micros, Ordering::Relaxed);
    }

    /// Time `f` and record its wall-clock duration under `op`.
    pub fn timed<T>(&self, op: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(op, start.elapsed().as_micros() as u64);
        out
    }

    /// Percentile snapshot per op, sorted by op name. Percentiles are
    /// estimated as the upper bound of the containing log2 bucket, clamped to
    /// the exact observed max, so `p50 <= p99 <= max` always holds.
    pub fn snapshot(&self) -> Vec<OpMetrics> {
        let map = self.ops.read().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<OpMetrics> = map
            .iter()
            .map(|(op, h)| {
                let counts: Vec<u64> = h
                    .buckets
                    .iter()
                    .map(|b| b.load(Ordering::Relaxed))
                    .collect();
                let count: u64 = counts.iter().sum();
                let max = h.max.load(Ordering::Relaxed);
                OpMetrics {
                    op: (*op).to_string(),
                    count: count as i64,
                    p50_us: percentile(&counts, count, 0.50, max) as i64,
                    p99_us: percentile(&counts, count, 0.99, max) as i64,
                    max_us: max as i64,
                }
            })
            .collect();
        out.sort_by(|a, b| a.op.cmp(&b.op));
        out
    }
}

fn percentile(counts: &[u64], total: u64, q: f64, max: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    // Rank of the q-th sample (1-based), at least 1.
    let rank = ((q * total as f64).ceil() as u64).max(1);
    let mut cum = 0u64;
    for (i, c) in counts.iter().enumerate() {
        cum += c;
        if cum >= rank {
            // Upper bound of bucket i is 2^(i+1) - 1; clamp to the true max.
            let upper = if i + 1 >= 64 {
                u64::MAX
            } else {
                (1u64 << (i + 1)) - 1
            };
            return upper.min(max);
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_and_percentiles() {
        let m = Metrics::new();
        for v in [0u64, 1, 3, 7, 100, 100, 100, 5000] {
            m.record("op.a", v);
        }
        let snap = m.snapshot();
        assert_eq!(snap.len(), 1);
        let a = &snap[0];
        assert_eq!(a.op, "op.a");
        assert_eq!(a.count, 8);
        assert!(a.p50_us <= a.p99_us, "p50 {} > p99 {}", a.p50_us, a.p99_us);
        assert!(a.p99_us <= a.max_us);
        assert_eq!(a.max_us, 5000);
    }

    #[test]
    fn timed_records() {
        let m = Metrics::new();
        let x = m.timed("op.t", || 42);
        assert_eq!(x, 42);
        let snap = m.snapshot();
        assert_eq!(snap[0].count, 1);
    }

    #[test]
    fn bucket_index_edges() {
        assert_eq!(bucket_index(0), 0);
        assert_eq!(bucket_index(1), 0);
        assert_eq!(bucket_index(2), 1);
        assert_eq!(bucket_index(3), 1);
        assert_eq!(bucket_index(4), 2);
        assert_eq!(bucket_index(u64::MAX), NUM_BUCKETS - 1);
    }
}

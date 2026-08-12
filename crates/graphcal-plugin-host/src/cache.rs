//! Bounded single-flight LRU cache used by the plugin host.
//!
//! This module is the functional cache core: it knows only keys, cloneable
//! outcomes, encoded-byte charges, and recency. WebAssembly compilation stays
//! in the host shell.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, OnceLock};

/// Process-level limits for cached plugin load outcomes.
///
/// Every success occupies one entry, which bounds the number of Wasmi compiled
/// modules independently of their encoded byte size. Every success and failure
/// is also charged its encoded module bytes. Fields are private so adding a new
/// cache resource cannot be omitted by struct literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginCacheLimits {
    max_entries: usize,
    max_encoded_bytes: usize,
}

impl PluginCacheLimits {
    /// Construct a complete cache policy. Zero is a valid deny-cache value.
    #[must_use]
    pub const fn new(max_entries: usize, max_encoded_bytes: usize) -> Self {
        Self {
            max_entries,
            max_encoded_bytes,
        }
    }

    /// Maximum resident successful and failed load outcomes.
    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.max_entries
    }

    /// Maximum aggregate encoded bytes charged to resident outcomes.
    #[must_use]
    pub const fn max_encoded_bytes(self) -> usize {
        self.max_encoded_bytes
    }
}

impl Default for PluginCacheLimits {
    fn default() -> Self {
        Self::new(32, 64 * 1024 * 1024)
    }
}

/// Result of probing the cache for a key.
pub enum CacheProbe<V> {
    /// A completed positive or negative outcome is resident.
    Ready(V),
    /// One shared cell owns the miss. `OnceLock::get_or_init` elects exactly
    /// one of all concurrent callers to compute it.
    Pending(Arc<OnceLock<V>>),
}

struct ReadyEntry<V> {
    value: V,
    encoded_bytes: usize,
}

struct InFlightEntry<V> {
    cell: Arc<OnceLock<V>>,
    encoded_bytes: usize,
}

/// A bounded LRU of completed outcomes plus transient single-flight cells.
///
/// In-flight cells are not evicted: evicting one could let the same key compile
/// twice concurrently. They are transient and bounded by simultaneous unique
/// calls into the synchronous host API. Only completed entries count against
/// the persistent policy.
pub struct SingleFlightLruCache<K, V> {
    limits: PluginCacheLimits,
    ready: HashMap<K, ReadyEntry<V>>,
    /// Least-recently used at the front, most-recently used at the back.
    recency: VecDeque<K>,
    resident_encoded_bytes: usize,
    in_flight: HashMap<K, InFlightEntry<V>>,
}

impl<K, V> SingleFlightLruCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub(crate) fn new(limits: PluginCacheLimits) -> Self {
        Self {
            limits,
            ready: HashMap::new(),
            recency: VecDeque::new(),
            resident_encoded_bytes: 0,
            in_flight: HashMap::new(),
        }
    }

    pub(crate) fn probe(&mut self, key: K, encoded_bytes: usize) -> CacheProbe<V> {
        if let Some(value) = self.ready.get(&key).map(|entry| entry.value.clone()) {
            self.touch(&key);
            return CacheProbe::Ready(value);
        }
        if let Some(entry) = self.in_flight.get(&key) {
            return CacheProbe::Pending(Arc::clone(&entry.cell));
        }
        let cell = Arc::new(OnceLock::new());
        self.in_flight.insert(
            key,
            InFlightEntry {
                cell: Arc::clone(&cell),
                encoded_bytes,
            },
        );
        CacheProbe::Pending(cell)
    }

    /// Promote a completed single-flight cell into the bounded resident LRU.
    ///
    /// Every waiter may call this; only the caller that still finds the exact
    /// in-flight cell performs the promotion.
    pub(crate) fn complete(&mut self, key: &K, cell: &Arc<OnceLock<V>>, value: &V) {
        if self.ready.contains_key(key) {
            self.touch(key);
            return;
        }
        let Some(in_flight) = self.in_flight.get(key) else {
            return;
        };
        if !Arc::ptr_eq(&in_flight.cell, cell) {
            return;
        }
        let encoded_bytes = in_flight.encoded_bytes;
        self.in_flight.remove(key);
        self.insert_ready(key.clone(), value.clone(), encoded_bytes);
    }

    fn insert_ready(&mut self, key: K, value: V, encoded_bytes: usize) {
        if self.limits.max_entries == 0 || encoded_bytes > self.limits.max_encoded_bytes {
            return;
        }
        while self.ready.len() >= self.limits.max_entries
            || self
                .resident_encoded_bytes
                .checked_add(encoded_bytes)
                .is_none_or(|total| total > self.limits.max_encoded_bytes)
        {
            let Some(oldest) = self.recency.pop_front() else {
                return;
            };
            if let Some(evicted) = self.ready.remove(&oldest) {
                self.resident_encoded_bytes -= evicted.encoded_bytes;
            }
        }
        self.resident_encoded_bytes += encoded_bytes;
        self.recency.push_back(key.clone());
        self.ready.insert(
            key,
            ReadyEntry {
                value,
                encoded_bytes,
            },
        );
    }

    fn touch(&mut self, key: &K) {
        if let Some(position) = self.recency.iter().position(|candidate| candidate == key) {
            self.recency.remove(position);
        }
        self.recency.push_back(key.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(
        cache: &mut SingleFlightLruCache<&'static str, Result<u8, &'static str>>,
        key: &'static str,
        encoded_bytes: usize,
        value: Result<u8, &'static str>,
    ) {
        let CacheProbe::Pending(cell) = cache.probe(key, encoded_bytes) else {
            panic!("test key unexpectedly ready");
        };
        cell.set(value).expect("new test cell is empty");
        cache.complete(&key, &cell, &value);
    }

    #[test]
    fn caches_positive_and_negative_outcomes() {
        let mut cache = SingleFlightLruCache::new(PluginCacheLimits::new(2, 10));
        complete(&mut cache, "ok", 4, Ok(7));
        complete(&mut cache, "error", 5, Err("invalid"));

        assert!(matches!(cache.probe("ok", 4), CacheProbe::Ready(Ok(7))));
        assert!(matches!(
            cache.probe("error", 5),
            CacheProbe::Ready(Err("invalid"))
        ));
    }

    #[test]
    fn evicts_least_recently_used_by_entry_and_byte_budgets() {
        let mut cache = SingleFlightLruCache::new(PluginCacheLimits::new(2, 8));
        complete(&mut cache, "a", 4, Ok(1));
        complete(&mut cache, "b", 4, Ok(2));
        assert!(matches!(cache.probe("a", 4), CacheProbe::Ready(Ok(1))));
        complete(&mut cache, "c", 4, Ok(3));

        assert!(matches!(cache.probe("a", 4), CacheProbe::Ready(Ok(1))));
        assert!(matches!(cache.probe("b", 4), CacheProbe::Pending(_)));

        let mut byte_limited = SingleFlightLruCache::new(PluginCacheLimits::new(3, 5));
        complete(&mut byte_limited, "a", 3, Ok(1));
        complete(&mut byte_limited, "b", 3, Ok(2));
        assert!(matches!(byte_limited.probe("a", 3), CacheProbe::Pending(_)));
        assert!(matches!(
            byte_limited.probe("b", 3),
            CacheProbe::Ready(Ok(2))
        ));
    }

    #[test]
    fn concurrent_probes_share_one_in_flight_cell() {
        let mut cache =
            SingleFlightLruCache::<&str, Result<u8, &str>>::new(PluginCacheLimits::default());
        let CacheProbe::Pending(first) = cache.probe("same", 4) else {
            panic!("first probe must miss");
        };
        let CacheProbe::Pending(second) = cache.probe("same", 4) else {
            panic!("second probe must join the miss");
        };
        assert!(Arc::ptr_eq(&first, &second));
    }
}

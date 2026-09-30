//! The plugin host: one WASM engine plus a bounded single-flight outcome cache.

use std::sync::{Arc, Mutex, PoisonError};

use sha2::Digest as _;

use crate::cache::{CacheProbe, PluginCacheLimits, SingleFlightLruCache};
use crate::limits::PluginLimits;
use crate::module::{PluginLoadError, PluginModule};

type CachedLoadResult = Result<Arc<PluginModule>, PluginLoadError>;

/// Loads, validates, caches, and executes WASM plugin modules.
///
/// Embedders keep one host alive for the process (the language server keeps
/// it across re-evaluations) so that reloading a project hits the bounded
/// positive/negative content-hash cache instead of recompiling modules.
pub struct PluginHost {
    engine: wasmi::Engine,
    limits: PluginLimits,
    cache_limits: PluginCacheLimits,
    cache: Mutex<SingleFlightLruCache<[u8; 32], CachedLoadResult>>,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
            .field("cache_limits", &self.cache_limits)
            .finish_non_exhaustive()
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    /// Create a host with the default [`PluginLimits`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(PluginLimits::default())
    }

    /// Create a host with an explicit per-module policy and default cache policy.
    #[must_use]
    pub fn with_limits(limits: PluginLimits) -> Self {
        Self::with_policies(limits, PluginCacheLimits::default())
    }

    /// Create a host with explicit per-module and process-cache policies.
    #[must_use]
    pub fn with_policies(limits: PluginLimits, cache_limits: PluginCacheLimits) -> Self {
        let mut config = wasmi::Config::default();
        // Fuel metering is the availability bound; mandatory, not optional
        // hardening (issue #25 decision log).
        config.consume_fuel(true);
        // Determinism note: this build of wasmi has no SIMD support, so the
        // relaxed-SIMD proposal (implementation-defined results) cannot be
        // reached. If the `simd` feature is ever enabled, relaxed SIMD must
        // be disabled here — plugin results are required to be
        // bit-identical across platforms.
        // Compile eagerly so invalid function bodies fail at load time
        // rather than mid-evaluation.
        config.compilation_mode(wasmi::CompilationMode::Eager);
        // Parsing and eager translation happen before call fuel exists. Wasmi's
        // strict policy bounds structural counts and adversarial tiny-function
        // amplification during this untrusted-input phase.
        config.enforced_limits(wasmi::EnforcedLimits::strict());
        Self {
            engine: wasmi::Engine::new(&config),
            limits,
            cache_limits,
            cache: Mutex::new(SingleFlightLruCache::new(cache_limits)),
        }
    }

    /// The limits applied to every module this host loads.
    #[must_use]
    pub const fn limits(&self) -> PluginLimits {
        self.limits
    }

    /// The process-level limits applied to completed cache entries.
    #[must_use]
    pub const fn cache_limits(&self) -> PluginCacheLimits {
        self.cache_limits
    }

    /// Load and validate a plugin module, reusing a cached success or failure
    /// when the same bytes (by SHA-256) were loaded before.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLoadError`] when validation fails; see the
    /// [`crate::module`] docs for the full list of checks.
    pub fn load(&self, bytes: &[u8]) -> Result<Arc<PluginModule>, PluginLoadError> {
        if bytes.len() > self.limits.max_module_bytes() {
            return Err(PluginLoadError::ModuleTooLarge {
                bytes: bytes.len(),
                max_bytes: self.limits.max_module_bytes(),
            });
        }
        self.load_cached(bytes, || {
            PluginModule::new(&self.engine, bytes, self.limits).map(Arc::new)
        })
    }

    fn load_cached(
        &self,
        bytes: &[u8],
        load: impl FnOnce() -> CachedLoadResult,
    ) -> CachedLoadResult {
        let hash: [u8; 32] = sha2::Sha256::digest(bytes).into();
        let probe = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .probe(hash, bytes.len());
        match probe {
            CacheProbe::Ready(result) => result,
            CacheProbe::Pending(cell) => {
                let result = cell.get_or_init(load).clone();
                self.cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .complete(&hash, &cell, &result);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::*;

    fn invalid(message: &str) -> CachedLoadResult {
        Err(PluginLoadError::InvalidModule {
            message: message.to_string(),
        })
    }

    fn assert_invalid(result: CachedLoadResult, expected: &str) {
        assert_eq!(
            result.unwrap_err(),
            PluginLoadError::InvalidModule {
                message: expected.to_string(),
            }
        );
    }

    #[test]
    fn deterministic_failures_are_cached_by_hash() {
        let host = PluginHost::new();
        let attempts = AtomicUsize::new(0);
        let first = host.load_cached(b"invalid", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            invalid("first")
        });
        let second = host.load_cached(b"invalid", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            invalid("should not run")
        });

        assert_invalid(first, "first");
        assert_invalid(second, "first");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_changed_hash_gets_an_independent_load_outcome() {
        let host = PluginHost::new();
        let attempts = AtomicUsize::new(0);
        let first = host.load_cached(b"version-a", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            invalid("a")
        });
        let changed = host.load_cached(b"version-b", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            invalid("b")
        });

        assert_invalid(first, "a");
        assert_invalid(changed, "b");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn concurrent_same_hash_misses_are_single_flight() {
        const THREADS: usize = 8;

        let host = Arc::new(PluginHost::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let host = Arc::clone(&host);
                let attempts = Arc::clone(&attempts);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    host.load_cached(b"same invalid module", || {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        invalid("shared")
                    })
                })
            })
            .collect();

        for handle in handles {
            assert_invalid(handle.join().unwrap(), "shared");
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}

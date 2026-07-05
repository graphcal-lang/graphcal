//! The plugin host: one WASM engine plus a content-hash module cache.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::Digest as _;

use crate::module::{PluginLoadError, PluginModule};

/// Resource bounds applied to every plugin call.
///
/// Plugins are trusted-by-default *because* these bounds exist: the sandbox
/// removes filesystem and network access (confidentiality and integrity),
/// and fuel plus the memory cap bound availability. Both limits are
/// per-call; the language server re-evaluates on every debounced keystroke,
/// so an unbounded plugin would hang the editor, not just one CLI run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    /// Fuel budget for one call (instantiation, including a `start`
    /// function, is metered with the same budget). Fuel corresponds roughly
    /// to executed instructions.
    pub fuel_per_call: u64,
    /// Cap on the plugin's linear memory in bytes.
    pub max_memory_bytes: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            fuel_per_call: 100_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Loads, validates, caches, and executes WASM plugin modules.
///
/// Embedders keep one host alive for the process (the language server keeps
/// it across re-evaluations) so that reloading a project hits the
/// content-hash cache instead of recompiling modules.
pub struct PluginHost {
    engine: wasmi::Engine,
    limits: PluginLimits,
    cache: Mutex<HashMap<[u8; 32], Arc<PluginModule>>>,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
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

    /// Create a host with explicit limits.
    #[must_use]
    pub fn with_limits(limits: PluginLimits) -> Self {
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
        Self {
            engine: wasmi::Engine::new(&config),
            limits,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The limits applied to every module this host loads.
    #[must_use]
    pub const fn limits(&self) -> PluginLimits {
        self.limits
    }

    /// Load and validate a plugin module, reusing the cached compilation
    /// when the same bytes (by SHA-256) were loaded before.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLoadError`] when validation fails; see the
    /// [`crate::module`] docs for the full list of checks.
    pub fn load(&self, bytes: &[u8]) -> Result<Arc<PluginModule>, PluginLoadError> {
        let hash: [u8; 32] = sha2::Sha256::digest(bytes).into();
        if let Some(module) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hash)
        {
            return Ok(Arc::clone(module));
        }

        let module = Arc::new(PluginModule::new(&self.engine, bytes, self.limits)?);
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(hash, Arc::clone(&module));
        Ok(module)
    }
}

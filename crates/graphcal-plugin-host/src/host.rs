//! The plugin host: one WASM engine plus a content-hash module cache.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::Digest as _;

use crate::module::{PluginLoadError, PluginModule};

/// Complete resource policy for plugin loading and execution.
///
/// Plugins are trusted-by-default *because* these bounds exist: the sandbox
/// removes filesystem and network access (confidentiality and integrity),
/// while encoded-module bytes, compile-time structure limits, fuel,
/// linear-memory bytes, and table elements bound availability.
/// The fields are private so adding a new resource dimension cannot silently
/// leave callers with an incomplete policy through a struct literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    fuel_per_call: u64,
    max_memory_bytes: usize,
    max_table_elements: usize,
    max_module_bytes: usize,
}

impl PluginLimits {
    /// Construct a complete resource policy.
    #[must_use]
    pub const fn new(
        fuel_per_call: u64,
        max_memory_bytes: usize,
        max_table_elements: usize,
        max_module_bytes: usize,
    ) -> Self {
        Self {
            fuel_per_call,
            max_memory_bytes,
            max_table_elements,
            max_module_bytes,
        }
    }

    /// Return a policy with a different per-call fuel budget.
    #[must_use]
    pub const fn with_fuel_per_call(mut self, fuel_per_call: u64) -> Self {
        self.fuel_per_call = fuel_per_call;
        self
    }

    /// Return a policy with a different linear-memory byte limit.
    #[must_use]
    pub const fn with_max_memory_bytes(mut self, max_memory_bytes: usize) -> Self {
        self.max_memory_bytes = max_memory_bytes;
        self
    }

    /// Return a policy with a different per-table element limit.
    #[must_use]
    pub const fn with_max_table_elements(mut self, max_table_elements: usize) -> Self {
        self.max_table_elements = max_table_elements;
        self
    }

    /// Return a policy with a different module byte limit.
    #[must_use]
    pub const fn with_max_module_bytes(mut self, max_module_bytes: usize) -> Self {
        self.max_module_bytes = max_module_bytes;
        self
    }

    /// Fuel budget for one logical call, including instantiation and `start`.
    #[must_use]
    pub const fn fuel_per_call(self) -> u64 {
        self.fuel_per_call
    }

    /// Maximum bytes in each linear memory.
    #[must_use]
    pub const fn max_memory_bytes(self) -> usize {
        self.max_memory_bytes
    }

    /// Maximum elements in each WebAssembly table.
    #[must_use]
    pub const fn max_table_elements(self) -> usize {
        self.max_table_elements
    }

    /// Maximum bytes accepted for one encoded WebAssembly module.
    #[must_use]
    pub const fn max_module_bytes(self) -> usize {
        self.max_module_bytes
    }
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self::new(100_000_000, 64 * 1024 * 1024, 10_000, 16 * 1024 * 1024)
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

    /// Create a host with an explicit complete resource policy.
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
        // Parsing and eager translation happen before call fuel exists. Wasmi's
        // strict policy bounds structural counts and adversarial tiny-function
        // amplification during this untrusted-input phase.
        config.enforced_limits(wasmi::EnforcedLimits::strict());
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
        if bytes.len() > self.limits.max_module_bytes() {
            return Err(PluginLoadError::ModuleTooLarge {
                bytes: bytes.len(),
                max_bytes: self.limits.max_module_bytes(),
            });
        }
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

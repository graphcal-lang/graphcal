//! The per-module resource policy for plugin loading and execution.

/// Complete per-module resource policy for plugin loading and execution.
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

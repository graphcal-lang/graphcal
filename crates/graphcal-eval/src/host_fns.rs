//! Host-native extern function registry (Phase A of the plugin plan, #25).
//!
//! Extern functions declared by `import plugin "…" as alias { … }` blocks
//! are resolved against a [`HostFunctionRegistry`] injected by the embedder
//! (CLI, LSP, or tests). In this phase the registry maps each canonical
//! `(plugin path, function name)` identity to a host-native Rust closure;
//! Phase B swaps the closure backend for a WASM runtime without changing
//! this interface.
//!
//! The Phase A host ABI is scalar-only, mirroring
//! [`BuiltinFunction::eval`](graphcal_compiler::registry::builtins::BuiltinFunction):
//! arguments arrive as `&[f64]` (Int and Bool arguments are converted —
//! exactly-representable integers and `1.0`/`0.0` respectively) and the
//! result is converted back per the declared result kind.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::function_signature::FunctionSignature;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::plugin::{ExternFnKey, PluginPath};

/// Error returned by a host function closure.
///
/// The message surfaces verbatim in the per-node `EvalFailed` diagnostic,
/// prefixed with the plugin alias and function name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFnError {
    /// Human-readable failure description.
    pub message: String,
}

impl HostFnError {
    /// Create an error from a message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostFnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HostFnError {}

impl From<String> for HostFnError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl From<&str> for HostFnError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// A host-native extern function implementation.
pub type HostFn = Arc<dyn Fn(&[f64]) -> Result<f64, HostFnError> + Send + Sync>;

/// One registered extern function: the callable closure plus, for
/// plugin-backed entries, the signature the plugin's manifest declared.
struct HostFnEntry {
    function: HostFn,
    provided_signature: Option<FunctionSignature>,
}

/// Why a plugin failed to register its functions.
///
/// Recorded by the embedder while building the registry (the WASM plugin
/// host discovers these when compiling/validating the module); surfaced by
/// the evaluation pipeline as load-time diagnostics with the import's span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRegistrationError {
    /// The module declares an import other than `graphcal::fail` — the
    /// purity rule every graphcal plugin must satisfy.
    ForbiddenImport {
        /// Wasm module name of the forbidden import.
        module: String,
        /// Wasm field name of the forbidden import.
        name: String,
    },
    /// Any other validation failure (missing/malformed manifest, invalid
    /// module, wrong export types, …), rendered by the plugin host.
    LoadFailed {
        /// Human-readable failure description.
        reason: String,
    },
}

/// Registry mapping resolved extern function references to host closures.
///
/// Injected by the embedder; evaluation looks functions up by
/// [`ExternFnKey`]. A declared extern function with no registry entry is a
/// load-time diagnostic (`MissingHostFunction`). Entries registered from a
/// WASM plugin manifest carry their provided [`FunctionSignature`], which
/// the pipeline verifies structurally against each declaration; host-native
/// entries (like the demo plugin) carry none and trust the declaration.
#[derive(Clone, Default)]
pub struct HostFunctionRegistry {
    fns: HashMap<ExternFnKey, Arc<HostFnEntry>>,
    failed_plugins: HashMap<PluginPath, PluginRegistrationError>,
}

impl std::fmt::Debug for HostFunctionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostFunctionRegistry")
            .field("functions", &self.fns.keys().collect::<Vec<_>>())
            .field(
                "failed_plugins",
                &self.failed_plugins.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl HostFunctionRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a host-native closure for one extern function, with no
    /// provided signature (the declaration is trusted).
    ///
    /// Re-registering the same `(plugin, name)` replaces the previous
    /// closure — the embedder owns the registry contents.
    pub fn register(
        &mut self,
        plugin: PluginPath,
        name: FnName,
        function: impl Fn(&[f64]) -> Result<f64, HostFnError> + Send + Sync + 'static,
    ) {
        self.fns.insert(
            ExternFnKey { plugin, name },
            Arc::new(HostFnEntry {
                function: Arc::new(function),
                provided_signature: None,
            }),
        );
    }

    /// Register a plugin-backed closure together with the signature its
    /// manifest declares; the pipeline verifies declarations against it.
    pub fn register_with_signature(
        &mut self,
        plugin: PluginPath,
        name: FnName,
        signature: FunctionSignature,
        function: impl Fn(&[f64]) -> Result<f64, HostFnError> + Send + Sync + 'static,
    ) {
        self.fns.insert(
            ExternFnKey { plugin, name },
            Arc::new(HostFnEntry {
                function: Arc::new(function),
                provided_signature: Some(signature),
            }),
        );
    }

    /// Record that a plugin's functions could not be registered at all.
    ///
    /// The pipeline reports this (with the import site's span) before any
    /// per-function "missing host function" diagnostic, so users see the
    /// root cause.
    pub fn record_plugin_failure(&mut self, plugin: PluginPath, error: PluginRegistrationError) {
        self.failed_plugins.insert(plugin, error);
    }

    /// The recorded registration failure for `plugin`, if any.
    #[must_use]
    pub fn plugin_failure(&self, plugin: &PluginPath) -> Option<&PluginRegistrationError> {
        self.failed_plugins.get(plugin)
    }

    /// Look up the host closure for an extern function.
    #[must_use]
    pub fn get(&self, key: &ExternFnKey) -> Option<&HostFn> {
        self.fns.get(key).map(|entry| &entry.function)
    }

    /// The manifest-provided signature for an extern function, when its
    /// entry came from a WASM plugin.
    #[must_use]
    pub fn provided_signature(&self, key: &ExternFnKey) -> Option<&FunctionSignature> {
        self.fns
            .get(key)
            .and_then(|entry| entry.provided_signature.as_ref())
    }

    /// Returns whether the registry provides an implementation for `key`.
    #[must_use]
    pub fn contains(&self, key: &ExternFnKey) -> bool {
        self.fns.contains_key(key)
    }
}

/// The plugin path of the built-in demo plugin registered by
/// [`demo_registry`].
pub const DEMO_PLUGIN_PATH: &str = "graphcal:demo";

/// Phase A stand-in registry used by the CLI and LSP embedders.
///
/// Real plugins do not exist until Phase B ships a WASM runtime, so the
/// default embedders provide one well-known demo plugin (path
/// [`DEMO_PLUGIN_PATH`]) to prove the extern path end-to-end:
///
/// ```gcl
/// import plugin "graphcal:demo" as demo {
///     fn lerp<D>(a: D, b: D, t: Dimensionless) -> D;
///     fn inverse<D>(x: D) -> D^-1;
///     fn geometric_mean<D1, D2>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
/// }
/// ```
#[must_use]
pub fn demo_registry() -> HostFunctionRegistry {
    let plugin = PluginPath::new(DEMO_PLUGIN_PATH);
    let mut registry = HostFunctionRegistry::new();
    registry.register(plugin.clone(), FnName::expect_valid("lerp"), |args| {
        let (a, b, t) = (args[0], args[1], args[2]);
        Ok((b - a).mul_add(t, a))
    });
    registry.register(plugin.clone(), FnName::expect_valid("inverse"), |args| {
        if args[0] == 0.0 {
            return Err(HostFnError::new("division by zero"));
        }
        Ok(args[0].recip())
    });
    registry.register(plugin, FnName::expect_valid("geometric_mean"), |args| {
        let product = args[0] * args[1];
        if product < 0.0 {
            return Err(HostFnError::new(
                "geometric mean of a negative product is undefined",
            ));
        }
        Ok(product.sqrt())
    });
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> ExternFnKey {
        ExternFnKey {
            plugin: PluginPath::new(DEMO_PLUGIN_PATH),
            name: FnName::expect_valid(name),
        }
    }

    #[test]
    fn demo_registry_provides_documented_functions() {
        let registry = demo_registry();
        for name in ["lerp", "inverse", "geometric_mean"] {
            assert!(registry.contains(&key(name)), "missing demo fn `{name}`");
        }
    }

    #[test]
    fn demo_lerp_interpolates() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        assert!((lerp(&[0.0, 10.0, 0.25]).unwrap() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn demo_inverse_rejects_zero() {
        let registry = demo_registry();
        let inverse = registry.get(&key("inverse")).unwrap();
        assert_eq!(
            inverse(&[0.0]).unwrap_err().message,
            "division by zero".to_string()
        );
    }
}

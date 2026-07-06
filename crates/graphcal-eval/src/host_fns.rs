//! Host-native extern function registry (Phase A of the plugin plan, #25).
//!
//! Extern functions declared by `import plugin "…" as alias { … }` blocks
//! are resolved against a [`HostFunctionRegistry`] injected by the embedder
//! (CLI, LSP, or tests). The registry maps each canonical
//! `(plugin path, function name)` identity to a host closure; the WASM
//! plugin host registers module-backed closures through the same interface.
//!
//! The host ABI carries SI-flat numbers: each value crosses as a
//! [`HostFnValue`] — a bare `f64` for scalars (Int and Bool arguments are
//! converted — exactly-representable integers and `1.0`/`0.0` respectively)
//! or a dense `f64` buffer in index order for arrays. The evaluator does all
//! typed interpretation against the declared signature; closures never see
//! dimensions, units, or index identities beyond buffer lengths.

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

/// One value crossing the host-function boundary, SI-flat in both directions.
///
/// Bool and Int values cross inside [`Self::Scalar`] using the documented
/// encodings (`1.0`/`0.0`, exactly-representable integers); arrays cross as
/// dense element buffers in index declaration order. The evaluator converts
/// to and from typed [`RuntimeValue`]s per the declared signature — a
/// closure returning the wrong shape is reported as a plugin failure, never
/// reinterpreted.
///
/// [`RuntimeValue`]: graphcal_compiler::registry::runtime_value::RuntimeValue
#[derive(Debug, Clone, PartialEq)]
pub enum HostFnValue {
    /// A scalar in SI base units (also the Bool/Int wire encoding).
    Scalar(f64),
    /// A dense array of SI scalars in index order.
    Buffer(Vec<f64>),
}

impl HostFnValue {
    /// The scalar payload, or an error naming the parameter position.
    ///
    /// # Errors
    ///
    /// Returns a [`HostFnError`] when this value is a buffer.
    pub fn expect_scalar(&self, position: usize) -> Result<f64, HostFnError> {
        match self {
            Self::Scalar(value) => Ok(*value),
            Self::Buffer(_) => Err(HostFnError::new(format!(
                "argument {position} is an array, expected a scalar"
            ))),
        }
    }

    /// The buffer payload, or an error naming the parameter position.
    ///
    /// # Errors
    ///
    /// Returns a [`HostFnError`] when this value is a scalar.
    pub fn expect_buffer(&self, position: usize) -> Result<&[f64], HostFnError> {
        match self {
            Self::Buffer(values) => Ok(values),
            Self::Scalar(_) => Err(HostFnError::new(format!(
                "argument {position} is a scalar, expected an array"
            ))),
        }
    }
}

/// A host-native extern function implementation.
pub type HostFn = Arc<dyn Fn(&[HostFnValue]) -> Result<HostFnValue, HostFnError> + Send + Sync>;

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
        function: impl Fn(&[HostFnValue]) -> Result<HostFnValue, HostFnError> + Send + Sync + 'static,
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
        function: impl Fn(&[HostFnValue]) -> Result<HostFnValue, HostFnError> + Send + Sync + 'static,
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

/// Host-native stand-in registry used by the CLI and LSP embedders.
///
/// The default embedders provide one well-known demo plugin (path
/// [`DEMO_PLUGIN_PATH`]) to prove the extern path end-to-end without a
/// `.wasm` module:
///
/// ```gcl
/// import plugin "graphcal:demo" as demo {
///     fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
///     fn inverse<D: Dim>(x: D) -> D^-1;
///     fn geometric_mean<D1: Dim, D2: Dim>(x: D1, y: D2) -> D1^(1/2) * D2^(1/2);
///     fn normalize<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I];
///     fn dv_range<I: Index>(xs: Velocity[I]) -> DvRange;
/// }
/// ```
#[must_use]
pub fn demo_registry() -> HostFunctionRegistry {
    let plugin = PluginPath::new(DEMO_PLUGIN_PATH);
    let mut registry = HostFunctionRegistry::new();
    registry.register(plugin.clone(), FnName::expect_valid("lerp"), |args| {
        let (a, b, t) = (
            args[0].expect_scalar(0)?,
            args[1].expect_scalar(1)?,
            args[2].expect_scalar(2)?,
        );
        Ok(HostFnValue::Scalar((b - a).mul_add(t, a)))
    });
    registry.register(plugin.clone(), FnName::expect_valid("inverse"), |args| {
        let x = args[0].expect_scalar(0)?;
        if x == 0.0 {
            return Err(HostFnError::new("division by zero"));
        }
        Ok(HostFnValue::Scalar(x.recip()))
    });
    registry.register(
        plugin.clone(),
        FnName::expect_valid("geometric_mean"),
        |args| {
            let product = args[0].expect_scalar(0)? * args[1].expect_scalar(1)?;
            if product < 0.0 {
                return Err(HostFnError::new(
                    "geometric mean of a negative product is undefined",
                ));
            }
            Ok(HostFnValue::Scalar(product.sqrt()))
        },
    );
    registry.register(plugin.clone(), FnName::expect_valid("normalize"), |args| {
        let xs = args[0].expect_buffer(0)?;
        let total: f64 = xs.iter().sum();
        if total == 0.0 {
            return Err(HostFnError::new(
                "cannot normalize: the elements sum to zero",
            ));
        }
        Ok(HostFnValue::Buffer(xs.iter().map(|x| x / total).collect()))
    });
    registry.register(plugin, FnName::expect_valid("dv_range"), |args| {
        let xs = args[0].expect_buffer(0)?;
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        for x in xs {
            min = min.min(*x);
            max = max.max(*x);
        }
        // Struct results cross as one f64 slot per field, in field order.
        Ok(HostFnValue::Buffer(vec![min, max]))
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

    fn scalars(values: &[f64]) -> Vec<HostFnValue> {
        values.iter().map(|v| HostFnValue::Scalar(*v)).collect()
    }

    #[test]
    fn demo_registry_provides_documented_functions() {
        let registry = demo_registry();
        for name in ["lerp", "inverse", "geometric_mean", "normalize", "dv_range"] {
            assert!(registry.contains(&key(name)), "missing demo fn `{name}`");
        }
    }

    #[test]
    fn demo_lerp_interpolates() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        let result = lerp(&scalars(&[0.0, 10.0, 0.25])).unwrap();
        assert_eq!(result, HostFnValue::Scalar(2.5));
    }

    #[test]
    fn demo_inverse_rejects_zero() {
        let registry = demo_registry();
        let inverse = registry.get(&key("inverse")).unwrap();
        assert_eq!(
            inverse(&scalars(&[0.0])).unwrap_err().message,
            "division by zero".to_string()
        );
    }

    #[test]
    fn demo_normalize_divides_by_the_sum() {
        let registry = demo_registry();
        let normalize = registry.get(&key("normalize")).unwrap();
        let result = normalize(&[HostFnValue::Buffer(vec![1.0, 3.0])]).unwrap();
        assert_eq!(result, HostFnValue::Buffer(vec![0.25, 0.75]));
    }

    #[test]
    fn shape_mismatches_are_reported_not_reinterpreted() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        let err = lerp(&[
            HostFnValue::Buffer(vec![1.0]),
            HostFnValue::Scalar(1.0),
            HostFnValue::Scalar(0.5),
        ])
        .unwrap_err();
        assert!(err.message.contains("expected a scalar"), "{err}");
    }
}

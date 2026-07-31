//! Host-native extern function registry (Phase A of the plugin plan, #25).
//!
//! Extern functions declared by `import plugin "…" as alias { … }` blocks
//! are resolved against a [`HostFunctionRegistry`] injected by the embedder
//! (CLI, LSP, or tests). The registry maps each canonical
//! `(plugin path, function name)` identity to a host closure; the WASM
//! plugin host registers module-backed closures through the same interface.
//!
//! The host ABI carries SI-flat numbers: each value crosses as a
//! [`HostFnValue`] — one `f64` slot for quantities, `Int`, and `Bool` (using
//! exactly-representable integers and `1.0`/`0.0` respectively), a shaped
//! row-major array, or fixed-layout record slots. The evaluator does all typed
//! interpretation against the declared signature; closures never see
//! dimensions, units, or index identities beyond ordered axis extents.

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
    pub(crate) message: String,
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

/// Dense row-major array crossing the host-function boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct HostArray {
    shape: Vec<usize>,
    values: Vec<f64>,
}

impl HostArray {
    /// Build an array whose non-empty shape product equals `values.len()`.
    ///
    /// # Errors
    ///
    /// Returns [`HostFnError`] for invalid axes, cardinality overflow, or a
    /// mismatched value count.
    pub fn try_new(shape: Vec<usize>, values: Vec<f64>) -> Result<Self, HostFnError> {
        if shape.is_empty() || shape.contains(&0) {
            return Err(HostFnError::new(
                "array shapes require one or more non-empty axes",
            ));
        }
        let expected = shape.iter().try_fold(1_usize, |size, extent| {
            size.checked_mul(*extent)
                .ok_or_else(|| HostFnError::new("array shape cardinality overflowed usize"))
        })?;
        if expected != values.len() {
            return Err(HostFnError::new(format!(
                "array shape {shape:?} requires {expected} values, found {}",
                values.len()
            )));
        }
        Ok(Self { shape, values })
    }

    /// Convenience constructor for a non-empty rank-one array.
    ///
    /// # Errors
    ///
    /// Returns [`HostFnError`] when `values` is empty.
    pub fn vector(values: Vec<f64>) -> Result<Self, HostFnError> {
        Self::try_new(vec![values.len()], values)
    }

    /// Ordered row-major shape.
    #[must_use]
    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    /// Flattened row-major values.
    #[must_use]
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Consume into shape and values.
    #[must_use]
    pub fn into_parts(self) -> (Vec<usize>, Vec<f64>) {
        (self.shape, self.values)
    }
}

/// One value crossing the host-function boundary, SI-flat in both directions.
///
/// Quantities, `Bool`, and `Int` each cross inside one [`Self::F64`] slot; the
/// declared signature determines its semantic kind and therefore its encoding
/// (`1.0`/`0.0` for `Bool`, exactly-representable integers for `Int`). Arrays
/// cross as shaped, row-major dense values. The evaluator
/// converts to and from typed [`RuntimeValue`]s per the declared signature — a
/// closure returning the wrong shape is reported as a plugin failure, never
/// reinterpreted.
///
/// [`RuntimeValue`]: graphcal_compiler::registry::runtime_value::RuntimeValue
#[derive(Debug, Clone, PartialEq)]
pub enum HostFnValue {
    /// One raw `f64` ABI slot; the function signature supplies its semantic kind.
    F64(f64),
    /// Dense row-major array with explicit axis extents.
    Array(HostArray),
    /// Flattened fixed-layout record result slots.
    Record(Vec<f64>),
}

impl HostFnValue {
    /// The quantity payload, or an error naming the parameter position.
    ///
    /// # Errors
    ///
    /// Returns a [`HostFnError`] when this value is a buffer.
    fn expect_quantity(&self, position: usize) -> Result<f64, HostFnError> {
        match self {
            Self::F64(value) => Ok(*value),
            Self::Array(_) | Self::Record(_) => Err(HostFnError::new(format!(
                "argument {position} is not a single quantity slot"
            ))),
        }
    }

    /// The buffer payload, or an error naming the parameter position.
    ///
    /// # Errors
    ///
    /// Returns a [`HostFnError`] when this value is a quantity.
    fn expect_array(&self, position: usize) -> Result<&HostArray, HostFnError> {
        match self {
            Self::Array(array) => Ok(array),
            Self::F64(_) | Self::Record(_) => Err(HostFnError::new(format!(
                "argument {position} is not an array"
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
    fn register(
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
    pub(crate) fn plugin_failure(&self, plugin: &PluginPath) -> Option<&PluginRegistrationError> {
        self.failed_plugins.get(plugin)
    }

    /// Look up the host closure for an extern function.
    #[must_use]
    pub(crate) fn get(&self, key: &ExternFnKey) -> Option<&HostFn> {
        self.fns.get(key).map(|entry| &entry.function)
    }

    /// The manifest-provided signature for an extern function, when its
    /// entry came from a WASM plugin.
    #[must_use]
    pub(crate) fn provided_signature(&self, key: &ExternFnKey) -> Option<&FunctionSignature> {
        self.fns
            .get(key)
            .and_then(|entry| entry.provided_signature.as_ref())
    }

    /// Returns whether the registry provides an implementation for `key`.
    #[must_use]
    pub(crate) fn contains(&self, key: &ExternFnKey) -> bool {
        self.fns.contains_key(key)
    }
}

/// The plugin path of the built-in demo plugin registered by
/// [`demo_registry`].
const DEMO_PLUGIN_PATH: &str = "graphcal:demo";

/// Host-native stand-in registry used by the CLI and LSP embedders.
///
/// The default embedders provide one well-known demo plugin (path
/// `DEMO_PLUGIN_PATH`) to prove the extern path end-to-end without a
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
            args[0].expect_quantity(0)?,
            args[1].expect_quantity(1)?,
            args[2].expect_quantity(2)?,
        );
        Ok(HostFnValue::F64((b - a).mul_add(t, a)))
    });
    registry.register(plugin.clone(), FnName::expect_valid("inverse"), |args| {
        let x = args[0].expect_quantity(0)?;
        if x == 0.0 {
            return Err(HostFnError::new("division by zero"));
        }
        Ok(HostFnValue::F64(x.recip()))
    });
    registry.register(
        plugin.clone(),
        FnName::expect_valid("geometric_mean"),
        |args| {
            let product = args[0].expect_quantity(0)? * args[1].expect_quantity(1)?;
            if product < 0.0 {
                return Err(HostFnError::new(
                    "geometric mean of a negative product is undefined",
                ));
            }
            Ok(HostFnValue::F64(product.sqrt()))
        },
    );
    registry.register(plugin.clone(), FnName::expect_valid("normalize"), |args| {
        let xs = args[0].expect_array(0)?;
        let total: f64 = xs.values().iter().sum();
        if total == 0.0 {
            return Err(HostFnError::new(
                "cannot normalize: the elements sum to zero",
            ));
        }
        Ok(HostFnValue::Array(HostArray::try_new(
            xs.shape().to_vec(),
            xs.values().iter().map(|x| x / total).collect(),
        )?))
    });
    registry.register(
        plugin.clone(),
        FnName::expect_valid("matrix_transpose"),
        |args| {
            let matrix = args[0].expect_array(0)?;
            let [rows, columns] = matrix.shape() else {
                return Err(HostFnError::new(
                    "matrix_transpose expects a rank-two array",
                ));
            };
            let values = (0..*columns)
                .flat_map(|column| {
                    (0..*rows).map(move |row| {
                        let offset = row
                            .checked_mul(*columns)
                            .and_then(|offset| offset.checked_add(column))
                            .ok_or_else(|| {
                                HostFnError::new("matrix_transpose offset overflowed usize")
                            })?;
                        matrix.values().get(offset).copied().ok_or_else(|| {
                            HostFnError::new("matrix_transpose input shape is inconsistent")
                        })
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HostFnValue::Array(HostArray::try_new(
                vec![*columns, *rows],
                values,
            )?))
        },
    );
    registry.register(plugin, FnName::expect_valid("dv_range"), |args| {
        let xs = args[0].expect_array(0)?;
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        for x in xs.values() {
            min = min.min(*x);
            max = max.max(*x);
        }
        // Struct results cross as one f64 slot per field, in field order.
        Ok(HostFnValue::Record(vec![min, max]))
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

    fn quantities(values: &[f64]) -> Vec<HostFnValue> {
        values.iter().map(|v| HostFnValue::F64(*v)).collect()
    }

    #[test]
    fn demo_registry_provides_documented_functions() {
        let registry = demo_registry();
        for name in [
            "lerp",
            "inverse",
            "geometric_mean",
            "normalize",
            "matrix_transpose",
            "dv_range",
        ] {
            assert!(registry.contains(&key(name)), "missing demo fn `{name}`");
        }
    }

    #[test]
    fn demo_lerp_interpolates() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        let result = lerp(&quantities(&[0.0, 10.0, 0.25])).unwrap();
        assert_eq!(result, HostFnValue::F64(2.5));
    }

    #[test]
    fn demo_inverse_rejects_zero() {
        let registry = demo_registry();
        let inverse = registry.get(&key("inverse")).unwrap();
        assert_eq!(
            inverse(&quantities(&[0.0])).unwrap_err().message,
            "division by zero".to_string()
        );
    }

    #[test]
    fn demo_normalize_divides_by_the_sum() {
        let registry = demo_registry();
        let normalize = registry.get(&key("normalize")).unwrap();
        let input = HostArray::vector(vec![1.0, 3.0]).unwrap();
        let result = normalize(&[HostFnValue::Array(input)]).unwrap();
        assert_eq!(
            result,
            HostFnValue::Array(HostArray::vector(vec![0.25, 0.75]).unwrap())
        );
    }

    #[test]
    fn shape_mismatches_are_reported_not_reinterpreted() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        let err = lerp(&[
            HostFnValue::Array(HostArray::vector(vec![1.0]).unwrap()),
            HostFnValue::F64(1.0),
            HostFnValue::F64(0.5),
        ])
        .unwrap_err();
        assert!(err.message.contains("not a single quantity slot"), "{err}");
    }

    #[test]
    fn evaluator_rejects_fractional_host_int_results() {
        let plugin = PluginPath::new("graphcal:test-exact-int");
        let mut registry = HostFunctionRegistry::new();
        registry.register(
            plugin.clone(),
            FnName::expect_valid("fractional_scalar"),
            |_| Ok(HostFnValue::F64(3.7)),
        );
        registry.register(plugin, FnName::expect_valid("fractional_record"), |_| {
            Ok(HostFnValue::Record(vec![3.7]))
        });
        let source = r#"
type IntResult {
    IntResult(value: Int),
}
import plugin "graphcal:test-exact-int" as test {
    fn fractional_scalar() -> Int;
    fn fractional_record() -> IntResult;
}
node invalid_scalar: Int = test.fractional_scalar();
node invalid_record: IntResult = test.fractional_record();
"#;
        let project = crate::loader::LoadedProject::from_source(source, "test.gcl").unwrap();
        let result = crate::eval::compile_and_eval_from_project_with_host_fns(
            &project,
            &HashMap::new(),
            &registry,
        )
        .unwrap();

        for name in ["invalid_scalar", "invalid_record"] {
            let error = result
                .nodes
                .iter()
                .find(|(candidate, _)| candidate.to_string() == name)
                .unwrap_or_else(|| panic!("{name} node should exist"))
                .1
                .as_ref()
                .expect_err("fractional Int result should fail");
            let crate::eval::NodeError::EvalFailed { message } = error else {
                panic!("expected EvalFailed, got {error:?}");
            };
            assert!(message.contains("is not integer-valued"), "{message}");
        }
    }
}

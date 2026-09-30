//! Host-native extern function registry (Phase A of the plugin plan, #25).
//!
//! Extern functions declared by `import plugin "…" as alias { … }` blocks
//! are resolved against a [`HostFunctionRegistry`] injected by the embedder
//! (CLI, LSP, or tests). The registry maps each canonical
//! `(plugin path, function name)` identity to a host closure; the WASM
//! plugin host registers module-backed closures through the same interface.
//!
//! The host ABI carries SI-flat numbers. A closure receives validated
//! [`HostArgument`]s — finite quantities, Booleans, exactly representable
//! integers, or shaped row-major arrays of one of them — and returns a raw
//! [`HostFnValue`]: one `f64` slot for quantities, `Int`, and `Bool` (using
//! exactly-representable integers and `1.0`/`0.0` respectively), a shaped
//! row-major array, or fixed-layout record slots. The evaluator validates that
//! result against the declared signature; closures never see dimensions, units,
//! or index identities beyond ordered axis extents.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::function_signature::FunctionSignature;
use graphcal_compiler::plugin_identity::{ExternFnKey, PluginIdentity};
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::plugin::PluginPath;

use crate::host_abi::HostScalar;
use crate::host_abi::argument::{
    DenseShapeError, HostArgument, HostArrayElements, check_dense_shape,
};

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

impl From<DenseShapeError> for HostFnError {
    fn from(error: DenseShapeError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<&str> for HostFnError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Dense row-major array returned across the host-function boundary.
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
        check_dense_shape(&shape, values.len())?;
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

/// One raw value a host function returns, SI-flat.
///
/// Quantities, `Bool`, and `Int` each return inside one [`Self::F64`] slot; the
/// declared signature determines its semantic kind and therefore its encoding
/// (`1.0`/`0.0` for `Bool`, exactly-representable integers for `Int`). Arrays
/// return as shaped, row-major dense values. The evaluator validates the
/// result against the declared signature — a closure returning the wrong
/// shape or an invalid slot is reported as a plugin failure, never
/// reinterpreted.
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
    /// The raw wire form of a validated argument, as a function that returns
    /// its input would produce it.
    #[must_use]
    pub fn from_argument(argument: &HostArgument) -> Self {
        match argument {
            HostArgument::Scalar(scalar) => Self::F64(scalar.abi_slot()),
            HostArgument::Array(array) => Self::Array(HostArray {
                shape: array.shape().to_vec(),
                values: array.elements().abi_slots(),
            }),
        }
    }
}

/// A host-native extern function implementation.
pub type HostFn = Arc<dyn Fn(&[HostArgument]) -> Result<HostFnValue, HostFnError> + Send + Sync>;

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

/// Compile-time metadata for externally provided functions.
///
/// This contains identities, manifest signatures, and plugin-load failures,
/// but deliberately carries no callable closure. Static checking can therefore
/// verify every extern declaration without gaining the capability to execute
/// runtime host code.
#[derive(Debug, Clone, Default)]
pub struct HostFunctionMetadata {
    signatures: HashMap<ExternFnKey, Option<FunctionSignature>>,
    failed_plugins: HashMap<PluginIdentity, PluginRegistrationError>,
}

impl HostFunctionMetadata {
    /// Whether an implementation is expected to exist for `key` at runtime.
    #[must_use]
    pub fn contains(&self, key: &ExternFnKey) -> bool {
        self.signatures.contains_key(key)
    }

    /// Manifest-provided signature for a plugin-backed implementation.
    #[must_use]
    pub fn provided_signature(&self, key: &ExternFnKey) -> Option<&FunctionSignature> {
        self.signatures.get(key).and_then(Option::as_ref)
    }

    /// Plugin registration failure captured by the embedding shell.
    #[must_use]
    pub fn plugin_failure(&self, plugin: &PluginIdentity) -> Option<&PluginRegistrationError> {
        self.failed_plugins.get(plugin)
    }
}

/// Registry mapping resolved extern function references to host closures.
///
/// Runtime evaluation uses the callable entries, while compilation receives
/// only [`HostFunctionMetadata`] through [`Self::metadata`].
#[derive(Clone, Default)]
pub struct HostFunctionRegistry {
    fns: HashMap<ExternFnKey, HostFn>,
    metadata: HostFunctionMetadata,
}

impl std::fmt::Debug for HostFunctionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostFunctionRegistry")
            .field("functions", &self.fns.keys().collect::<Vec<_>>())
            .field(
                "failed_plugins",
                &self.metadata.failed_plugins.keys().collect::<Vec<_>>(),
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
    pub(crate) fn register(
        &mut self,
        plugin: PluginPath,
        name: FnName,
        function: impl Fn(&[HostArgument]) -> Result<HostFnValue, HostFnError> + Send + Sync + 'static,
    ) {
        let key = ExternFnKey {
            plugin: PluginIdentity::Host(plugin),
            name,
        };
        self.metadata.signatures.insert(key.clone(), None);
        self.fns.insert(key, Arc::new(function));
    }

    /// Test-only: register a trusted host-native closure with no provided
    /// signature, as the built-in demo registry does.
    #[cfg(feature = "test-internals")]
    pub fn register_for_test(
        &mut self,
        plugin: PluginPath,
        name: FnName,
        function: impl Fn(&[HostArgument]) -> Result<HostFnValue, HostFnError> + Send + Sync + 'static,
    ) {
        self.register(plugin, name, function);
    }

    /// Register a plugin-backed closure together with the signature its
    /// manifest declares; the pipeline verifies declarations against it.
    pub fn register_with_signature(
        &mut self,
        plugin: PluginIdentity,
        name: FnName,
        signature: FunctionSignature,
        function: impl Fn(&[HostArgument]) -> Result<HostFnValue, HostFnError> + Send + Sync + 'static,
    ) {
        let key = ExternFnKey { plugin, name };
        self.metadata
            .signatures
            .insert(key.clone(), Some(signature));
        self.fns.insert(key, Arc::new(function));
    }

    /// Record that a plugin's functions could not be registered at all.
    ///
    /// The pipeline reports this (with the import site's span) before any
    /// per-function "missing host function" diagnostic, so users see the
    /// root cause.
    pub fn record_plugin_failure(
        &mut self,
        plugin: PluginIdentity,
        error: PluginRegistrationError,
    ) {
        self.metadata.failed_plugins.insert(plugin, error);
    }

    /// Clone the non-callable metadata view used by static compilation.
    #[must_use]
    pub fn metadata(&self) -> HostFunctionMetadata {
        self.metadata.clone()
    }

    /// Look up the host closure for an extern function.
    #[must_use]
    pub(crate) fn get(&self, key: &ExternFnKey) -> Option<&HostFn> {
        self.fns.get(key)
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
/// The quantity argument at `position`.
fn quantity_argument(args: &[HostArgument], position: usize) -> Result<f64, HostFnError> {
    match args.get(position) {
        Some(HostArgument::Scalar(HostScalar::Quantity(value))) => Ok(value.get()),
        _ => Err(HostFnError::new(format!(
            "argument {position} is not a single quantity slot"
        ))),
    }
}

/// The quantity array argument at `position`: its extents and SI values.
fn quantity_array_argument(
    args: &[HostArgument],
    position: usize,
) -> Result<(&[usize], Vec<f64>), HostFnError> {
    match args.get(position) {
        Some(HostArgument::Array(array)) => match array.elements() {
            HostArrayElements::Quantity(values) => Ok((
                array.shape(),
                values.iter().map(|value| value.get()).collect(),
            )),
            HostArrayElements::Bool(_) | HostArrayElements::Int(_) => Err(HostFnError::new(
                format!("argument {position} is not a quantity array"),
            )),
        },
        _ => Err(HostFnError::new(format!(
            "argument {position} is not an array"
        ))),
    }
}

fn checked_demo_result(value: f64, function: &str) -> Result<f64, HostFnError> {
    crate::eval_expr::numeric::computed_finite_quantity(value, function)
        .map(graphcal_compiler::finite_value::FiniteQuantity::get)
        .map_err(|error| HostFnError::new(error.to_string()))
}

fn demo_lerp(args: &[HostArgument]) -> Result<HostFnValue, HostFnError> {
    let (a, b, t) = (
        quantity_argument(args, 0)?,
        quantity_argument(args, 1)?,
        quantity_argument(args, 2)?,
    );
    let interpolated = (1.0 - t).mul_add(a, t * b);
    Ok(HostFnValue::F64(checked_demo_result(
        interpolated,
        "lerp()",
    )?))
}

fn demo_geometric_mean(args: &[HostArgument]) -> Result<HostFnValue, HostFnError> {
    let x = quantity_argument(args, 0)?;
    let y = quantity_argument(args, 1)?;
    if x.is_sign_negative() != y.is_sign_negative() && x != 0.0 && y != 0.0 {
        return Err(HostFnError::new(
            "geometric mean of a negative product is undefined",
        ));
    }
    let mean = x.abs().sqrt() * y.abs().sqrt();
    Ok(HostFnValue::F64(checked_demo_result(
        mean,
        "geometric_mean()",
    )?))
}

fn demo_normalize(args: &[HostArgument]) -> Result<HostFnValue, HostFnError> {
    let (shape, values) = quantity_array_argument(args, 0)?;
    let total = crate::eval_expr::numeric::ScaledSum::from_values(&values, "normalize() input")
        .map_err(|error| HostFnError::new(error.to_string()))?;
    if total.is_zero() {
        return Err(HostFnError::new(
            "cannot normalize: the elements sum to zero",
        ));
    }
    let normalized = values
        .iter()
        .map(|value| {
            total
                .normalized_ratio(*value, "normalize()")
                .map(graphcal_compiler::finite_value::FiniteQuantity::get)
                .map_err(|error| HostFnError::new(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HostFnValue::Array(HostArray::try_new(
        shape.to_vec(),
        normalized,
    )?))
}

#[must_use]
pub fn demo_registry() -> HostFunctionRegistry {
    let plugin = PluginPath::new(DEMO_PLUGIN_PATH);
    let mut registry = HostFunctionRegistry::new();
    registry.register(plugin.clone(), FnName::expect_valid("lerp"), demo_lerp);
    registry.register(plugin.clone(), FnName::expect_valid("inverse"), |args| {
        let x = quantity_argument(args, 0)?;
        if x == 0.0 {
            return Err(HostFnError::new("division by zero"));
        }
        Ok(HostFnValue::F64(x.recip()))
    });
    registry.register(
        plugin.clone(),
        FnName::expect_valid("geometric_mean"),
        demo_geometric_mean,
    );
    registry.register(
        plugin.clone(),
        FnName::expect_valid("normalize"),
        demo_normalize,
    );
    registry.register(
        plugin.clone(),
        FnName::expect_valid("matrix_transpose"),
        |args| {
            let (shape, matrix) = quantity_array_argument(args, 0)?;
            let matrix = matrix.as_slice();
            let [rows, columns] = shape else {
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
                        matrix.get(offset).copied().ok_or_else(|| {
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
        let (_, xs) = quantity_array_argument(args, 0)?;
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        for x in &xs {
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
    use crate::host_abi::argument::HostArgumentArray;

    fn key(name: &str) -> ExternFnKey {
        ExternFnKey {
            plugin: PluginIdentity::Host(PluginPath::new(DEMO_PLUGIN_PATH)),
            name: FnName::expect_valid(name),
        }
    }

    fn quantity(value: f64) -> HostArgument {
        HostArgument::Scalar(HostScalar::Quantity(
            graphcal_compiler::finite_value::FiniteQuantity::try_new(value).unwrap(),
        ))
    }

    fn quantities(values: &[f64]) -> Vec<HostArgument> {
        values.iter().copied().map(quantity).collect()
    }

    fn quantity_vector(values: &[f64]) -> HostArgument {
        let elements = values
            .iter()
            .map(|value| graphcal_compiler::finite_value::FiniteQuantity::try_new(*value).unwrap())
            .collect();
        HostArgument::Array(
            HostArgumentArray::try_new(vec![values.len()], HostArrayElements::Quantity(elements))
                .unwrap(),
        )
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
            assert!(
                registry.metadata().contains(&key(name)),
                "missing demo fn `{name}`"
            );
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
    fn demo_kernels_preserve_extreme_finite_results() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        assert_eq!(
            lerp(&quantities(&[-1.0e308, 1.0e308, 0.5])).unwrap(),
            HostFnValue::F64(0.0)
        );

        let geometric_mean = registry.get(&key("geometric_mean")).unwrap();
        assert_eq!(
            geometric_mean(&quantities(&[1.0e308, 1.0e308])).unwrap(),
            HostFnValue::F64(1.0e308)
        );

        let normalize = registry.get(&key("normalize")).unwrap();
        assert_eq!(
            normalize(&[quantity_vector(&[1.0e308, 1.0e308])]).unwrap(),
            HostFnValue::Array(HostArray::vector(vec![0.5, 0.5]).unwrap())
        );
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
        let result = normalize(&[quantity_vector(&[1.0, 3.0])]).unwrap();
        assert_eq!(
            result,
            HostFnValue::Array(HostArray::vector(vec![0.25, 0.75]).unwrap())
        );
    }

    #[test]
    fn arguments_have_the_raw_wire_form_a_function_would_return() {
        assert_eq!(
            HostFnValue::from_argument(&HostArgument::Scalar(HostScalar::Bool(true))),
            HostFnValue::F64(1.0)
        );
        assert_eq!(
            HostFnValue::from_argument(&quantity_vector(&[1.5, -2.0])),
            HostFnValue::Array(HostArray::vector(vec![1.5, -2.0]).unwrap())
        );
    }

    #[test]
    fn shape_mismatches_are_reported_not_reinterpreted() {
        let registry = demo_registry();
        let lerp = registry.get(&key("lerp")).unwrap();
        let err = lerp(&[quantity_vector(&[1.0]), quantity(1.0), quantity(0.5)]).unwrap_err();
        assert!(err.message.contains("not a single quantity slot"), "{err}");
    }
}

//! Function signatures rendered for Signature Help.

use std::collections::HashMap;
use std::sync::LazyLock;

use graphcal_compiler::builtin::{BuiltinEntry, BuiltinFn};
use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::dimension::Dimension;
use graphcal_compiler::function_signature::FunctionSignature;
use graphcal_compiler::semantic::scalar_function::scalar_function;
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::names::NamePath;

/// Structured function signature for Signature Help.
pub struct FnSignatureInfo {
    /// Full signature label, e.g. `"fn sqrt<D: Dim>(x: D) -> D^(1/2)"`.
    pub label: String,
    /// Individual parameter labels, e.g. `["x: D"]`.
    pub parameters: Vec<String>,
}

/// Signature Help lookup key of an extern (plugin) function: the
/// `alias::name` call spelling, kept as its two typed parts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternCallee {
    /// The module alias the file bound the plugin to.
    pub alias: ModuleAliasName,
    /// The function leaf name inside the plugin.
    pub name: FnName,
}

impl ExternCallee {
    /// Classify a call-site source path as an extern callee.
    ///
    /// Only the `alias::name` shape (exactly one owner segment) can name an
    /// extern function; any other path yields `None`.
    #[must_use]
    pub fn from_call_path(path: &NamePath) -> Option<Self> {
        match path.qualifier() {
            [alias] => Some(Self {
                alias: ModuleAliasName::classify(alias.clone()),
                name: FnName::classify(path.leaf().clone()),
            }),
            _ => None,
        }
    }
}

impl std::fmt::Display for ExternCallee {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}", self.alias, self.name)
    }
}

/// Build extern (plugin) function signatures for Signature Help, keyed by
/// the typed `alias::name` call spelling.
///
/// Unlike builtins, extern signatures are per-file (they depend on the
/// file's `import plugin` blocks and its registry's dimension names).
pub fn build_extern_fn_signatures(
    tir: &graphcal_compiler::tir::typed::CheckedTir,
    cancellation: &CancellationToken,
) -> std::result::Result<HashMap<ExternCallee, FnSignatureInfo>, Cancelled> {
    let mut format_dim = |dim: &Dimension| tir.registry().dimensions.format_dimension(dim);
    let mut sigs = HashMap::new();
    for function in tir.extern_functions().values() {
        cancellation.checkpoint()?;
        let parameters: Vec<String> = function
            .signature
            .params()
            .iter()
            .map(|param| param.format_with(&mut format_dim))
            .collect();
        let rendered = function
            .signature
            .format_with_result(&mut format_dim, &mut |result_struct, _| {
                result_struct.record_type().as_str().to_string()
            });
        let callee = ExternCallee {
            alias: function.alias.clone(),
            name: function.name.clone(),
        };
        let label = format!("fn {callee}{rendered}");
        sigs.insert(callee, FnSignatureInfo { label, parameters });
    }
    Ok(sigs)
}

/// Get builtin function signatures for Signature Help.
///
/// Computed once and cached in a static. Builtins never change at runtime.
pub fn build_fn_signatures() -> &'static HashMap<String, FnSignatureInfo> {
    static FN_SIGS: LazyLock<HashMap<String, FnSignatureInfo>> = LazyLock::new(|| {
        let mut sigs = HashMap::new();
        for function in BuiltinFn::all() {
            let info = match function.entry() {
                BuiltinEntry::Kernel(scalar) => {
                    builtin_kernel_signature_info(function, scalar_function(scalar).signature())
                }
                BuiltinEntry::Signature(signature) => FnSignatureInfo {
                    label: signature.label(function.as_str()),
                    parameters: signature.parameter_labels().collect(),
                },
                BuiltinEntry::Bespoke(_) => continue,
            };
            sigs.insert(function.as_str().to_string(), info);
        }
        sigs
    });
    &FN_SIGS
}

/// Render a builtin kernel signature through the shared signature renderer.
///
/// Builtin signatures reference only prelude dimensions, so the canonical
/// [`Dimension`] display needs no per-file registry.
pub fn builtin_kernel_signature_info(
    function: BuiltinFn,
    signature: &FunctionSignature,
) -> FnSignatureInfo {
    let mut format_dim = |dim: &Dimension| dim.to_string();
    FnSignatureInfo {
        label: format!("fn {function}{}", signature.format_with(&mut format_dim)),
        parameters: signature
            .params()
            .iter()
            .map(|param| param.format_with(&mut format_dim))
            .collect(),
    }
}

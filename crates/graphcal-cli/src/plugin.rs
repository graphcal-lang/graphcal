//! Functional core for the `graphcal plugin` commands.
//!
//! `new` produces a scaffold *plan* (paths and contents) and `test`
//! produces typed reports and rendered text; the binary shell in `main.rs`
//! does the disk writes and printing. Signature rendering here is a
//! display boundary: the output is `.gcl`-valid extern-declaration syntax,
//! ready to paste into an `import plugin` block.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use graphcal_compiler::dimension::{Dimension, Rational};
use graphcal_compiler::function_signature::{
    DimMonomial, FunctionSignature, StructFieldKind, ValueKind,
};
use graphcal_compiler::registry::format::format_exponent;
use graphcal_compiler::syntax::token::{SourceIdentifier, SourceIdentifierError};
use graphcal_eval::eval::format_number;
use graphcal_eval::host_abi::{
    ValidatedHostFieldValue, ValidatedHostResult, decode_result, encode_bool, encode_int,
    validate_quantity,
};
use graphcal_eval::host_fns::{HostArray, HostFnValue};
use graphcal_plugin_host::PluginModule;
use thiserror::Error;

// ---------------------------------------------------------------------------
// `graphcal plugin new`
// ---------------------------------------------------------------------------

/// The files `graphcal plugin new` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldPlan {
    /// Directory the scaffold is rooted at.
    pub root: PathBuf,
    /// Files to create, relative to `root`.
    pub files: Vec<ScaffoldFile>,
}

/// One file of the scaffold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    /// Path relative to the scaffold root.
    pub relative_path: &'static str,
    /// Full file contents.
    pub contents: String,
}

/// Reject invalid plugin crate names before cargo has to.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScaffoldNameError {
    /// The name is empty.
    #[error("plugin name cannot be empty")]
    Empty,
    /// The name uses characters outside the portable crate-name set.
    #[error(
        "plugin name `{name}` must start with a lowercase letter and contain only lowercase \
         letters, digits, `-`, or `_`"
    )]
    InvalidCharacters {
        /// The rejected name.
        name: String,
    },
}

/// Build the scaffold plan for a new plugin crate.
///
/// `dir` overrides the target directory (default: `./<name>`).
///
/// # Errors
///
/// Returns [`ScaffoldNameError`] when `name` is not a portable crate name.
pub fn scaffold_plan(name: &str, dir: Option<&Path>) -> Result<ScaffoldPlan, ScaffoldNameError> {
    validate_name(name)?;
    let root = dir.map_or_else(|| PathBuf::from(name), Path::to_path_buf);
    // Cargo names the cdylib artifact after the crate with `-` mapped to `_`.
    let artifact = name.replace('-', "_");

    let files = vec![
        cargo_toml_file(name),
        toolchain_file(),
        gitignore_file(),
        justfile_file(&artifact),
        lib_rs_file(),
        readme_file(name, &artifact),
    ];
    Ok(ScaffoldPlan { root, files })
}

fn cargo_toml_file(name: &str) -> ScaffoldFile {
    let sdk_version = env!("CARGO_PKG_VERSION");
    ScaffoldFile {
        relative_path: "Cargo.toml",
        contents: format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
# cdylib: the wasm plugin module; rlib: lets `cargo test` link the kernels.
crate-type = ["cdylib", "rlib"]

[dependencies]
graphcal-plugin = "={sdk_version}"

[profile.release]
codegen-units = 1
lto = true
# The SDK forwards panic messages to graphcal before the abort runtime
# runs, so `abort` sheds unwinding machinery without losing diagnostics.
panic = "abort"
strip = "debuginfo"
"#
        ),
    }
}

fn toolchain_file() -> ScaffoldFile {
    ScaffoldFile {
        relative_path: "rust-toolchain.toml",
        contents: r#"[toolchain]
channel = "stable"
targets = ["wasm32-unknown-unknown"]
"#
        .to_string(),
    }
}

fn gitignore_file() -> ScaffoldFile {
    ScaffoldFile {
        relative_path: ".gitignore",
        contents: "/target\n".to_string(),
    }
}

fn justfile_file(artifact: &str) -> ScaffoldFile {
    ScaffoldFile {
        relative_path: "justfile",
        contents: format!(
            r#"# Build the wasm plugin module.
build:
    cargo build --release --target wasm32-unknown-unknown
    @echo "artifact: target/wasm32-unknown-unknown/release/{artifact}.wasm"

# Run the native unit tests.
test:
    cargo test
"#
        ),
    }
}

fn lib_rs_file() -> ScaffoldFile {
    ScaffoldFile {
        relative_path: "src/lib.rs",
        contents: r#"//! A graphcal plugin: pure kernels with dimensional signatures.
//!
//! Build with `cargo build --release --target wasm32-unknown-unknown`
//! (or `just build`), then vendor the artifact into your graphcal project.

graphcal_plugin::plugin! {
    /// Linear interpolation between `a` and `b`.
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D {
        (b - a).mul_add(t, a)
    }

    /// Square root with an explicit domain failure. Values cross the
    /// boundary in SI base units.
    fn checked_sqrt(x: Dimensionless) -> Dimensionless {
        if x < 0.0 {
            graphcal_plugin::fail!("checked_sqrt: negative input {x}");
        }
        x.sqrt()
    }

    /// Each element's share of the total. Array parameters carry an
    /// explicit shape and flattened row-major values.
    fn share<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I] {
        let total: f64 = xs.iter().sum();
        if total == 0.0 {
            graphcal_plugin::fail!("share: the elements sum to zero");
        }
        let values = xs.iter().map(|x| x / total).collect();
        graphcal_plugin::Array::new(xs.shape().to_vec(), values)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lerp_interpolates() {
        assert!((super::lerp(0.0, 10.0, 0.25) - 2.5).abs() < 1e-12);
    }

    #[test]
    #[should_panic(expected = "negative input")]
    fn checked_sqrt_rejects_negatives() {
        let _ = super::checked_sqrt(-1.0);
    }

    #[test]
    fn share_normalizes() {
        let values = [1.0, 3.0];
        let xs = graphcal_plugin::ArrayView::new(&[2], &values).unwrap();
        assert_eq!(
            super::share(xs),
            graphcal_plugin::Array::vector(vec![0.25, 0.75]).unwrap()
        );
    }
}
"#
        .to_string(),
    }
}

fn readme_file(name: &str, artifact: &str) -> ScaffoldFile {
    ScaffoldFile {
        relative_path: "README.md",
        contents: format!(
            r#"# {name}

A [graphcal](https://github.com/graphcal-lang/graphcal) plugin: pure quantity
kernels with dimensional signatures, compiled to WebAssembly.

## Build

```sh
cargo build --release --target wasm32-unknown-unknown
```

The module is written to
`target/wasm32-unknown-unknown/release/{artifact}.wasm`.

## Test

Kernels are plain Rust natively, so `cargo test` works as usual. To
validate the built module against the plugin ABI and call it directly:

```sh
graphcal plugin test target/wasm32-unknown-unknown/release/{artifact}.wasm \
    --call lerp 0.0 10.0 0.25
```

## Use from a graphcal project

Vendor the module (for example under `plugins/`), declare it, and pin it:

```text
import plugin "plugins/{artifact}.wasm" as {artifact} {{
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
    fn checked_sqrt(x: Dimensionless) -> Dimensionless;
    fn share<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I];
}}

node mid: Length = {artifact}.lerp(1.0 m, 3.0 m, 0.5);
```

```sh
graphcal deps lock   # records the module's SHA-256 in graphcal.lock
```

Quantity values cross the plugin boundary as `f64`s in SI base units; keep
kernel math in SI throughout.
"#
        ),
    }
}

fn validate_name(name: &str) -> Result<(), ScaffoldNameError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(ScaffoldNameError::Empty);
    };
    let valid = first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(ScaffoldNameError::InvalidCharacters {
            name: name.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// `graphcal plugin test`
// ---------------------------------------------------------------------------

/// A failure to turn a validated plugin module into paste-ready Graphcal source.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImportBlockRenderError {
    /// Graphcal string literals currently have no escape syntax.
    #[error(
        "plugin import path {path:?} contains a quote or line break, which Graphcal string literals cannot represent"
    )]
    UnrepresentablePath {
        /// The rejected path.
        path: String,
    },
    /// A manifest function name is broader than a source identifier.
    #[error("plugin function name {name:?} {reason}")]
    InvalidFunctionName {
        /// The rejected wire name.
        name: String,
        /// Why source rendering rejected it.
        reason: SourceIdentifierError,
    },
    /// A manifest parameter name is broader than a source identifier.
    #[error("parameter name {name:?} of plugin function {function:?} {reason}")]
    InvalidParameterName {
        /// The containing function.
        function: String,
        /// The rejected wire name.
        name: String,
        /// Why source rendering rejected it.
        reason: SourceIdentifierError,
    },
    /// A manifest dimension variable is broader than a source identifier.
    #[error("dimension variable {name:?} of plugin function {function:?} {reason}")]
    InvalidDimensionVariable {
        /// The containing function.
        function: String,
        /// The rejected wire name.
        name: String,
        /// Why source rendering rejected it.
        reason: SourceIdentifierError,
    },
    /// A manifest index variable is broader than a source identifier.
    #[error("index variable {name:?} of plugin function {function:?} {reason}")]
    InvalidIndexVariable {
        /// The containing function.
        function: String,
        /// The rejected wire name.
        name: String,
        /// Why source rendering rejected it.
        reason: SourceIdentifierError,
    },
    /// A manifest result field is broader than a source identifier.
    #[error("result field name {name:?} of plugin function {function:?} {reason}")]
    InvalidResultFieldName {
        /// The containing function.
        function: String,
        /// The rejected wire name.
        name: String,
        /// Why source rendering rejected it.
        reason: SourceIdentifierError,
    },
    /// A source-valid function unexpectedly could not produce a result type name.
    #[error("could not derive a result type name from plugin function {function:?}: {reason}")]
    InvalidGeneratedResultTypeName {
        /// The containing function.
        function: String,
        /// Why the derived source name was rejected.
        reason: SourceIdentifierError,
    },
    /// The finite set of generated result names was unexpectedly exhausted.
    #[error("could not allocate a unique result type name for plugin function {function:?}")]
    ResultTypeNameExhausted {
        /// The containing function.
        function: String,
    },
}

/// A Graphcal string-literal payload proven representable by the current lexer.
#[derive(Debug, Clone, Copy)]
struct SourceStringLiteral<'a>(&'a str);

impl<'a> SourceStringLiteral<'a> {
    fn parse(path: &'a str) -> Result<Self, ImportBlockRenderError> {
        if path.contains(['"', '\r', '\n']) {
            return Err(ImportBlockRenderError::UnrepresentablePath {
                path: path.to_string(),
            });
        }
        Ok(Self(path))
    }

    const fn as_str(self) -> &'a str {
        self.0
    }
}

/// One function after every source spelling in its signature has been validated.
struct RenderableFunction<'a> {
    name: SourceIdentifier,
    signature: &'a FunctionSignature,
    result_type_name: Option<SourceIdentifier>,
}

/// A complete import that cannot contain source-breaking manifest text.
struct RenderableImport<'a> {
    path: SourceStringLiteral<'a>,
    alias: &'a SourceIdentifier,
    functions: Vec<RenderableFunction<'a>>,
}

impl<'a> RenderableImport<'a> {
    fn try_new(
        path: &'a str,
        alias: &'a SourceIdentifier,
        module: &'a PluginModule,
    ) -> Result<Self, ImportBlockRenderError> {
        let path = SourceStringLiteral::parse(path)?;
        let mut used_result_names = HashSet::new();
        let functions = module
            .functions()
            .iter()
            .map(|(function, signature)| {
                let function_name = parse_function_name(function.as_str())?;
                validate_signature_names(function.as_str(), signature)?;
                let result_type_name = matches!(signature.result(), ValueKind::Struct(_))
                    .then(|| {
                        allocate_result_type_name(function_name.as_str(), &mut used_result_names)
                    })
                    .transpose()?;
                Ok(RenderableFunction {
                    name: function_name,
                    signature,
                    result_type_name,
                })
            })
            .collect::<Result<_, ImportBlockRenderError>>()?;
        Ok(Self {
            path,
            alias,
            functions,
        })
    }

    fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for function in &self.functions {
            if let (Some(type_name), ValueKind::Struct(shape)) =
                (&function.result_type_name, function.signature.result())
            {
                out.push_str(&render_result_type_decl(type_name, shape));
                out.push_str("\n\n");
            }
        }
        let _ = writeln!(
            out,
            "import plugin \"{}\" as {} {{",
            self.path.as_str(),
            self.alias
        );
        for function in &self.functions {
            out.push_str("    ");
            out.push_str(&render_declaration(function));
            out.push('\n');
        }
        out.push('}');
        out
    }
}

/// Render the paste-ready `import plugin` block for a loaded module.
///
/// Suggested record declarations for struct-returning functions precede the
/// import. Colliding suggestions receive deterministic numeric suffixes.
///
/// # Errors
///
/// Returns [`ImportBlockRenderError`] rather than interpolating a manifest name
/// or path that Graphcal source cannot represent.
pub fn render_import_block(
    path: &str,
    alias: &SourceIdentifier,
    module: &PluginModule,
) -> Result<String, ImportBlockRenderError> {
    RenderableImport::try_new(path, alias, module).map(|source| source.render())
}

/// Derive a source-valid import alias from the module's file name.
///
/// # Errors
///
/// Returns [`SourceIdentifierError`] if the sanitized alias cannot be represented
/// by the source identifier grammar.
pub fn suggest_alias(module_path: &Path) -> Result<SourceIdentifier, SourceIdentifierError> {
    let stem = module_path
        .file_stem()
        .map(|stem| stem.to_string_lossy())
        .unwrap_or_default();
    let mut alias: String = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    SourceIdentifier::parse(alias.clone()).or_else(|_| {
        alias.insert_str(0, "plugin_");
        SourceIdentifier::parse(alias)
    })
}

fn parse_function_name(name: &str) -> Result<SourceIdentifier, ImportBlockRenderError> {
    SourceIdentifier::parse(name).map_err(|reason| ImportBlockRenderError::InvalidFunctionName {
        name: name.to_string(),
        reason,
    })
}

fn validate_signature_names(
    function: &str,
    signature: &FunctionSignature,
) -> Result<(), ImportBlockRenderError> {
    for variable in signature.dim_vars() {
        SourceIdentifier::parse(variable.as_str()).map_err(|reason| {
            ImportBlockRenderError::InvalidDimensionVariable {
                function: function.to_string(),
                name: variable.as_str().to_string(),
                reason,
            }
        })?;
    }
    for variable in signature.index_vars() {
        SourceIdentifier::parse(variable.as_str()).map_err(|reason| {
            ImportBlockRenderError::InvalidIndexVariable {
                function: function.to_string(),
                name: variable.as_str().to_string(),
                reason,
            }
        })?;
    }
    for parameter in signature.params() {
        SourceIdentifier::parse(parameter.name.as_str()).map_err(|reason| {
            ImportBlockRenderError::InvalidParameterName {
                function: function.to_string(),
                name: parameter.name.as_str().to_string(),
                reason,
            }
        })?;
    }
    if let ValueKind::Struct(shape) = signature.result() {
        for field in shape.fields() {
            SourceIdentifier::parse(field.name.as_str()).map_err(|reason| {
                ImportBlockRenderError::InvalidResultFieldName {
                    function: function.to_string(),
                    name: field.name.as_str().to_string(),
                    reason,
                }
            })?;
        }
    }
    Ok(())
}

fn allocate_result_type_name(
    function: &str,
    used: &mut HashSet<String>,
) -> Result<SourceIdentifier, ImportBlockRenderError> {
    let base = suggest_result_type_name(function);
    for collision_index in 0..=used.len() {
        let candidate = if collision_index == 0 {
            base.clone()
        } else {
            format!("{base}{}", collision_index + 1)
        };
        if used.insert(candidate.clone()) {
            return SourceIdentifier::parse(candidate).map_err(|reason| {
                ImportBlockRenderError::InvalidGeneratedResultTypeName {
                    function: function.to_string(),
                    reason,
                }
            });
        }
    }
    Err(ImportBlockRenderError::ResultTypeNameExhausted {
        function: function.to_string(),
    })
}

fn suggest_result_type_name(function: &str) -> String {
    let mut out = String::new();
    for part in function.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out.push_str("Result");
    out
}

fn render_result_type_decl(
    type_name: &SourceIdentifier,
    shape: &graphcal_compiler::function_signature::StructShape,
) -> String {
    let fields = shape
        .fields()
        .iter()
        .map(|field| format!("{}: {}", field.name, render_struct_field_kind(&field.kind)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("type {type_name} {{ {type_name}({fields}) }}")
}

fn render_declaration(function: &RenderableFunction<'_>) -> String {
    use std::fmt::Write as _;

    let signature = function.signature;
    let mut out = format!("fn {}", function.name);
    if !signature.dim_vars().is_empty() || !signature.index_vars().is_empty() {
        let binders = signature
            .dim_vars()
            .iter()
            .map(|variable| format!("{}: Dim", variable.as_str()))
            .chain(
                signature
                    .index_vars()
                    .iter()
                    .map(|variable| format!("{}: Index", variable.as_str())),
            )
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(out, "<{binders}>");
    }
    let parameters = signature
        .params()
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, render_value_kind(&parameter.kind)))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(out, "({parameters}) -> ");
    match (&function.result_type_name, signature.result()) {
        (Some(type_name), ValueKind::Struct(_)) => out.push_str(type_name.as_str()),
        (_, result) => out.push_str(&render_value_kind(result)),
    }
    out.push(';');
    out
}

fn render_value_kind(kind: &ValueKind) -> String {
    match kind {
        ValueKind::Bool => "Bool".to_string(),
        ValueKind::Int => "Int".to_string(),
        ValueKind::Quantity(monomial) => render_monomial(monomial),
        ValueKind::Indexed { element, indexes } => format!(
            "{}[{}]",
            render_monomial(element),
            indexes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ValueKind::Struct(shape) => format!(
            "{{ {} }}",
            shape
                .fields()
                .iter()
                .map(|field| format!("{}: {}", field.name, render_struct_field_kind(&field.kind)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn render_struct_field_kind(kind: &StructFieldKind) -> String {
    match kind {
        StructFieldKind::Bool => "Bool".to_string(),
        StructFieldKind::Int => "Int".to_string(),
        StructFieldKind::Quantity(dimension) if dimension.is_dimensionless() => {
            "Dimensionless".to_string()
        }
        StructFieldKind::Quantity(dimension) => render_dimension(dimension),
    }
}

fn render_monomial(monomial: &DimMonomial) -> String {
    let mut parts = monomial
        .vars
        .iter()
        .map(|factor| {
            if factor.power == Rational::ONE {
                factor.var.to_string()
            } else {
                format!("{}{}", factor.var, format_exponent(factor.power))
            }
        })
        .collect::<Vec<_>>();
    if !monomial.fixed.is_dimensionless() {
        parts.push(render_dimension(&monomial.fixed));
    }
    if parts.is_empty() {
        "Dimensionless".to_string()
    } else {
        parts.join(" * ")
    }
}

/// Render a concrete dimension in `.gcl` dimension-expression syntax
/// (`Mass * Length^-3`, `Length^(1/2)`), or `Dimensionless` for the empty
/// product.
fn render_dimension(dim: &Dimension) -> String {
    if dim.is_dimensionless() {
        return "Dimensionless".to_string();
    }
    let factors: Vec<String> = dim
        .iter()
        .map(|(id, power)| {
            let name = id.name();
            if *power == graphcal_compiler::dimension::Rational::ONE {
                name.to_string()
            } else {
                format!("{name}{}", format_exponent(*power))
            }
        })
        .collect();
    factors.join(" * ")
}

/// Error turning `--call` arguments into ABI values.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CallArgError {
    /// Wrong number of arguments for the signature.
    #[error("function `{function}` takes {expected} argument(s), got {got}")]
    ArityMismatch {
        /// The called function.
        function: String,
        /// The signature's parameter count.
        expected: usize,
        /// The provided argument count.
        got: usize,
    },
    /// One argument failed to parse for its declared kind.
    #[error("argument `{argument}` for parameter `{param}`: {expected}")]
    InvalidArgument {
        /// The raw argument text.
        argument: String,
        /// The parameter it was bound to.
        param: String,
        /// What the declared kind expects.
        expected: String,
    },
}

fn parse_dense_array(text: &str, rank: usize) -> Result<HostArray, String> {
    fn flatten(value: &serde_json::Value, rank: usize) -> Result<(Vec<usize>, Vec<f64>), String> {
        if rank == 0 {
            let value = value
                .as_f64()
                .ok_or_else(|| "array leaves must be JSON numbers".to_string())?;
            let finite = validate_quantity(value).map_err(|error| error.to_string())?;
            return Ok((Vec::new(), vec![finite.get()]));
        }
        let items = value
            .as_array()
            .ok_or_else(|| format!("expected {rank} more array level(s)"))?;
        if items.is_empty() {
            return Err("array axes cannot be empty".to_string());
        }
        let children = items
            .iter()
            .map(|item| flatten(item, rank - 1))
            .collect::<Result<Vec<_>, _>>()?;
        let first_shape = children
            .first()
            .map(|(shape, _)| shape)
            .ok_or_else(|| "array axes cannot be empty".to_string())?;
        if children.iter().any(|(shape, _)| shape != first_shape) {
            return Err("array must be rectangular".to_string());
        }
        let mut shape = Vec::with_capacity(rank);
        shape.push(items.len());
        shape.extend(first_shape.iter().copied());
        let values = children
            .into_iter()
            .flat_map(|(_, values)| values)
            .collect();
        Ok((shape, values))
    }

    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("invalid JSON array: {error}"))?;
    let (shape, values) = flatten(&value, rank)?;
    HostArray::try_new(shape, values).map_err(|error| error.to_string())
}

fn render_dense_array(shape: &[usize], values: &[f64]) -> Result<String, String> {
    match shape {
        [] => Err("array result has no axes".to_string()),
        [_] => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(|value| format_number(*value))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        [extent, remaining @ ..] => {
            let child_len = remaining
                .iter()
                .try_fold(1_usize, |size, extent| size.checked_mul(*extent));
            let Some(child_len) = child_len else {
                return Err("array result shape cardinality overflowed".to_string());
            };
            let chunks = values.chunks_exact(child_len);
            if !chunks.remainder().is_empty() || chunks.len() != *extent {
                return Err("array result values do not match its shape".to_string());
            }
            let children = chunks
                .map(|chunk| render_dense_array(remaining, chunk))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("[{}]", children.join(", ")))
        }
    }
}

/// Parse `--call` arguments against the signature's parameter kinds:
/// quantities as (SI) floats, `Bool` as `true`/`false`, `Int` as an integer,
/// and rank-`R` arrays as rectangular JSON arrays with `R` nesting levels.
pub fn parse_call_args(
    function: &str,
    signature: &FunctionSignature,
    raw: &[String],
) -> Result<Vec<HostFnValue>, CallArgError> {
    if raw.len() != signature.arity() {
        return Err(CallArgError::ArityMismatch {
            function: function.to_string(),
            expected: signature.arity(),
            got: raw.len(),
        });
    }
    signature
        .params()
        .iter()
        .zip(raw)
        .map(|(param, text)| {
            let invalid = |expected: &str| CallArgError::InvalidArgument {
                argument: text.clone(),
                param: param.name.to_string(),
                expected: expected.to_string(),
            };
            match &param.kind {
                ValueKind::Bool => match text.as_str() {
                    "true" => Ok(HostFnValue::F64(encode_bool(true))),
                    "false" => Ok(HostFnValue::F64(encode_bool(false))),
                    _ => Err(invalid("expected `true` or `false`")),
                },
                ValueKind::Int => {
                    let value: i64 = text.parse().map_err(|_| invalid("expected an integer"))?;
                    encode_int(value)
                        .map(HostFnValue::F64)
                        .map_err(|error| invalid(&error.to_string()))
                }
                ValueKind::Quantity(_) => {
                    let value = text
                        .parse::<f64>()
                        .map_err(|_| invalid("expected a number (in SI base units)"))?;
                    validate_quantity(value)
                        .map(|value| HostFnValue::F64(value.get()))
                        .map_err(|error| invalid(&error.to_string()))
                }
                // Struct parameters never pass signature validation.
                ValueKind::Struct(_) => Err(invalid("struct parameters are not supported")),
                ValueKind::Indexed { indexes, .. } => parse_dense_array(text, indexes.len())
                    .map(HostFnValue::Array)
                    .map_err(|error| invalid(&error)),
            }
        })
        .collect()
}

/// Render a call's result value per the declared result kind.
///
/// Returns `Err` with a description when the value violates the declared
/// kind's encoding (a plugin bug worth surfacing, not reinterpreting).
pub fn render_result(signature: &FunctionSignature, value: &HostFnValue) -> Result<String, String> {
    match decode_result(signature.result(), value).map_err(|error| error.to_string())? {
        ValidatedHostResult::Bool(value) => Ok(value.to_string()),
        ValidatedHostResult::Int(value) => Ok(value.to_string()),
        ValidatedHostResult::Quantity { dimension, value } => {
            let rendered = format_number(value.get());
            let dim = render_quantity_result_dimension(&dimension);
            Ok(match dim {
                Some(dim) => format!("{rendered} [{dim}, SI base units]"),
                None => rendered,
            })
        }
        ValidatedHostResult::Array(array) => {
            let values = array
                .values()
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>();
            let rendered = render_dense_array(array.shape(), &values)?;
            let dim = render_quantity_result_dimension(array.element());
            Ok(match dim {
                Some(dim) => format!("{rendered} [{dim}, SI base units]"),
                None => rendered,
            })
        }
        ValidatedHostResult::Struct(fields) => {
            let fields = fields
                .iter()
                .map(|field| {
                    let rendered = match field.value() {
                        ValidatedHostFieldValue::Bool(value) => value.to_string(),
                        ValidatedHostFieldValue::Int(value) => value.to_string(),
                        ValidatedHostFieldValue::Quantity(value) => format_number(value.get()),
                    };
                    format!("{}: {rendered}", field.name())
                })
                .collect::<Vec<_>>();
            Ok(format!("{{ {} }} [SI base units]", fields.join(", ")))
        }
    }
}

/// Describe a quantity result's dimension when it is concrete; dim-variable
/// results depend on the call site, so no fixed description exists.
fn render_quantity_result_dimension(
    monomial: &graphcal_compiler::function_signature::DimMonomial,
) -> Option<String> {
    if !monomial.vars.is_empty() {
        return None;
    }
    if monomial.fixed.is_dimensionless() {
        return None;
    }
    Some(render_dimension(&monomial.fixed))
}

#[cfg(test)]
mod tests {
    use graphcal_compiler::dimension::Rational;
    use graphcal_compiler::function_signature::{DimMonomial, FunctionParam};
    use graphcal_compiler::registry::prelude::prelude_base_dimension;
    use graphcal_compiler::syntax::dimension::DimVarName;
    use graphcal_compiler::syntax::function_name::FnParamName;

    use super::*;

    fn lerp_signature() -> FunctionSignature {
        let var = || DimVarName::expect_valid("D");
        FunctionSignature::try_new(
            vec![var()],
            Vec::new(),
            vec![
                FunctionParam {
                    name: FnParamName::expect_valid("a"),
                    kind: ValueKind::Quantity(DimMonomial::var(var())),
                },
                FunctionParam {
                    name: FnParamName::expect_valid("b"),
                    kind: ValueKind::Quantity(DimMonomial::var(var())),
                },
                FunctionParam {
                    name: FnParamName::expect_valid("t"),
                    kind: ValueKind::dimensionless(),
                },
            ],
            ValueKind::Quantity(DimMonomial::var(var())),
        )
        .expect("valid signature")
    }

    fn step_signature() -> FunctionSignature {
        FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![
                FunctionParam {
                    name: FnParamName::expect_valid("n"),
                    kind: ValueKind::Int,
                },
                FunctionParam {
                    name: FnParamName::expect_valid("up"),
                    kind: ValueKind::Bool,
                },
            ],
            ValueKind::Int,
        )
        .expect("valid signature")
    }

    #[test]
    fn scaffold_contains_the_expected_files() {
        let plan = scaffold_plan("fluid-props", None).unwrap();
        assert_eq!(plan.root, PathBuf::from("fluid-props"));
        let paths: Vec<&str> = plan.files.iter().map(|f| f.relative_path).collect();
        assert_eq!(
            paths,
            [
                "Cargo.toml",
                "rust-toolchain.toml",
                ".gitignore",
                "justfile",
                "src/lib.rs",
                "README.md"
            ]
        );
        let cargo = &plan.files[0].contents;
        assert!(cargo.contains("name = \"fluid-props\""), "{cargo}");
        assert!(
            cargo.contains(&format!(
                "graphcal-plugin = \"={}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "{cargo}"
        );
        assert!(
            cargo.contains("crate-type = [\"cdylib\", \"rlib\"]"),
            "{cargo}"
        );
        let readme = &plan.files[5].contents;
        assert!(readme.contains("plugins/fluid_props.wasm"), "{readme}");
        let toolchain = &plan.files[1].contents;
        assert!(toolchain.contains("wasm32-unknown-unknown"), "{toolchain}");
    }

    #[test]
    fn scaffold_rejects_bad_names() {
        assert_eq!(
            scaffold_plan("", None).unwrap_err(),
            ScaffoldNameError::Empty
        );
        assert!(matches!(
            scaffold_plan("Fluids", None).unwrap_err(),
            ScaffoldNameError::InvalidCharacters { .. }
        ));
        assert!(matches!(
            scaffold_plan("1fluids", None).unwrap_err(),
            ScaffoldNameError::InvalidCharacters { .. }
        ));
        assert!(matches!(
            scaffold_plan("flu ids", None).unwrap_err(),
            ScaffoldNameError::InvalidCharacters { .. }
        ));
    }

    #[test]
    fn scaffold_respects_dir_override() {
        let plan = scaffold_plan("fluids", Some(Path::new("plugins/fluids-src"))).unwrap();
        assert_eq!(plan.root, PathBuf::from("plugins/fluids-src"));
    }

    fn render_test_declaration(name: &str, signature: &FunctionSignature) -> String {
        let name = SourceIdentifier::parse(name).unwrap();
        let result_type_name = matches!(signature.result(), ValueKind::Struct(_))
            .then(|| SourceIdentifier::parse(suggest_result_type_name(name.as_str())).unwrap());
        render_declaration(&RenderableFunction {
            name,
            signature,
            result_type_name,
        })
    }

    #[test]
    fn declarations_render_in_gcl_syntax() {
        assert_eq!(
            render_test_declaration("lerp", &lerp_signature()),
            "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;"
        );
        assert_eq!(
            render_test_declaration("step", &step_signature()),
            "fn step(n: Int, up: Bool) -> Int;"
        );

        let pressure = prelude_base_dimension("Mass")
            .unwrap()
            .checked_mul(&prelude_base_dimension("Length").unwrap().pow(-1).unwrap())
            .unwrap()
            .checked_mul(&prelude_base_dimension("Time").unwrap().pow(-2).unwrap())
            .unwrap();
        let sqrt_len = prelude_base_dimension("Length")
            .unwrap()
            .pow(Rational::HALF)
            .unwrap();
        let signature = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![FunctionParam {
                name: FnParamName::expect_valid("p"),
                kind: ValueKind::Quantity(DimMonomial::fixed(pressure)),
            }],
            ValueKind::Quantity(DimMonomial::fixed(sqrt_len)),
        )
        .expect("valid signature");
        assert_eq!(
            render_test_declaration("weird", &signature),
            "fn weird(p: Length^-1 * Mass * Time^-2) -> Length^(1/2);"
        );
    }

    #[test]
    fn source_identifiers_reject_keywords_and_non_source_characters() {
        assert_eq!(
            SourceIdentifier::parse("node").unwrap_err(),
            SourceIdentifierError::ReservedKeyword
        );
        assert_eq!(
            SourceIdentifier::parse("x;\nnode injected").unwrap_err(),
            SourceIdentifierError::InvalidCharacters
        );
        assert_eq!(SourceIdentifier::parse("scan").unwrap().as_str(), "scan");
    }

    #[test]
    fn result_type_names_are_collision_free() {
        let mut used = HashSet::new();
        assert_eq!(
            allocate_result_type_name("solve_orbit", &mut used)
                .unwrap()
                .as_str(),
            "SolveOrbitResult"
        );
        assert_eq!(
            allocate_result_type_name("solve__orbit", &mut used)
                .unwrap()
                .as_str(),
            "SolveOrbitResult2"
        );
    }

    #[test]
    fn unrepresentable_plugin_paths_are_rejected() {
        assert!(matches!(
            SourceStringLiteral::parse("plugins/quoted\"name.wasm"),
            Err(ImportBlockRenderError::UnrepresentablePath { .. })
        ));
        assert!(matches!(
            SourceStringLiteral::parse("plugins/line\nbreak.wasm"),
            Err(ImportBlockRenderError::UnrepresentablePath { .. })
        ));
    }

    #[test]
    fn aliases_are_derived_from_file_names() {
        assert_eq!(
            suggest_alias(Path::new("plugins/fluid-props.wasm"))
                .unwrap()
                .as_str(),
            "fluid_props"
        );
        assert_eq!(
            suggest_alias(Path::new("x/3d.wasm")).unwrap().as_str(),
            "plugin_3d"
        );
        assert_eq!(
            suggest_alias(Path::new("x/node.wasm")).unwrap().as_str(),
            "plugin_node"
        );
    }

    #[test]
    fn call_args_parse_per_kind() {
        let args = parse_call_args(
            "lerp",
            &lerp_signature(),
            &["1.0".into(), "3.0".into(), "0.5".into()],
        )
        .unwrap();
        assert_eq!(
            args,
            [
                HostFnValue::F64(1.0),
                HostFnValue::F64(3.0),
                HostFnValue::F64(0.5)
            ]
        );

        let args =
            parse_call_args("step", &step_signature(), &["5".into(), "true".into()]).unwrap();
        assert_eq!(args, [HostFnValue::F64(5.0), HostFnValue::F64(1.0)]);

        assert!(matches!(
            parse_call_args("step", &step_signature(), &["5".into()]).unwrap_err(),
            CallArgError::ArityMismatch {
                expected: 2,
                got: 1,
                ..
            }
        ));
        assert!(matches!(
            parse_call_args("step", &step_signature(), &["5.5".into(), "true".into()]).unwrap_err(),
            CallArgError::InvalidArgument { .. }
        ));
        assert!(matches!(
            parse_call_args("step", &step_signature(), &["5".into(), "yes".into()]).unwrap_err(),
            CallArgError::InvalidArgument { .. }
        ));
        assert!(matches!(
            parse_call_args(
                "step",
                &step_signature(),
                &["9007199254740993".into(), "true".into()]
            )
            .unwrap_err(),
            CallArgError::InvalidArgument { .. }
        ));
        for non_finite in ["NaN", "inf", "-inf"] {
            assert!(matches!(
                parse_call_args(
                    "lerp",
                    &lerp_signature(),
                    &[non_finite.into(), "3.0".into(), "0.5".into()]
                )
                .unwrap_err(),
                CallArgError::InvalidArgument { .. }
            ));
        }
    }

    #[test]
    fn multi_axis_call_arrays_parse_and_render_rectangular_json() {
        let array = parse_dense_array("[[1, 2, 3], [4, 5, 6]]", 2).unwrap();
        assert_eq!(array.shape(), [2, 3]);
        assert_eq!(array.values(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(
            render_dense_array(array.shape(), array.values()).unwrap(),
            "[[1, 2, 3], [4, 5, 6]]"
        );
        assert!(parse_dense_array("[1, 2, 3]", 2).is_err());
        assert!(parse_dense_array("[[1, 2], [3]]", 2).is_err());
    }

    #[test]
    fn results_render_per_kind() {
        assert_eq!(
            render_result(&step_signature(), &HostFnValue::F64(42.0)).unwrap(),
            "42"
        );
        assert_eq!(
            render_result(&step_signature(), &HostFnValue::F64(-0.0)).unwrap(),
            "0"
        );
        for invalid in [42.5, 2.0_f64.powi(63), f64::INFINITY, f64::NAN] {
            assert!(render_result(&step_signature(), &HostFnValue::F64(invalid)).is_err());
        }

        let bool_result = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![FunctionParam {
                name: FnParamName::expect_valid("x"),
                kind: ValueKind::dimensionless(),
            }],
            ValueKind::Bool,
        )
        .expect("valid signature");
        assert_eq!(
            render_result(&bool_result, &HostFnValue::F64(1.0)).unwrap(),
            "true"
        );
        assert_eq!(
            render_result(&bool_result, &HostFnValue::F64(0.0)).unwrap(),
            "false"
        );
        assert_eq!(
            render_result(&bool_result, &HostFnValue::F64(-0.0)).unwrap(),
            "false"
        );
        for invalid in [0.5, f64::INFINITY, f64::NAN] {
            assert!(render_result(&bool_result, &HostFnValue::F64(invalid)).is_err());
        }

        let velocity = prelude_base_dimension("Length")
            .unwrap()
            .checked_mul(&prelude_base_dimension("Time").unwrap().pow(-1).unwrap())
            .unwrap();
        let quantity_result = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![FunctionParam {
                name: FnParamName::expect_valid("x"),
                kind: ValueKind::dimensionless(),
            }],
            ValueKind::Quantity(DimMonomial::fixed(velocity)),
        )
        .expect("valid signature");
        assert_eq!(
            render_result(&quantity_result, &HostFnValue::F64(2.5)).unwrap(),
            "2.5 [Length * Time^-1, SI base units]"
        );
        for invalid in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(render_result(&quantity_result, &HostFnValue::F64(invalid)).is_err());
        }
    }
}

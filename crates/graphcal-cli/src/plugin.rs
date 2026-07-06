//! Functional core for the `graphcal plugin` commands.
//!
//! `new` produces a scaffold *plan* (paths and contents) and `test`
//! produces typed reports and rendered text; the binary shell in `main.rs`
//! does the disk writes and printing. Signature rendering here is a
//! display boundary: the output is `.gcl`-valid extern-declaration syntax,
//! ready to paste into an `import plugin` block.

use std::path::{Path, PathBuf};

use graphcal_compiler::dimension::{BaseDimId, Dimension};
use graphcal_compiler::function_signature::{FunctionSignature, ValueKind};
use graphcal_compiler::registry::format::format_exponent;
use graphcal_eval::eval::format_number;
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
        contents: r#"//! A graphcal plugin: pure scalar kernels with dimensional signatures.
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

A [graphcal](https://github.com/graphcal-lang/graphcal) plugin: pure scalar
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
}}

node mid: Length = {artifact}.lerp(1.0 m, 3.0 m, 0.5);
```

```sh
graphcal deps lock   # records the module's SHA-256 in graphcal.lock
```

Scalar values cross the plugin boundary as `f64`s in SI base units; keep
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

/// Render one function as a `.gcl` extern declaration line (no leading
/// indentation): `fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;`.
pub fn render_declaration(name: &str, signature: &FunctionSignature) -> String {
    format!("fn {name}{};", signature.format_with(render_dimension))
}

/// Render the paste-ready `import plugin` block for a loaded module.
///
/// `path` is the verbatim path string for the import (callers pass the
/// CLI argument; users adjust it to the vendored location).
pub fn render_import_block(path: &str, alias: &str, module: &PluginModule) -> String {
    let mut out = format!("import plugin \"{path}\" as {alias} {{\n");
    for (name, signature) in module.functions() {
        out.push_str("    ");
        out.push_str(&render_declaration(name.as_str(), signature));
        out.push('\n');
    }
    out.push('}');
    out
}

/// Derive a usable import alias from the module's file name.
pub fn suggest_alias(module_path: &Path) -> String {
    let stem = module_path
        .file_stem()
        .map(|stem| stem.to_string_lossy())
        .unwrap_or_default();
    let mut alias: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if alias.is_empty() || alias.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        alias.insert_str(0, "plugin_");
    }
    alias
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
            let (BaseDimId::Prelude(name) | BaseDimId::UserDefined { name, .. }) = id;
            if *power == graphcal_compiler::dimension::Rational::ONE {
                name.clone()
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

/// Parse `--call` arguments against the signature's parameter kinds:
/// scalars as (SI) floats, `Bool` as `true`/`false`, `Int` as an integer.
pub fn parse_call_args(
    function: &str,
    signature: &FunctionSignature,
    raw: &[String],
) -> Result<Vec<f64>, CallArgError> {
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
                    "true" => Ok(1.0),
                    "false" => Ok(0.0),
                    _ => Err(invalid("expected `true` or `false`")),
                },
                ValueKind::Int => {
                    let value: i64 = text.parse().map_err(|_| invalid("expected an integer"))?;
                    int_to_abi(value)
                        .ok_or_else(|| invalid("integer is not exactly representable as an f64"))
                }
                ValueKind::Scalar(_) => text
                    .parse::<f64>()
                    .map_err(|_| invalid("expected a number (in SI base units)")),
                // ABI v1 manifests cannot declare arrays; `--call` support
                // arrives with the ABI v2 buffer protocol (issue #25 Phase D).
                ValueKind::Indexed { .. } => Err(invalid(
                    "array parameters are not supported by `--call` yet",
                )),
            }
        })
        .collect()
}

/// Render a call's raw `f64` result per the declared result kind.
///
/// Returns `Err` with a description when the value violates the declared
/// kind's encoding (a plugin bug worth surfacing, not reinterpreting).
pub fn render_result(signature: &FunctionSignature, raw: f64) -> Result<String, String> {
    match signature.result() {
        ValueKind::Bool => {
            if raw.to_bits() == 1.0_f64.to_bits() {
                Ok("true".to_string())
            } else if raw.to_bits() == 0.0_f64.to_bits() {
                Ok("false".to_string())
            } else {
                Err(format!(
                    "declared Bool result is not encoded as 1.0/0.0: got {raw}"
                ))
            }
        }
        ValueKind::Int => int_from_abi(raw)
            .map(|value| value.to_string())
            .ok_or_else(|| {
                format!("declared Int result is not an exactly-representable integer: got {raw}")
            }),
        ValueKind::Scalar(monomial) => {
            let rendered = format_number(raw);
            let dim = render_scalar_result_dimension(monomial);
            Ok(match dim {
                Some(dim) => format!("{rendered} [{dim}, SI base units]"),
                None => rendered,
            })
        }
        // Unreachable until the ABI v2 buffer protocol lands (issue #25
        // Phase D): v1 manifests cannot declare array results.
        ValueKind::Indexed { .. } => {
            Err("array results are not supported by `--call` yet".to_string())
        }
    }
}

/// Describe a scalar result's dimension when it is concrete; dim-variable
/// results depend on the call site, so no fixed description exists.
fn render_scalar_result_dimension(
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

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "round-trip through i128 proves exactness"
)]
fn int_to_abi(value: i64) -> Option<f64> {
    let raw = value as f64;
    (raw as i128 == i128::from(value)).then_some(raw)
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "round-trip comparison proves exactness"
)]
fn int_from_abi(raw: f64) -> Option<i64> {
    i64::try_from(raw as i128)
        .ok()
        .filter(|value| (*value as f64).to_bits() == raw.to_bits())
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
                    kind: ValueKind::Scalar(DimMonomial::var(var())),
                },
                FunctionParam {
                    name: FnParamName::expect_valid("b"),
                    kind: ValueKind::Scalar(DimMonomial::var(var())),
                },
                FunctionParam {
                    name: FnParamName::expect_valid("t"),
                    kind: ValueKind::dimensionless(),
                },
            ],
            ValueKind::Scalar(DimMonomial::var(var())),
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

    #[test]
    fn declarations_render_in_gcl_syntax() {
        assert_eq!(
            render_declaration("lerp", &lerp_signature()),
            "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;"
        );
        assert_eq!(
            render_declaration("step", &step_signature()),
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
                kind: ValueKind::Scalar(DimMonomial::fixed(pressure)),
            }],
            ValueKind::Scalar(DimMonomial::fixed(sqrt_len)),
        )
        .expect("valid signature");
        assert_eq!(
            render_declaration("weird", &signature),
            "fn weird(p: Length^-1 * Mass * Time^-2) -> Length^(1/2);"
        );
    }

    #[test]
    fn aliases_are_derived_from_file_names() {
        assert_eq!(
            suggest_alias(Path::new("plugins/fluid-props.wasm")),
            "fluid_props"
        );
        assert_eq!(suggest_alias(Path::new("x/3d.wasm")), "plugin_3d");
    }

    #[test]
    fn call_args_parse_per_kind() {
        let args = parse_call_args(
            "lerp",
            &lerp_signature(),
            &["1.0".into(), "3.0".into(), "0.5".into()],
        )
        .unwrap();
        assert_eq!(args, [1.0, 3.0, 0.5]);

        let args =
            parse_call_args("step", &step_signature(), &["5".into(), "true".into()]).unwrap();
        assert_eq!(args, [5.0, 1.0]);

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
    }

    #[test]
    fn results_render_per_kind() {
        assert_eq!(render_result(&step_signature(), 42.0).unwrap(), "42");
        assert!(render_result(&step_signature(), 42.5).is_err());

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
        assert_eq!(render_result(&bool_result, 1.0).unwrap(), "true");
        assert_eq!(render_result(&bool_result, 0.0).unwrap(), "false");
        assert!(render_result(&bool_result, 0.5).is_err());

        let velocity = prelude_base_dimension("Length")
            .unwrap()
            .checked_mul(&prelude_base_dimension("Time").unwrap().pow(-1).unwrap())
            .unwrap();
        let scalar_result = FunctionSignature::try_new(
            Vec::new(),
            Vec::new(),
            vec![FunctionParam {
                name: FnParamName::expect_valid("x"),
                kind: ValueKind::dimensionless(),
            }],
            ValueKind::Scalar(DimMonomial::fixed(velocity)),
        )
        .expect("valid signature");
        assert_eq!(
            render_result(&scalar_result, 2.5).unwrap(),
            "2.5 [Length * Time^-1, SI base units]"
        );
    }
}

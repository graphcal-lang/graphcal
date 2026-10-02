//! Plugin diagnostics: extern declarations, host functions, and plugin loading.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use std::sync::Arc;

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::function_signature::SignatureSpelling;
use crate::plugin_identity::PluginDigest;
use crate::syntax::function_name::FnParamName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;
use crate::syntax::type_name::{FieldName, StructTypeName};

/// Why an extern function signature is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternSignatureError {
    ConflictingSignature {
        function: crate::syntax::function_name::FnName,
        plugin: crate::plugin_identity::PluginIdentity,
    },
    ConflictingResultType {
        function: crate::syntax::function_name::FnName,
        plugin: crate::plugin_identity::PluginIdentity,
    },
    DuplicateGenericBinder(NameAtom),
    DomainConstraint,
    UnsupportedParameterType,
    GenericStructReturn,
    UndeclaredRecordType(StructTypeName),
    GenericRecordType(StructTypeName),
    NotARecordType(StructTypeName),
    UnsupportedStructField(FieldName),
    ArrayAxesMustBeBinders,
    ArrayElementKind,
    Signature(crate::function_signature::SignatureError),
}

impl std::fmt::Display for ExternSignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingSignature { function, plugin } => write!(
                f,
                "function `{function}` of plugin \"{plugin}\" is declared elsewhere with a different signature"
            ),
            Self::ConflictingResultType { function, plugin } => write!(
                f,
                "function `{function}` of plugin \"{plugin}\" is declared elsewhere with a different result type"
            ),
            Self::DuplicateGenericBinder(atom) => {
                write!(f, "generic binder `{atom}` is declared more than once")
            }
            Self::DomainConstraint => {
                f.write_str("domain constraints are not allowed in extern function signatures")
            }
            Self::UnsupportedParameterType => f.write_str(
                "extern function signatures support Bool, Int, quantity types, and indexed scalar collections over one or more declared index variables",
            ),
            Self::GenericStructReturn => f.write_str(
                "generic struct returns are not supported in this phase; use a record type with concrete field types",
            ),
            Self::UndeclaredRecordType(leaf) => write!(
                f,
                "record type `{leaf}` has no declaration; extern struct returns must use a type declared in (or imported into) the declaring file"
            ),
            Self::GenericRecordType(leaf) => write!(
                f,
                "record type `{leaf}` is generic; generic struct returns are not supported in this phase"
            ),
            Self::NotARecordType(leaf) => write!(
                f,
                "`{leaf}` is not a record type; extern struct returns need a single constructor named after the type"
            ),
            Self::UnsupportedStructField(field) => write!(
                f,
                "field `{field}` has a type that cannot cross the plugin boundary; struct-return fields support Bool, Int, and quantity types in this phase"
            ),
            Self::ArrayAxesMustBeBinders => f.write_str(
                "extern array axes must name the signature's `Index` binders (concrete indexes and `Fin(N)` axes cannot appear in the declaration)",
            ),
            Self::ArrayElementKind => {
                f.write_str("extern array elements must be Bool, Int, or quantities")
            }
            Self::Signature(error) => error.fmt(f),
        }
    }
}

/// Where an extern call is rejected because it is not a runtime position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternCallContext {
    DomainBound,
    UnitScaleExpression,
    ConstExpression,
}

impl std::fmt::Display for ExternCallContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DomainBound => "domain bound",
            Self::UnitScaleExpression => "unit scale expression",
            Self::ConstExpression => "const expression",
        })
    }
}

/// Plugin diagnostics: extern declarations, host functions, and plugin loading.
#[derive(Debug, Clone, Error)]
pub enum PluginError {
    #[error("plugin alias `{alias}` does not declare a function `{name}`")]
    UnknownExternFunction {
        alias: crate::syntax::module_name::ModuleAliasName,
        name: crate::syntax::function_name::FnName,
    },
    #[error("invalid extern function signature: {error}")]
    InvalidExternSignature { error: ExternSignatureError },
    #[error("duplicate parameter `{name}` in extern function signature")]
    DuplicateExternParameter { name: FnParamName, first: Span },
    #[error("extern function `{name}` (plugin \"{plugin}\") is not provided by the host")]
    MissingHostFunction {
        plugin: crate::plugin_identity::PluginIdentity,
        name: crate::syntax::function_name::FnName,
    },
    #[error("extern function call `{name}` not allowed in {context}")]
    ExternCallNotAllowed {
        name: crate::hir::expr::ExternFnRef,
        context: ExternCallContext,
    },
    #[error(
        "extern function `{name}` is declared with signature {declared}, but plugin \"{plugin}\" provides {provided}"
    )]
    ExternSignatureMismatch {
        plugin: crate::plugin_identity::PluginIdentity,
        name: crate::syntax::function_name::FnName,
        declared: SignatureSpelling,
        provided: SignatureSpelling,
    },
    #[error("failed to load plugin \"{plugin}\": {reason}")]
    PluginLoadFailed {
        plugin: crate::plugin_identity::PluginIdentity,
        reason: PluginLoadFailure,
    },
    #[error("plugin \"{plugin}\" imports `{import_module}::{import_name}`, which is not allowed")]
    PluginForbiddenImport {
        plugin: crate::plugin_identity::PluginIdentity,
        import_module: String,
        import_name: String,
    },
    #[error("plugin \"{plugin}\" is not pinned in graphcal.lock")]
    PluginNotPinned {
        plugin: crate::plugin_identity::PluginIdentity,
    },
    #[error(
        "plugin \"{plugin}\" does not match its graphcal.lock pin: file hashes to {actual}, lockfile pins {expected}"
    )]
    PluginHashMismatch {
        plugin: crate::plugin_identity::PluginIdentity,
        expected: PluginDigest,
        actual: PluginDigest,
    },
}

/// Why a plugin could not be loaded.
#[derive(Debug, Clone, Error)]
pub enum PluginLoadFailure {
    /// The project loader could not provide the plugin artifact; the loader's
    /// typed failure is kept as reported.
    #[error("{0}")]
    Artifact(Arc<dyn std::error::Error + Send + Sync>),
    /// The plugin host rejected the module; `reason` is the host engine's
    /// own description.
    #[error("{reason}")]
    Module { reason: String },
}

impl DiagnosticKind for PluginError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownExternFunction { .. } => "graphcal::P002",
            Self::InvalidExternSignature { .. } => "graphcal::P001",
            Self::DuplicateExternParameter { .. } => "graphcal::P011",
            Self::MissingHostFunction { .. } => "graphcal::P003",
            Self::ExternCallNotAllowed { .. } => "graphcal::P004",
            Self::ExternSignatureMismatch { .. } => "graphcal::P005",
            Self::PluginLoadFailed { .. } => "graphcal::P006",
            Self::PluginForbiddenImport { .. } => "graphcal::P007",
            Self::PluginNotPinned { .. } => "graphcal::P009",
            Self::PluginHashMismatch { .. } => "graphcal::P010",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::UnknownExternFunction { .. } => Some("unknown extern function".to_owned()),
            Self::InvalidExternSignature { .. } => Some("invalid signature".to_owned()),
            Self::DuplicateExternParameter { .. } => Some("duplicate parameter".to_owned()),
            Self::MissingHostFunction { .. } => Some("missing host function".to_owned()),
            Self::ExternCallNotAllowed { .. } => Some("extern call not allowed here".to_owned()),
            Self::ExternSignatureMismatch { .. } => {
                Some("signature does not match the plugin manifest".to_owned())
            }
            Self::PluginLoadFailed { .. } => Some("plugin failed to load".to_owned()),
            Self::PluginForbiddenImport { .. } => {
                Some("plugin declares a forbidden import".to_owned())
            }
            Self::PluginNotPinned { .. } => Some("plugin has no graphcal.lock pin".to_owned()),
            Self::PluginHashMismatch { .. } => {
                Some("plugin file does not match its pin".to_owned())
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::UnknownExternFunction { .. } => Some("extern functions must be declared in the plugin's `import plugin ... { ... }` block".to_string()),
            Self::InvalidExternSignature { .. } => Some("extern signatures support Bool, Int, quantity types, indexed collections of those scalar kinds over one or more declared index variables, and record struct returns with concrete fields; each dimension variable must be declared in the `<...>` binder list and bound by a bare quantity parameter or bare quantity-array element before compound uses, and every result axis must reuse an index variable that indexes some parameter".to_owned()),
            Self::DuplicateExternParameter { .. } => Some("each extern function parameter name must be unique".to_owned()),
            Self::MissingHostFunction { .. } => Some("the embedder's host function registry has no entry for this declared extern function".to_owned()),
            Self::ExternCallNotAllowed { .. } => Some("extern functions are runtime-provided and can only be called from runtime expressions (nodes, param defaults, and asserts)".to_owned()),
            Self::ExternSignatureMismatch { .. } => Some("the extern declaration must structurally match the signature in the plugin's manifest (dimension-variable and parameter names may differ; the dimensional shape may not); note that plugin manifests cannot express user-defined base dimensions".to_owned()),
            Self::PluginLoadFailed { .. } => Some("plugin paths ending in `.wasm` resolve relative to the declaring package root and must name a vendored WebAssembly module with an embedded graphcal manifest".to_owned()),
            Self::PluginForbiddenImport { .. } => Some("graphcal plugins must be pure: they may import nothing except `graphcal::fail`. A module importing WASI or other host APIs is not a graphcal plugin (rebuild for a bare wasm32 target or stub the imports out)".to_owned()),
            Self::PluginNotPinned { .. } => Some("projects with a graphcal.toml load plugin binaries only through lockfile pins; run `graphcal deps lock` to record this plugin's hash".to_owned()),
            Self::PluginHashMismatch { .. } => Some("the lockfile is the trust boundary for plugin code — a changed binary must arrive together with a reviewed pin update; if this change is intentional, rerun `graphcal deps lock`".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::UnknownExternFunction { .. }
            | Self::InvalidExternSignature { .. }
            | Self::MissingHostFunction { .. }
            | Self::ExternCallNotAllowed { .. }
            | Self::ExternSignatureMismatch { .. }
            | Self::PluginLoadFailed { .. }
            | Self::PluginForbiddenImport { .. }
            | Self::PluginNotPinned { .. }
            | Self::PluginHashMismatch { .. } => Vec::new(),
            Self::DuplicateExternParameter { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first declared here".to_owned(),
            }],
        }
    }
}

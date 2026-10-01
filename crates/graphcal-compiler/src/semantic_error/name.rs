//! Name-resolution diagnostics: duplicate, unknown, and misused names.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::resolve::category::DeclSymbolKind;
use crate::semantic::time_scale::TimeScale;
use crate::syntax::function_name::FnName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::{ConstructorName, FieldName, StructTypeName};

/// The function an arity diagnostic names: a closed built-in or a plugin
/// function spelled by its declared name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalledFunction {
    Builtin(crate::builtin::BuiltinFn),
    Extern(FnName),
}

impl std::fmt::Display for CalledFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin(function) => function.fmt(f),
            Self::Extern(name) => name.fmt(f),
        }
    }
}

/// Name-resolution diagnostics: duplicate, unknown, and misused names.
#[derive(Debug, Clone, Error)]
pub enum NameError {
    #[error("duplicate name `{name}`")]
    DuplicateName { name: String, first: Span },
    /// A constructor payload repeats a field declaration.
    #[error("constructor `{constructor}` declares field `{field}` more than once")]
    DuplicateConstructorField {
        type_name: StructTypeName,
        constructor: ConstructorName,
        field: FieldName,
        first: Span,
    },
    #[error("{kind} `{name}` shadows a built-in name")]
    BuiltinNameShadowed { kind: &'static str, name: String },
    #[error("property `{property}` is not valid in {context}")]
    InvalidPlotProperty {
        property: String,
        context: &'static str,
        /// Preformatted help listing the valid property set for `context`.
        valid: String,
    },
    #[error("{owner_kind} `{owner}` references unknown plot `{name}`")]
    UnknownPlotReference {
        owner_kind: &'static str,
        owner: crate::syntax::decl_name::DeclName,
        name: ScopedName,
    },
    #[error("`{name}` is a {actual_kind}, not a plot")]
    CompositionReferencesNonPlot {
        owner_kind: &'static str,
        actual_kind: &'static str,
        name: ScopedName,
    },
    #[error("{owner_kind} `{owner}` lists plot `{name}` more than once")]
    DuplicatePlotReference {
        owner_kind: &'static str,
        owner: crate::syntax::decl_name::DeclName,
        name: ScopedName,
    },
    #[error("unknown graph reference `@{name}`")]
    UnknownGraphRef { name: ScopedName },
    #[error("bare reference `{name}` names a {kind}; user graph declarations require `@`")]
    BareGraphDeclarationRef {
        name: ScopedName,
        kind: DeclSymbolKind,
    },
    #[error("time scale `{scale}` cannot be used as a value")]
    TimeScaleInValuePosition { scale: TimeScale },
    #[error("unknown function `{name}`")]
    UnknownFunction { name: String },
    #[error("function `{name}` uses positional arguments")]
    NamedArgumentsOnFunction {
        name: String,
        positional_call: String,
    },
    #[error("graph reference `@{name}` not allowed in const expression")]
    GraphRefInConst { name: ScopedName },
    #[error("function `{name}` expects {expected} argument(s), got {got}")]
    WrongArity {
        name: CalledFunction,
        expected: usize,
        got: usize,
    },
}

impl DiagnosticKind for NameError {
    fn code(&self) -> &'static str {
        match self {
            Self::DuplicateName { .. } => "graphcal::N001",
            Self::DuplicateConstructorField { .. } => "graphcal::N016",
            Self::BuiltinNameShadowed { .. } => "graphcal::N009",
            Self::InvalidPlotProperty { .. } => "graphcal::N011",
            Self::UnknownPlotReference { .. } => "graphcal::N012",
            Self::CompositionReferencesNonPlot { .. } => "graphcal::N013",
            Self::DuplicatePlotReference { .. } => "graphcal::N014",
            Self::UnknownGraphRef { .. } => "graphcal::N002",
            Self::BareGraphDeclarationRef { .. } => "graphcal::N017",
            Self::TimeScaleInValuePosition { .. } => "graphcal::N018",
            Self::UnknownFunction { .. } => "graphcal::N004",
            Self::NamedArgumentsOnFunction { .. } => "graphcal::N015",
            Self::GraphRefInConst { .. } => "graphcal::N005",
            Self::WrongArity { .. } => "graphcal::N006",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::DuplicateName { .. } => Some("duplicate definition here".to_owned()),
            Self::DuplicateConstructorField {
                constructor,
                field,
                type_name,
                ..
            } => Some(format!(
                "duplicate `{field}` field in `{type_name}.{constructor}`"
            )),
            Self::BuiltinNameShadowed { .. } => Some("shadows a built-in name".to_owned()),
            Self::InvalidPlotProperty { context, .. } => Some(format!("not a {context} property")),
            Self::UnknownPlotReference { .. } => Some("no plot with this name".to_owned()),
            Self::CompositionReferencesNonPlot { actual_kind, .. } => {
                Some(format!("this names a {actual_kind}"))
            }
            Self::DuplicatePlotReference { .. } => Some("duplicate entry".to_owned()),
            Self::UnknownGraphRef { .. } => Some("not found".to_owned()),
            Self::BareGraphDeclarationRef { .. } => Some("missing `@` sigil".to_owned()),
            Self::TimeScaleInValuePosition { scale, .. } => {
                Some(format!("no Term named `{scale}` is in scope"))
            }
            Self::UnknownFunction { .. } => Some("unknown function".to_owned()),
            Self::NamedArgumentsOnFunction { .. } => {
                Some("named arguments are not allowed in function calls".to_owned())
            }
            Self::GraphRefInConst { .. } => Some("@ reference not allowed here".to_owned()),
            Self::WrongArity { .. } => Some("wrong number of arguments".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::DuplicateName { .. } => Some("each name must be unique within a file".to_owned()),
            Self::DuplicateConstructorField { .. } => Some("constructor field names must be unique; remove or rename one declaration".to_owned()),
            Self::BuiltinNameShadowed { .. } => Some("choose a different name; prelude dimensions, built-in types, prelude units, and built-in numeric constants cannot be redefined in their namespaces".to_owned()),
            Self::InvalidPlotProperty { valid, .. } => Some(valid.clone()),
            Self::UnknownPlotReference { .. } => Some("`plots:` entries must name `plot` declarations visible in this file".to_owned()),
            Self::CompositionReferencesNonPlot { actual_kind, owner_kind, .. } => Some(format!("{owner_kind}s compose `plot` declarations; they cannot nest other {actual_kind}s")),
            Self::DuplicatePlotReference { .. } => Some("each plot may appear at most once in a `plots:` list".to_owned()),
            Self::UnknownGraphRef { .. } => Some("graph references must point to a `param` or `node`".to_owned()),
            Self::BareGraphDeclarationRef { kind, name, .. } => Some(format!("write `@{name}` to reference this {kind}")),
            Self::TimeScaleInValuePosition { scale, .. } => Some(format!("time scales are Static atoms; use `{scale}` in `Datetime<{scale}>` or `epoch<{scale}>(...)`")),
            Self::UnknownFunction { .. } => Some("check function name and ensure it is defined".to_owned()),
            Self::NamedArgumentsOnFunction { positional_call, .. } => Some(format!("write `{positional_call}`")),
            Self::GraphRefInConst { .. } => Some("const expressions are evaluated at compile time and cannot reference params or nodes".to_owned()),
            Self::WrongArity { .. } => None,
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DuplicateName { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first defined here".to_owned(),
            }],
            Self::DuplicateConstructorField { field, first, .. } => vec![SecondaryLabel {
                span: *first,
                text: format!("first `{field}` field declared here"),
            }],
            Self::BuiltinNameShadowed { .. }
            | Self::InvalidPlotProperty { .. }
            | Self::UnknownPlotReference { .. }
            | Self::CompositionReferencesNonPlot { .. }
            | Self::DuplicatePlotReference { .. }
            | Self::UnknownGraphRef { .. }
            | Self::BareGraphDeclarationRef { .. }
            | Self::TimeScaleInValuePosition { .. }
            | Self::UnknownFunction { .. }
            | Self::NamedArgumentsOnFunction { .. }
            | Self::GraphRefInConst { .. }
            | Self::WrongArity { .. } => Vec::new(),
        }
    }
}

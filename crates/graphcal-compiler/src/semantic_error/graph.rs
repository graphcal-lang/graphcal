//! Diagnostics of the declaration graph and of DAG calls.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::dag_id::DagId;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::semantic::checked_type::TypeSpelling;
use crate::syntax::decl_name::DeclName;

/// The member a dependency cycle is reported at: a DAG that inline-calls
/// itself, or a declaration that depends on itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleMember {
    Dag(DagId),
    Declaration(DeclName),
}

impl std::fmt::Display for CycleMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dag(dag) => dag.fmt(f),
            Self::Declaration(name) => name.fmt(f),
        }
    }
}

/// Diagnostics of the declaration graph and of DAG calls.
#[derive(Debug, Clone, Error)]
pub enum GraphError {
    #[error("DAG call `{name}` is not allowed in a compile-time expression")]
    DagCallInCompileTime { name: DagId },
    #[error("cyclic dependency involving `{name}`")]
    CyclicDependency { name: CycleMember },
    #[error("unknown dag `{name}`")]
    UnknownDag { name: DagId },
    #[error("unknown param `{name}` in DAG call to `{dag_name}`")]
    UnknownDagParam { name: DeclName, dag_name: DagId },
    #[error(
        "missing required binding(s) {} when instantiating DAG `{dag_name}`",
        format_missing_bindings(missing)
    )]
    MissingDagBindings {
        /// Sorted by spelling.
        missing: Vec<DeclName>,
        // TODO(S15 leftover): include sites name the DAG by its written
        // path or inline declaration; type them once the include validators
        // carry those identities instead of `&str`.
        dag_name: String,
    },
    #[error("unknown output `{name}` in DAG call to `{dag_name}`")]
    UnknownDagOutput { name: DeclName, dag_name: DagId },
    #[error("DAG call binding `{param_name}`: expected {expected}, found {found}")]
    DagArgTypeMismatch {
        param_name: DeclName,
        expected: String,
        found: TypeSpelling,
    },
    #[error("inline DAG target not found in project: {target}")]
    InlineDagTargetNotFound { target: DagId },
    /// Templates that include each other in a circle, from the template the
    /// include expansion re-entered back to it.
    #[error("recursive DAG instantiation: {}", format_template_cycle(templates))]
    RecursiveDagInstantiation { templates: Vec<DagId> },
}

impl DiagnosticKind for GraphError {
    fn code(&self) -> &'static str {
        match self {
            Self::DagCallInCompileTime { .. } => "graphcal::G007",
            Self::CyclicDependency { .. } => "graphcal::G001",
            Self::UnknownDag { .. } => "graphcal::G002",
            Self::UnknownDagParam { .. } => "graphcal::G003",
            Self::MissingDagBindings { .. } => "graphcal::G004",
            Self::UnknownDagOutput { .. } => "graphcal::G005",
            Self::DagArgTypeMismatch { .. } => "graphcal::G006",
            Self::InlineDagTargetNotFound { .. } => "graphcal::G008",
            Self::RecursiveDagInstantiation { .. } => "graphcal::G009",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::DagCallInCompileTime { .. } => {
                Some("runtime DAG instantiation is not allowed here".to_owned())
            }
            Self::CyclicDependency { .. } => Some("involved in cycle".to_owned()),
            Self::UnknownDag { .. } => Some("unknown dag".to_owned()),
            Self::UnknownDagParam { dag_name, .. } => Some(format!("not a param in `{dag_name}`")),
            Self::MissingDagBindings { .. } => Some("missing binding(s)".to_owned()),
            Self::UnknownDagOutput { dag_name, .. } => {
                Some(format!("not a projectable value in `{dag_name}`"))
            }
            Self::DagArgTypeMismatch { .. } => Some("type mismatch".to_owned()),
            Self::InlineDagTargetNotFound { .. } | Self::RecursiveDagInstantiation { .. } => {
                Some("error here".to_owned())
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::DagCallInCompileTime { .. } => Some("a DAG call is an anonymous runtime include; call it from a `node`, `param` default, assertion, or visualization expression instead".to_owned()),
            Self::CyclicDependency { .. } => Some("declarations cannot form dependency cycles".to_owned()),
            Self::UnknownDag { .. } => Some("the inline call references a dag that is not declared in this file".to_owned()),
            Self::UnknownDagParam { .. } => Some("the binding name must match a `param` declared in the called DAG".to_owned()),
            Self::MissingDagBindings { .. } => Some("every required `param` declared in the DAG must be bound at each `include` or call site".to_owned()),
            Self::UnknownDagOutput { .. } => Some("the projection after `).` must name a param input port or an explicitly exported node in the called DAG".to_owned()),
            Self::DagArgTypeMismatch { .. } => Some("the binding expression must have the same type as the DAG's param declaration".to_owned()),
            Self::InlineDagTargetNotFound { .. }
            | Self::RecursiveDagInstantiation { .. }=> None,
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DagCallInCompileTime { .. }
            | Self::CyclicDependency { .. }
            | Self::UnknownDag { .. }
            | Self::UnknownDagParam { .. }
            | Self::MissingDagBindings { .. }
            | Self::UnknownDagOutput { .. }
            | Self::DagArgTypeMismatch { .. }
            | Self::InlineDagTargetNotFound { .. }
            | Self::RecursiveDagInstantiation { .. } => Vec::new(),
        }
    }
}

/// Render missing bindings as the quoted, bracketed list diagnostics have
/// always shown (`["a", "b"]`).
fn format_missing_bindings(missing: &[DeclName]) -> String {
    format!(
        "{:?}",
        missing.iter().map(ToString::to_string).collect::<Vec<_>>()
    )
}

/// Each template is named by its path inside its file (`outer.inner`), or by
/// its module identity when it is a file root.
fn format_template_cycle(templates: &[DagId]) -> String {
    templates
        .iter()
        .map(|template| {
            let file_depth = template.file_root().segments().len();
            let inline_path = template
                .segments()
                .iter()
                .skip(file_depth)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if inline_path.is_empty() {
                template.to_string()
            } else {
                inline_path.join(".")
            }
        })
        .collect::<Vec<_>>()
        .join(" -> ")
}

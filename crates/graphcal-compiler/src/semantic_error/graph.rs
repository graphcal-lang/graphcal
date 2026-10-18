//! Diagnostics of the declaration graph and of DAG calls.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::dag_id::DagId;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::semantic::checked_type::TypeSpelling;
use crate::syntax::ast::ModulePath;
use crate::syntax::decl_name::DeclName;
use crate::tir::typed::declared_type_spelling::DeclaredTypeSpelling;

/// How a diagnostic names the DAG an import, include, or DAG call refers
/// to: the module path as the source wrote it, a resolved DAG identity, or
/// an inline `dag` declaration by its declared name.
#[derive(Debug, Clone)]
pub enum DagReference {
    Path(ModulePath),
    Dag(DagId),
    InlineDag(DeclName),
}

impl std::fmt::Display for DagReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(path) => f.write_str(&path.display_path()),
            Self::Dag(dag) => dag.fmt(f),
            Self::InlineDag(name) => name.fmt(f),
        }
    }
}

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

/// Why a dependency cycle with an `unfold` member is not an accepted
/// recurrence group: the first violated rule of
/// `internals/recurrence-groups.md` (§3), in rule order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecurrenceRejection {
    /// R1: the cycle reads `other`, which another DAG declares.
    MixedOwners { other: DeclName },
    /// R2: the member is an unfinished `todo` node.
    Unfinished,
    /// R3: the member is not a top-level `for` or `unfold`.
    NotRecurrenceShape,
    /// R4: the member steps along another axis than `first`.
    AxisMismatch { first: DeclName },
    /// R5: the member reads itself.
    SelfRead,
    /// R6: the member reads all of `target`.
    WholeValueRead { target: DeclName },
    /// R6: the member reads `target` without an `@` reference, through a
    /// dynamic unit's scale.
    ImplicitRead { target: DeclName },
    /// R6: the member reads `target` with a leading key that is not its own
    /// step or previous-step binder.
    NotStepKeyed { target: DeclName },
    /// R7: these members read each other at the same step, in this order.
    SameStepCycle { cycle: Vec<DeclName> },
}

/// Diagnostics of the declaration graph and of DAG calls.
#[derive(Debug, Clone, Error)]
pub enum GraphError {
    #[error("DAG call `{name}` is not allowed in a compile-time expression")]
    DagCallInCompileTime { name: DagId },
    #[error("cyclic dependency involving `{name}`")]
    CyclicDependency { name: CycleMember },
    /// A dependency cycle with an `unfold` member that is not an accepted
    /// recurrence group, reported at the member or read that violates the
    /// first rule.
    #[error("cyclic dependency involving `{name}`")]
    RejectedRecurrence {
        name: DeclName,
        reason: RecurrenceRejection,
    },
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
        dag_name: DagReference,
    },
    #[error("unknown output `{name}` in DAG call to `{dag_name}`")]
    UnknownDagOutput { name: DeclName, dag_name: DagId },
    #[error("DAG call binding `{param_name}`: expected {expected}, found {found}")]
    DagArgTypeMismatch {
        param_name: DeclName,
        expected: DeclaredTypeSpelling,
        found: TypeSpelling,
    },
    #[error("inline DAG target not found in project: {target}")]
    InlineDagTargetNotFound { target: DagId },
    /// Templates that include each other in a circle, from the template the
    /// include expansion re-entered back to it.
    #[error("recursive DAG instantiation: {}", format_template_cycle(templates))]
    RecursiveDagInstantiation { templates: Vec<DagId> },
    /// A `todo<T> { … }` hole outside the body of a runtime `node`.
    #[error("`todo` holes are allowed only in `node` formulas")]
    TodoHoleOutsideNode,
}

impl DiagnosticKind for GraphError {
    fn code(&self) -> &'static str {
        match self {
            Self::DagCallInCompileTime { .. } => "graphcal::G007",
            Self::CyclicDependency { .. } | Self::RejectedRecurrence { .. } => "graphcal::G001",
            Self::UnknownDag { .. } => "graphcal::G002",
            Self::UnknownDagParam { .. } => "graphcal::G003",
            Self::MissingDagBindings { .. } => "graphcal::G004",
            Self::UnknownDagOutput { .. } => "graphcal::G005",
            Self::DagArgTypeMismatch { .. } => "graphcal::G006",
            Self::InlineDagTargetNotFound { .. } => "graphcal::G008",
            Self::RecursiveDagInstantiation { .. } => "graphcal::G009",
            Self::TodoHoleOutsideNode => "graphcal::G010",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::DagCallInCompileTime { .. } => {
                Some("runtime DAG instantiation is not allowed here".to_owned())
            }
            Self::CyclicDependency { .. } => Some("involved in cycle".to_owned()),
            Self::RejectedRecurrence { reason, .. } => Some(reason.label()),
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
            Self::TodoHoleOutsideNode => Some("unfinished formula not allowed here".to_owned()),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::DagCallInCompileTime { .. } => Some("a DAG call is an anonymous runtime include; call it from a `node`, `param` default, assertion, or visualization expression instead".to_owned()),
            Self::CyclicDependency { .. } => Some("declarations cannot form dependency cycles".to_owned()),
            Self::RejectedRecurrence { reason, .. } => Some(reason.help()),
            Self::UnknownDag { .. } => Some("the inline call references a dag that is not declared in this file".to_owned()),
            Self::UnknownDagParam { .. } => Some("the binding name must match a `param` declared in the called DAG".to_owned()),
            Self::MissingDagBindings { .. } => Some("every required `param` declared in the DAG must be bound at each `include` or call site".to_owned()),
            Self::UnknownDagOutput { .. } => Some("the projection after `)::` must name a param input port, an explicitly exported node, or a public include-output alias in the called DAG".to_owned()),
            Self::DagArgTypeMismatch { .. } => Some("the binding expression must have the same type as the DAG's param declaration".to_owned()),
            Self::TodoHoleOutsideNode => Some("only a `node` can be unfinished; write the formula here".to_owned()),
            Self::InlineDagTargetNotFound { .. }
            | Self::RecursiveDagInstantiation { .. }=> None,
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::DagCallInCompileTime { .. }
            | Self::CyclicDependency { .. }
            | Self::RejectedRecurrence { .. }
            | Self::UnknownDag { .. }
            | Self::UnknownDagParam { .. }
            | Self::MissingDagBindings { .. }
            | Self::UnknownDagOutput { .. }
            | Self::DagArgTypeMismatch { .. }
            | Self::InlineDagTargetNotFound { .. }
            | Self::RecursiveDagInstantiation { .. }
            | Self::TodoHoleOutsideNode => Vec::new(),
        }
    }
}

impl RecurrenceRejection {
    fn label(&self) -> String {
        match self {
            Self::MixedOwners { other } => {
                format!("this cycle reads `{other}`, declared in another DAG")
            }
            Self::Unfinished => "unfinished node in a cycle".to_owned(),
            Self::NotRecurrenceShape => "not a top-level `for` or `unfold`".to_owned(),
            Self::AxisMismatch { first } => {
                format!("steps along another axis than `{first}`")
            }
            Self::SelfRead => "reads itself".to_owned(),
            Self::WholeValueRead { target } => format!("reads all of `{target}`"),
            Self::ImplicitRead { target } => {
                format!("reads `{target}` through a dynamic unit")
            }
            Self::NotStepKeyed { target } => {
                format!("reads `{target}` without this node's step key")
            }
            Self::SameStepCycle { cycle } => format!(
                "same-step cycle {}",
                cycle
                    .iter()
                    .chain(cycle.first())
                    .map(|member| format!("`{member}`"))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ),
        }
    }

    fn help(&self) -> String {
        match self {
            Self::MixedOwners { .. } => {
                "a recurrence cannot span declarations of different DAGs".to_owned()
            }
            Self::Unfinished => {
                "a recurrence needs formulas to check the step offsets of its reads".to_owned()
            }
            Self::NotRecurrenceShape => "every declaration in a cycle with an `unfold` must be a top-level `for` or `unfold` over the recurrence axis".to_owned(),
            Self::AxisMismatch { .. } => {
                "every declaration in a recurrence steps along the same axis".to_owned()
            }
            Self::SelfRead => {
                "use the `unfold` state binding to read the previous value".to_owned()
            }
            Self::WholeValueRead { .. } | Self::ImplicitRead { .. } => "a declaration in a recurrence can read another one only element by element, as `@y[t]` or `@y[prev_t]`".to_owned(),
            Self::NotStepKeyed { .. } => "index a declaration of the recurrence with this node's own step binder `t` or previous-step binder `prev_t`".to_owned(),
            Self::SameStepCycle { .. } => {
                "one read on the cycle must use the previous step `prev_t`".to_owned()
            }
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

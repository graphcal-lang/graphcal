//! Module diagnostics: imports, includes, module bindings, and DAG inputs.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::dag_id::DagId;
use crate::declaration_kind::DeclarationKind;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::resolve::error::ModuleResolveError;
use crate::resolved_name::ResolvedStructTypeName;
use crate::semantic::index_def::{IndexBindingCategory, IndexBindingTarget, IndexCategory};
use crate::semantic_error::graph::DagReference;
use crate::syntax::dimension::UnitRef;
use crate::syntax::import_category::{ImportItemCategoryMismatch, ImportItemNamespace};
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::ModuleAliasName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;
use crate::syntax::type_name::ConstructorName;

/// Module diagnostics: imports, includes, module bindings, and DAG inputs.
#[derive(Debug, Clone, Error)]
pub enum ModuleError {
    #[error("cannot `import` plot `{name}`")]
    ImportPlotItem { name: NameAtom },
    #[error("cannot `import` assertion `{name}` from a module blueprint")]
    ImportAssertionItem { name: NameAtom },
    #[error("cannot `import` runtime unit `{name}`")]
    ImportRuntimeUnit { name: UnitRef },
    #[error("cannot `import` required {kind} input `{name}`")]
    ImportRequiredStaticInput {
        kind: crate::static_interface::StaticInputKind,
        name: NameAtom,
    },
    #[error(
        "cannot `import` `{name}` because it depends on required {dependency_kind} input `{dependency}`"
    )]
    ImportUnresolvedStaticDependency {
        name: NameAtom,
        dependency_kind: crate::static_interface::StaticInputKind,
        dependency: NameAtom,
    },
    #[error("cannot bind {kind} input `{name}` to non-concrete {kind} `{target}`")]
    InvalidStaticBindingTarget {
        kind: crate::static_interface::StaticInputKind,
        name: NameAtom,
        target: NameAtom,
    },
    #[error("cannot project `{name}` from a configured DAG instance")]
    IncludeItemNotProjectable { name: NameAtom },
    #[error(
        "cannot project constructor `{constructor}` because its owning type `{owner_type}` is rebound"
    )]
    IncludeConstructorOwnerRebound {
        constructor: ConstructorName,
        owner_type: ResolvedStructTypeName,
    },
    #[error("selective include chooses {namespace} producer `{name}` more than once")]
    DuplicateIncludeSelection {
        namespace: ImportItemNamespace,
        name: NameAtom,
        first: Span,
    },
    #[error("name `{name}` not found in imported file `{file_path}`")]
    ImportNameNotFound {
        name: NameAtom,
        file_path: DagReference,
    },
    #[error("in imported file `{file_path}`, {mismatch}")]
    ImportCategoryMismatch {
        file_path: DagReference,
        mismatch: ImportItemCategoryMismatch,
    },
    #[error("duplicate module name `{name}`")]
    DuplicateModuleName { name: ModuleAliasName, first: Span },
    #[error("unknown module `{name}`")]
    UnknownModule { name: ModuleAliasName },
    #[error("unknown param `{name}` in import binding for `{file_path}`")]
    UnknownParamBinding {
        name: NameAtom,
        file_path: DagReference,
    },
    #[error("binding target `{name}` is a {actual_kind}, not a param")]
    BindingNotAParam {
        name: NameAtom,
        actual_kind: DeclarationKind,
    },
    #[error("`{name}` is not a {expected} input of the invoked DAG")]
    DagInputCategoryMismatch {
        name: NameAtom,
        expected: &'static str,
    },
    #[error("invalid type-level binding value for `{name}`")]
    InvalidTypeLevelBindingValue { name: NameAtom },
    #[error("index binding `{dep_index}: {value}`: `{value}` is not a known index")]
    IndexBindingNotAnIndex {
        dep_index: IndexName,
        value: IndexBindingTarget,
    },
    #[error("index kind mismatch: `{dep_index}` is {dep_kind} but `{bound_index}` is {bound_kind}")]
    IndexKindMismatch {
        dep_index: IndexName,
        dep_kind: IndexBindingCategory,
        bound_index: IndexBindingTarget,
        bound_kind: IndexCategory,
    },
    #[error("cannot import runtime item `{name}`; use `include` for runtime nodes and params")]
    ImportRuntimeItem { name: NameAtom },
    /// A module-aware lookup failed in a way no more specific diagnostic covers.
    #[error("{error}")]
    ModuleResolution { error: Box<ModuleResolveError> },
    /// Two source modules share one module-path spelling.
    #[error(
        "module path `{first}` is ambiguous: it names a module in file `{}` and a module in file `{}`",
        first.file_root(),
        second.file_root()
    )]
    AmbiguousModulePath { first: DagId, second: DagId },
}

impl DiagnosticKind for ModuleError {
    fn code(&self) -> &'static str {
        match self {
            Self::ImportPlotItem { .. } => "graphcal::M021",
            Self::ImportAssertionItem { .. } => "graphcal::M024",
            Self::ImportRuntimeUnit { .. } => "graphcal::M025",
            Self::ImportRequiredStaticInput { .. } => "graphcal::M028",
            Self::ImportUnresolvedStaticDependency { .. } => "graphcal::M029",
            Self::InvalidStaticBindingTarget { .. } => "graphcal::M030",
            Self::IncludeItemNotProjectable { .. } => "graphcal::M031",
            Self::IncludeConstructorOwnerRebound { .. } => "graphcal::M032",
            Self::DuplicateIncludeSelection { .. } => "graphcal::M027",
            Self::ImportNameNotFound { .. } => "graphcal::M003",
            Self::ImportCategoryMismatch { .. } => "graphcal::M022",
            Self::DuplicateModuleName { .. } => "graphcal::M005",
            Self::UnknownModule { .. } => "graphcal::M006",
            Self::UnknownParamBinding { .. } => "graphcal::M009",
            Self::BindingNotAParam { .. } => "graphcal::M010",
            Self::DagInputCategoryMismatch { .. } => "graphcal::M011",
            Self::InvalidTypeLevelBindingValue { .. } => "graphcal::M016",
            Self::IndexBindingNotAnIndex { .. } => "graphcal::M019",
            Self::IndexKindMismatch { .. } => "graphcal::M018",
            Self::ImportRuntimeItem { .. } => "graphcal::M020",
            Self::ModuleResolution { .. } => "graphcal::M036",
            Self::AmbiguousModulePath { .. } => "graphcal::M034",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::ImportPlotItem { .. } => Some("plots cannot travel through `import`".to_owned()),
            Self::ImportAssertionItem { .. } => {
                Some("an imported module has no assertion outcome".to_owned())
            }
            Self::ImportRuntimeUnit { .. } => {
                Some("a plain `unit` belongs to a runtime instance".to_owned())
            }
            Self::ImportRequiredStaticInput { .. } => {
                Some("required Static input has no blueprint-stable target".to_owned())
            }
            Self::ImportUnresolvedStaticDependency { .. } => {
                Some("declaration is not blueprint-closed".to_owned())
            }
            Self::InvalidStaticBindingTarget { .. } => {
                Some("required Static inputs are holes, not concrete targets".to_owned())
            }
            Self::IncludeItemNotProjectable { .. } => {
                Some("this declaration is not an instance projection".to_owned())
            }
            Self::IncludeConstructorOwnerRebound { .. } => {
                Some("source constructor no longer belongs to the projected type".to_owned())
            }
            Self::DuplicateIncludeSelection { .. } => {
                Some("duplicate producer selection".to_owned())
            }
            Self::ImportNameNotFound { .. } => Some("not found in imported file".to_owned()),
            Self::ImportCategoryMismatch { .. } => Some("wrong import category".to_owned()),
            Self::DuplicateModuleName { .. } => Some("duplicate module import".to_owned()),
            Self::UnknownModule { .. } => Some("unknown module".to_owned()),
            Self::UnknownParamBinding { .. } => Some("not a param in the imported file".to_owned()),
            Self::BindingNotAParam { actual_kind, .. } => {
                Some(format!("targets a {actual_kind}, not a param"))
            }
            Self::DagInputCategoryMismatch { expected, .. } => {
                Some(format!("no {expected} input with this name"))
            }
            Self::InvalidTypeLevelBindingValue { .. } => {
                Some("expected a compatible type-level binding argument".to_owned())
            }
            Self::IndexBindingNotAnIndex { .. } => Some("not a known index".to_owned()),
            Self::IndexKindMismatch { .. } => Some("kind mismatch".to_owned()),
            Self::ImportRuntimeItem { .. } => Some("runtime item cannot be imported".to_owned()),
            Self::ModuleResolution { .. } | Self::AmbiguousModulePath { .. } => {
                Some("error here".to_owned())
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::ImportPlotItem { name, .. } => Some(format!("plots are runtime sinks evaluated against an instance; request them through an include brace list instead: `include path(...)::{{ {name} }}`")),
            Self::ImportAssertionItem { name, .. } => Some(format!("assertions run in concrete DAG instances; use `include path(...)::{{ {name} }}` or evaluate the library as an entry DAG")),
            Self::ImportRuntimeUnit { .. } => Some("use the unit through a concrete `include` or direct DAG call instance; otherwise declare a `const unit` to grant blueprint import capability".to_owned()),
            Self::ImportRequiredStaticInput { .. } => Some("supply this typed input through `include` or a direct DAG call".to_owned()),
            Self::ImportUnresolvedStaticDependency { .. } => Some("supply the required Static input through `include` before projecting this declaration".to_owned()),
            Self::InvalidStaticBindingTarget { .. } => Some("bind to a fixed declaration or an optional `pub(bind)` declaration with a default".to_owned()),
            Self::IncludeItemNotProjectable { .. } => Some("DAGs are reusable blueprints, not instance members; address the child with a dotted blueprint path such as `module.child`".to_owned()),
            Self::IncludeConstructorOwnerRebound { .. } => Some("project constructors only when their source nominal type keeps its canonical identity; use constructors of the replacement type instead".to_owned()),
            Self::DuplicateIncludeSelection { .. } => Some("select each producer at most once in an include list".to_owned()),
            Self::ImportNameNotFound { .. } => Some("check that the name is declared in the imported file".to_owned()),
            Self::ImportCategoryMismatch { .. }
            | Self::DuplicateModuleName { .. }
            | Self::ModuleResolution { .. }
            | Self::AmbiguousModulePath { .. } => None,
            Self::UnknownModule { .. } => Some("module-qualified references start with a local name introduced by `import`; call an aliased module through that alias, for example `import pkg.module as m; @m(...)::out`".to_owned()),
            Self::UnknownParamBinding { .. } => Some("param bindings must reference `param` declarations in the imported file".to_owned()),
            Self::BindingNotAParam { .. } => Some("only `param` declarations can be overridden in import bindings".to_owned()),
            Self::DagInputCategoryMismatch { .. } => Some("the binding marker selects exactly one input category; correct the marker or target name".to_owned()),
            Self::InvalidTypeLevelBindingValue { name, .. } => Some(format!("index bindings accept a compatible index name or structural axis such as `index {name}: Fin(3)`; type and dimension bindings use the explicit `type` and `dim` markers")),
            Self::IndexBindingNotAnIndex { .. } => Some("the right-hand side must name a compatible index visible in the including DAG or use a structural Fin(N) axis".to_owned()),
            Self::IndexKindMismatch { .. } => Some("unconstrained required indexes accept named or Fin(N) axes; concrete named indexes remain named-only, and coordinate indexes remain coordinate-only".to_owned()),
            Self::ImportRuntimeItem { .. } => Some("`import` only allows compile-time items (const, dimension, unit, type, index, dag); use `include` for runtime nodes and params".to_owned()),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::ImportPlotItem { .. }
            | Self::ImportAssertionItem { .. }
            | Self::ImportRuntimeUnit { .. }
            | Self::ImportRequiredStaticInput { .. }
            | Self::ImportUnresolvedStaticDependency { .. }
            | Self::InvalidStaticBindingTarget { .. }
            | Self::IncludeItemNotProjectable { .. }
            | Self::IncludeConstructorOwnerRebound { .. }
            | Self::ImportNameNotFound { .. }
            | Self::ImportCategoryMismatch { .. }
            | Self::UnknownModule { .. }
            | Self::UnknownParamBinding { .. }
            | Self::BindingNotAParam { .. }
            | Self::DagInputCategoryMismatch { .. }
            | Self::InvalidTypeLevelBindingValue { .. }
            | Self::IndexBindingNotAnIndex { .. }
            | Self::IndexKindMismatch { .. }
            | Self::ImportRuntimeItem { .. }
            | Self::ModuleResolution { .. }
            | Self::AmbiguousModulePath { .. } => Vec::new(),
            Self::DuplicateIncludeSelection { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first selected here".to_owned(),
            }],
            Self::DuplicateModuleName { first, .. } => vec![SecondaryLabel {
                span: *first,
                text: "first imported here".to_owned(),
            }],
        }
    }
}

impl ModuleError {
    /// The diagnostic of a module-resolution failure that no more specific
    /// family variant describes; an ambiguous module path keeps its own code.
    #[must_use]
    pub fn resolution(error: ModuleResolveError) -> Self {
        match error {
            ModuleResolveError::AmbiguousModulePath { first, second } => {
                Self::AmbiguousModulePath { first, second }
            }
            error => Self::ModuleResolution {
                error: Box::new(error),
            },
        }
    }
}

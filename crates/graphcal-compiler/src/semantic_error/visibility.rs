//! Diagnostics of item visibility and bindable interfaces.
//!
//! Each variant is a typed payload located by a
//! [`Diagnostic`](crate::diagnostic::Diagnostic); its stable code, labels,
//! and help are described by its [`DiagnosticKind`] implementation.

use thiserror::Error;

use crate::declaration_kind::DeclarationKind;
use crate::diagnostic::{DiagnosticKind, SecondaryLabel};
use crate::semantic_error::graph::DagReference;
use crate::static_interface::StaticInputKind;
use crate::syntax::decl_name::DeclName;
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;

/// The kind of bindable symbol an include overrides without reconciling
/// every declaration that mentions it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverriddenKind {
    Index,
    Type,
}

impl std::fmt::Display for OverriddenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Index => "index",
            Self::Type => "type",
        })
    }
}

/// Diagnostics of item visibility and bindable interfaces.
#[derive(Debug, Clone, Error)]
pub enum VisibilityError {
    // --- Visibility errors ---
    /// Attempting to import a private (non-`pub`) item from another file.
    #[error("cannot import private item `{name}` from `{file_path}`")]
    ImportPrivateItem {
        name: String,
        file_path: DagReference,
    },
    /// A required `index`, `type`, or `dim` is not marked `pub(bind)`.
    ///
    /// `param` is excluded: the declaration kind itself creates a required or
    /// defaulted input port and never carries a visibility annotation.
    #[error("required {kind} `{name}` must be declared `pub(bind)`")]
    RequiredItemMustBeBindable {
        kind: StaticInputKind,
        name: NameAtom,
    },
    /// A visible declaration references a private type-system item in
    /// its written signature (A9 case 1).
    ///
    /// `pub_kind` is the externally visible declaration category. A `param`
    /// contributes an input-port signature rather than an explicitly exported
    /// signature.
    #[error(
        "`{pub_kind}` `{pub_name}` references private {ref_kind} `{ref_name}` in its signature"
    )]
    PrivateInPublic {
        pub_kind: DeclarationKind,
        pub_name: NameAtom,
        ref_kind: DeclarationKind,
        ref_name: NameAtom,
        pub_span: Span,
    },
    /// A `pub(bind)` index with concrete variants has its variants used
    /// in a non-bindable body (`node` / `const`) or a public sink
    /// declaration in the defining file.
    ///
    /// Per axiom A10(c) / A10(b), a bindable index's variant literals
    /// must not appear in bodies that cannot themselves be re-bound by
    /// importers (the defining library must abstract over the index).
    #[error(
        "variant literal `{index}#{variant}` of `pub(bind) index` cannot be used in the defining file"
    )]
    PubIndexVariantLiteral {
        index: IndexName,
        variant: IndexVariantName,
    },
    /// An include overrides a bindable symbol `s`, but some kept
    /// declaration's body or default mentions a name nominally tied to
    /// `s` and was not itself re-bound by the same include statement
    /// (A8).
    ///
    /// Nominally-tied mentions today are: variant literals `s.v` for
    /// an overridden `index`, and constructors / field accesses of `s`
    /// for an overridden `type`. `dim` and `param` overrides are
    /// vacuous for A8 — their substitution is total — so they never
    /// trigger this error.
    #[error(
        "include overrides {overridden_kind} `{overridden}` but does not re-bind `{orphan_decl}`, whose default mentions `{detail}`"
    )]
    IncludeMustReconcileOverride {
        overridden: String,
        overridden_kind: OverriddenKind,
        orphan_decl: DeclName,
        detail: String,
    },
    /// A selectively re-exported import/include item (`{ pub item }`)
    /// has an effective (post-substitution) signature that mentions a symbol
    /// that is `V = private` at the importing site — A9 case 2 / visibility
    /// composition.
    ///
    /// Concretely: an include binding renames a bindable symbol `s`
    /// in the dep to a name that is private at the importer, and the
    /// re-exported surface of the include carries that name into the
    /// importer's public API. Downstream consumers of the importer
    /// would see a signature referring to a symbol they cannot name.
    #[error(
        "re-exported {reexport_kind} `{reexport_name}`'s signature references private {leaked_kind} `{leaked_name}`"
    )]
    GenericsLeakage {
        reexport_kind: String,
        reexport_name: String,
        leaked_kind: String,
        leaked_name: String,
    },
    /// A template body observes the concrete default of an optional Static port.
    ///
    /// Templates are checked once with every `pub(bind)` Static port rigid.
    /// Parameter defaults are exempt because V005 reconciles them at include
    /// sites; executable bodies and sinks must remain valid for every binding.
    #[error(
        "{body_kind} `{body_name}` depends on the default of `pub(bind) {port_kind} {port_name}`"
    )]
    TemplateBodyDependsOnStaticDefault {
        body_kind: DeclarationKind,
        body_name: NameAtom,
        port_kind: crate::static_interface::StaticInputKind,
        port_name: NameAtom,
    },
}

impl DiagnosticKind for VisibilityError {
    fn code(&self) -> &'static str {
        match self {
            Self::ImportPrivateItem { .. } => "graphcal::V001",
            Self::RequiredItemMustBeBindable { .. } => "graphcal::V002",
            Self::PrivateInPublic { .. } => "graphcal::V003",
            Self::PubIndexVariantLiteral { .. } => "graphcal::V004",
            Self::IncludeMustReconcileOverride { .. } => "graphcal::V005",
            Self::GenericsLeakage { .. } => "graphcal::V006",
            Self::TemplateBodyDependsOnStaticDefault { .. } => "graphcal::V007",
        }
    }

    fn primary_label(&self) -> Option<String> {
        match self {
            Self::ImportPrivateItem { .. } => Some("not visible — item is private".to_owned()),
            Self::RequiredItemMustBeBindable { .. } => {
                Some("required item must be `pub(bind)`".to_owned())
            }
            Self::PrivateInPublic { ref_name, .. } => {
                Some(format!("references private `{ref_name}`"))
            }
            Self::PubIndexVariantLiteral { .. } => {
                Some("variant literal of pub(bind) index".to_owned())
            }
            Self::IncludeMustReconcileOverride { orphan_decl, .. } => {
                Some(format!("include is missing a binding for `{orphan_decl}`"))
            }
            Self::GenericsLeakage { leaked_name, .. } => Some(format!(
                "leaks private `{leaked_name}` across the include boundary"
            )),
            Self::TemplateBodyDependsOnStaticDefault { .. } => {
                Some("uses the bindable port's default definition".to_owned())
            }
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::ImportPrivateItem { .. } => Some("add `pub` to the declaration in the source file to make it importable".to_owned()),
            Self::RequiredItemMustBeBindable { .. } => Some("required indexes, types, and dimensions form the bindable interface — add `pub(bind)` before the declaration".to_owned()),
            Self::PrivateInPublic { pub_name, ref_name, .. } => Some(format!("add `pub` to `{ref_name}` so it is visible across the include boundary, or stop exposing `{pub_name}`")),
            Self::PubIndexVariantLiteral { .. } => Some("pub(bind) indexes may be overridden by importers; use `param` declarations for variant-specific values, or abstract over the index via `for p : I { … }`".to_string()),
            Self::IncludeMustReconcileOverride { orphan_decl, overridden, .. } => Some(format!("add a binding for `{orphan_decl}` to this include, or keep `{overridden}` bound to its default")),
            Self::GenericsLeakage { leaked_name, .. } => Some(format!("make `{leaked_name}` `pub` at the importing file, or drop the per-item `pub` marker on this include / import")),
            Self::TemplateBodyDependsOnStaticDefault { port_name, .. } => Some(format!("pass the required value through a `param`, or make `{port_name}` non-bindable when its concrete definition is part of the template contract")),
        }
    }

    fn secondary_labels(&self) -> Vec<SecondaryLabel> {
        match self {
            Self::ImportPrivateItem { .. }
            | Self::RequiredItemMustBeBindable { .. }
            | Self::PubIndexVariantLiteral { .. }
            | Self::IncludeMustReconcileOverride { .. }
            | Self::GenericsLeakage { .. }
            | Self::TemplateBodyDependsOnStaticDefault { .. } => Vec::new(),
            Self::PrivateInPublic { pub_span, .. } => vec![SecondaryLabel {
                span: *pub_span,
                text: "visible declaration is here".to_owned(),
            }],
        }
    }
}

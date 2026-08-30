//! Shared semantic vocabulary for typed Static input ports.
//!
//! Syntax, required-bindability validation, import capability checks, and
//! include binding construction all consume this module so their category and
//! role inventories cannot drift.

use std::fmt;

use crate::desugar::desugared_ast::{DeclKind, IndexDeclKind, TypeDeclBody};
use crate::syntax::ast::BindableVisibility;

/// Closed set of Static declaration categories accepted by typed DAG bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticInputKind {
    Type,
    Dimension,
    Index,
}

impl StaticInputKind {
    /// Source marker selecting this category.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Dimension => "dim",
            Self::Index => "index",
        }
    }
}

impl fmt::Display for StaticInputKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.marker())
    }
}

/// Whether a Static declaration has a local definition or requires a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Requirement {
    Defaulted,
    Required,
}

/// Validated role of an input-capable Static declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticRole {
    /// Ordinary declaration that cannot be overridden.
    Fixed,
    /// `pub(bind)` declaration retaining a local default.
    OptionalInput,
    /// `pub(bind)` declaration with no local definition.
    RequiredInput,
}

impl StaticRole {
    /// Whether callers may supply a typed Static binding.
    #[must_use]
    pub const fn is_bindable(self) -> bool {
        matches!(self, Self::OptionalInput | Self::RequiredInput)
    }

    /// Whether callers must supply a typed Static binding.
    #[must_use]
    pub const fn is_required(self) -> bool {
        matches!(self, Self::RequiredInput)
    }
}

/// Typed Static declaration interface after parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticInterface {
    kind: StaticInputKind,
    role: StaticRole,
}

impl StaticInterface {
    #[must_use]
    pub const fn new(kind: StaticInputKind, role: StaticRole) -> Self {
        Self { kind, role }
    }

    #[must_use]
    pub const fn kind(self) -> StaticInputKind {
        self.kind
    }

    #[must_use]
    pub const fn role(self) -> StaticRole {
        self.role
    }
}

/// Construct the semantic role represented by visibility and requirement.
///
/// A required declaration without `pub(bind)` is rejected by V002 before this
/// interface reaches project composition, so it has no valid semantic role.
#[must_use]
pub const fn static_role_of(
    visibility: BindableVisibility,
    requirement: Requirement,
) -> Option<StaticRole> {
    match (visibility, requirement) {
        (BindableVisibility::PublicBind, Requirement::Defaulted) => Some(StaticRole::OptionalInput),
        (BindableVisibility::PublicBind, Requirement::Required) => Some(StaticRole::RequiredInput),
        (BindableVisibility::Private | BindableVisibility::Public, Requirement::Defaulted) => {
            Some(StaticRole::Fixed)
        }
        (BindableVisibility::Private | BindableVisibility::Public, Requirement::Required) => None,
    }
}

/// Project a declaration onto its typed Static interface.
///
/// `None` means the declaration is not one of the three bindable Static
/// categories. An invalid required declaration also returns `None`; V002 emits
/// its source diagnostic before project composition.
#[must_use]
pub fn static_interface(declaration: &DeclKind) -> Option<StaticInterface> {
    let (kind, visibility, requirement) = match declaration {
        DeclKind::BaseDimension(_dimension) => {
            return Some(StaticInterface::new(
                StaticInputKind::Dimension,
                StaticRole::Fixed,
            ));
        }
        DeclKind::Dimension(dimension) => (
            StaticInputKind::Dimension,
            dimension.visibility,
            if dimension.definition.is_none() {
                Requirement::Required
            } else {
                Requirement::Defaulted
            },
        ),
        DeclKind::Type(type_decl) => (
            StaticInputKind::Type,
            type_decl.visibility,
            if matches!(type_decl.body, TypeDeclBody::Required) {
                Requirement::Required
            } else {
                Requirement::Defaulted
            },
        ),
        DeclKind::Index(index) => (
            StaticInputKind::Index,
            index.visibility,
            if matches!(
                index.kind,
                IndexDeclKind::RequiredNamed | IndexDeclKind::RequiredCoordinate { .. }
            ) {
                Requirement::Required
            } else {
                Requirement::Defaulted
            },
        ),
        _ => return None,
    };
    static_role_of(visibility, requirement).map(|role| StaticInterface::new(kind, role))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn interfaces(source: &str) -> Vec<StaticInterface> {
        let file = Parser::new(source).parse_file().expect("source parses");
        crate::syntax::desugar::desugar_multi_decls_in_file(file)
            .declarations
            .iter()
            .filter_map(|declaration| static_interface(&declaration.kind))
            .collect()
    }

    #[test]
    fn classifies_fixed_optional_and_required_roles() {
        assert_eq!(
            interfaces(
                "pub type Fixed { Fixed }\n\
                 pub(bind) dim Optional = Length;\n\
                 pub(bind) index Required;",
            ),
            vec![
                StaticInterface::new(StaticInputKind::Type, StaticRole::Fixed),
                StaticInterface::new(StaticInputKind::Dimension, StaticRole::OptionalInput,),
                StaticInterface::new(StaticInputKind::Index, StaticRole::RequiredInput),
            ]
        );
    }
}

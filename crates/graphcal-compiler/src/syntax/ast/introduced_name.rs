//! Names a desugared declaration introduces into its module's namespaces.
//!
//! This is the single projection from a declaration onto its bound names:
//! the kind, namespace, spelling, span, and external exposure of every name a
//! declaration adds to its module. Name-collision checks, builtin-shadowing
//! checks, external surfaces, and module interfaces all consume it instead of
//! re-matching every `DeclKind` variant.

use crate::syntax::import_category::ImportItemNamespace;
use crate::syntax::names::NameAtom;
use crate::syntax::phase::{Desugared, never};
use crate::syntax::span::Span;

use super::{DeclKind, TypeDeclBody};

/// Declaration role that introduces a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntroducedKind {
    Param,
    Node,
    ConstNode,
    Assert,
    Plot,
    Figure,
    Layer,
    Dag,
    /// A constructor of a `type` declaration's tagged-union body.
    Constructor,
    BaseDimension,
    Dimension,
    Unit,
    Type,
    Index,
}

impl IntroducedKind {
    /// Namespace the introduced name occupies.
    #[must_use]
    pub const fn namespace(self) -> ImportItemNamespace {
        match self {
            Self::Param
            | Self::Node
            | Self::ConstNode
            | Self::Assert
            | Self::Plot
            | Self::Figure
            | Self::Layer
            | Self::Dag
            | Self::Constructor => ImportItemNamespace::Term,
            Self::BaseDimension | Self::Dimension => ImportItemNamespace::Dimension,
            Self::Unit => ImportItemNamespace::Unit,
            Self::Type => ImportItemNamespace::Type,
            Self::Index => ImportItemNamespace::Index,
        }
    }

    /// Diagnostic noun for the introducing declaration.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Param => "param",
            Self::Node => "node",
            Self::ConstNode => "const node",
            Self::Assert => "assert",
            Self::Plot => "plot",
            Self::Figure => "figure",
            Self::Layer => "layer",
            Self::Dag => "dag",
            Self::Constructor => "constructor",
            Self::BaseDimension | Self::Dimension => "dimension",
            Self::Unit => "unit",
            Self::Type => "type",
            Self::Index => "index",
        }
    }
}

/// How an introduced name is exposed at its module's external boundary.
///
/// Ordered by precedence: when several declarations share one name and
/// namespace, the most exposed role wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeclExposure {
    /// Visible only inside the declaring module.
    Private,
    /// A `param` named input port; its value is externally selectable.
    InputPort,
    /// Explicitly exported with `pub` / `pub(bind)` (or a `pub` re-export).
    ExplicitExport,
}

impl DeclExposure {
    const fn from_public(is_public: bool) -> Self {
        if is_public {
            Self::ExplicitExport
        } else {
            Self::Private
        }
    }

    /// Whether an importer may select the name as an output.
    #[must_use]
    pub const fn can_select_output(self) -> bool {
        matches!(self, Self::ExplicitExport | Self::InputPort)
    }
}

/// One name introduced by a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntroducedName<'a> {
    kind: IntroducedKind,
    atom: &'a NameAtom,
    span: Span,
    exposure: DeclExposure,
}

impl<'a> IntroducedName<'a> {
    #[must_use]
    pub const fn kind(self) -> IntroducedKind {
        self.kind
    }

    #[must_use]
    pub const fn namespace(self) -> ImportItemNamespace {
        self.kind.namespace()
    }

    #[must_use]
    pub const fn atom(self) -> &'a NameAtom {
        self.atom
    }

    /// Span of the name's spelling at its definition site.
    #[must_use]
    pub const fn span(self) -> Span {
        self.span
    }

    #[must_use]
    pub const fn exposure(self) -> DeclExposure {
        self.exposure
    }
}

impl DeclKind<Desugared> {
    /// The name this declaration itself binds, if any.
    ///
    /// Use-sites (`import`, `import plugin`, `include`) bind no declaration
    /// name. Constructors of a `type` declaration are not included; see
    /// [`Self::introduced_names`].
    #[must_use]
    pub const fn declared_name(&self) -> Option<IntroducedName<'_>> {
        let (kind, atom, span, exposure) = match self {
            Self::Param(d) => (
                IntroducedKind::Param,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::InputPort,
            ),
            Self::Node(d) => (
                IntroducedKind::Node,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::ConstNode(d) => (
                IntroducedKind::ConstNode,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Assert(d) => (
                IntroducedKind::Assert,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Plot(d) => (
                IntroducedKind::Plot,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Figure(d) => (
                IntroducedKind::Figure,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Layer(d) => (
                IntroducedKind::Layer,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Dag(d) => (
                IntroducedKind::Dag,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::BaseDimension(d) => (
                IntroducedKind::BaseDimension,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Dimension(d) => (
                IntroducedKind::Dimension,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Unit(d) => (
                IntroducedKind::Unit,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Type(d) => (
                IntroducedKind::Type,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Index(d) => (
                IntroducedKind::Index,
                d.name.value.atom(),
                d.name.span,
                DeclExposure::from_public(d.visibility.is_public()),
            ),
            Self::Import(_) | Self::PluginImport(_) | Self::Include(_) => return None,
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            Self::Sugar(s) => never(*s),
        };
        Some(IntroducedName {
            kind,
            atom,
            span,
            exposure,
        })
    }

    /// Every name this declaration introduces: its own name followed by the
    /// constructors of a `type` declaration's tagged-union body, in source
    /// order. Constructors share their type's exposure.
    pub fn introduced_names(&self) -> impl Iterator<Item = IntroducedName<'_>> {
        let declared = self.declared_name();
        let constructors = match self {
            Self::Type(type_decl) => match &type_decl.body {
                TypeDeclBody::Constructors(members) => members.as_slice(),
                TypeDeclBody::Required => &[],
            },
            Self::Param(_)
            | Self::Node(_)
            | Self::ConstNode(_)
            | Self::BaseDimension(_)
            | Self::Dimension(_)
            | Self::Unit(_)
            | Self::Index(_)
            | Self::Import(_)
            | Self::PluginImport(_)
            | Self::Include(_)
            | Self::Dag(_)
            | Self::Assert(_)
            | Self::Plot(_)
            | Self::Figure(_)
            | Self::Layer(_) => &[],
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            Self::Sugar(s) => never(*s),
        };
        let exposure = declared.map_or(DeclExposure::Private, IntroducedName::exposure);
        declared
            .into_iter()
            .chain(constructors.iter().map(move |member| IntroducedName {
                kind: IntroducedKind::Constructor,
                atom: member.name.value.atom(),
                span: member.name.span,
                exposure,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::File;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> File<Desugared> {
        let raw = Parser::new(source).parse_file().expect("source parses");
        File::<Desugared>::from(raw)
    }

    fn introduced(source: &str) -> Vec<(IntroducedKind, String, DeclExposure)> {
        parse(source)
            .declarations
            .iter()
            .flat_map(|declaration| {
                declaration
                    .kind
                    .introduced_names()
                    .map(|name| (name.kind(), name.atom().to_string(), name.exposure()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn every_declaration_role_projects_its_name_namespace_and_exposure() {
        let names = introduced(
            "param p: Dimensionless = 1.0;\n\
             pub node n: Dimensionless = 1.0;\n\
             const node c: Dimensionless = 1.0;\n\
             pub assert a = true;\n\
             plot pl = { mark: point, encode: { x: 1.0 } };\n\
             pub base dim B;\n\
             pub(bind) dim D;\n\
             const unit u: Length = 2.0 m;\n\
             pub(bind) type T;\n\
             index I = { x, y };\n\
             pub dag g { }\n\
             import lib;\n",
        );
        assert_eq!(
            names,
            vec![
                (IntroducedKind::Param, "p".into(), DeclExposure::InputPort),
                (
                    IntroducedKind::Node,
                    "n".into(),
                    DeclExposure::ExplicitExport
                ),
                (IntroducedKind::ConstNode, "c".into(), DeclExposure::Private),
                (
                    IntroducedKind::Assert,
                    "a".into(),
                    DeclExposure::ExplicitExport
                ),
                (IntroducedKind::Plot, "pl".into(), DeclExposure::Private),
                (
                    IntroducedKind::BaseDimension,
                    "B".into(),
                    DeclExposure::ExplicitExport
                ),
                (
                    IntroducedKind::Dimension,
                    "D".into(),
                    DeclExposure::ExplicitExport
                ),
                (IntroducedKind::Unit, "u".into(), DeclExposure::Private),
                (
                    IntroducedKind::Type,
                    "T".into(),
                    DeclExposure::ExplicitExport
                ),
                (IntroducedKind::Index, "I".into(), DeclExposure::Private),
                (
                    IntroducedKind::Dag,
                    "g".into(),
                    DeclExposure::ExplicitExport
                ),
            ]
        );
    }

    #[test]
    fn type_constructors_follow_the_type_name_and_share_its_exposure() {
        assert_eq!(
            introduced("pub type Choice { Pick, Skip(x: Dimensionless) }\ntype Hidden { Only }\n"),
            vec![
                (
                    IntroducedKind::Type,
                    "Choice".into(),
                    DeclExposure::ExplicitExport
                ),
                (
                    IntroducedKind::Constructor,
                    "Pick".into(),
                    DeclExposure::ExplicitExport
                ),
                (
                    IntroducedKind::Constructor,
                    "Skip".into(),
                    DeclExposure::ExplicitExport
                ),
                (IntroducedKind::Type, "Hidden".into(), DeclExposure::Private),
                (
                    IntroducedKind::Constructor,
                    "Only".into(),
                    DeclExposure::Private
                ),
            ]
        );
    }

    #[test]
    fn kinds_map_to_their_import_namespace_and_diagnostic_noun() {
        let cases = [
            (IntroducedKind::Param, ImportItemNamespace::Term, "param"),
            (IntroducedKind::Node, ImportItemNamespace::Term, "node"),
            (
                IntroducedKind::ConstNode,
                ImportItemNamespace::Term,
                "const node",
            ),
            (IntroducedKind::Assert, ImportItemNamespace::Term, "assert"),
            (IntroducedKind::Plot, ImportItemNamespace::Term, "plot"),
            (IntroducedKind::Figure, ImportItemNamespace::Term, "figure"),
            (IntroducedKind::Layer, ImportItemNamespace::Term, "layer"),
            (IntroducedKind::Dag, ImportItemNamespace::Term, "dag"),
            (
                IntroducedKind::Constructor,
                ImportItemNamespace::Term,
                "constructor",
            ),
            (
                IntroducedKind::BaseDimension,
                ImportItemNamespace::Dimension,
                "dimension",
            ),
            (
                IntroducedKind::Dimension,
                ImportItemNamespace::Dimension,
                "dimension",
            ),
            (IntroducedKind::Unit, ImportItemNamespace::Unit, "unit"),
            (IntroducedKind::Type, ImportItemNamespace::Type, "type"),
            (IntroducedKind::Index, ImportItemNamespace::Index, "index"),
        ];
        for (kind, namespace, noun) in cases {
            assert_eq!(kind.namespace(), namespace, "{kind:?}");
            assert_eq!(kind.describe(), noun, "{kind:?}");
        }
    }

    #[test]
    fn exposure_precedence_and_output_selection() {
        assert!(DeclExposure::Private < DeclExposure::InputPort);
        assert!(DeclExposure::InputPort < DeclExposure::ExplicitExport);
        assert!(!DeclExposure::Private.can_select_output());
        assert!(DeclExposure::InputPort.can_select_output());
        assert!(DeclExposure::ExplicitExport.can_select_output());
    }
}

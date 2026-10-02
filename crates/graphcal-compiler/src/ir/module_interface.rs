//! A module's declared interface, computed once from its declaration list.
//!
//! Every consumer that asks "what does this module declare, in which
//! namespace, and how is it exposed?" — selective-import visibility checks,
//! include binding classification, pure-import term policy, include output
//! surfaces, and the external declaration surface — reads one
//! [`ModuleInterface`] instead of re-walking the module's AST.

use std::collections::{HashMap, HashSet};

use crate::desugar::desugared_ast::{DeclKind, Declaration, ImportDecl, ImportKind};
use crate::ir::resolve::collected::ExternalDeclSurface;
use crate::semantic_error::SemanticError;
use crate::semantic_error::module::ModuleError;
use crate::source_id::SourceId;
use crate::static_interface::{StaticInputKind, StaticInterface, StaticRole, static_interface};
use crate::syntax::ast::{DeclExposure, ImportItemNamespace, IntroducedKind};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::UnitName;
use crate::syntax::names::NameAtom;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;

/// Canonical order in which namespaces are reported for one spelling.
const NAMESPACE_ORDER: [ImportItemNamespace; 5] = [
    ImportItemNamespace::Term,
    ImportItemNamespace::Type,
    ImportItemNamespace::Dimension,
    ImportItemNamespace::Unit,
    ImportItemNamespace::Index,
];

/// Where one importable name of a module comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceOrigin {
    /// Introduced by one of the module's own declarations.
    Declared(IntroducedKind),
    /// A `pub` item of a selective `import` or `include` in the module.
    ReExport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InterfaceEntry {
    origin: InterfaceOrigin,
    exposure: DeclExposure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InterfaceSlot {
    namespace: ImportItemNamespace,
    name: NameAtom,
}

impl InterfaceSlot {
    fn new(namespace: ImportItemNamespace, name: &NameAtom) -> Self {
        Self {
            namespace,
            name: name.clone(),
        }
    }
}

/// Compile-time policy for a term selected by a pure import.
///
/// Both cross-file imports and an inline DAG's `import <self>::{...}` consume
/// this classification. Keeping the permitted actions and rejection reasons
/// exhaustive prevents either path from silently accepting a new declaration
/// kind when the language grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureImportTermDisposition {
    /// Materialize a compile-time constant binding in the importer.
    BindConstant,
    /// Keep the name available only to compile-time/type-system resolution,
    /// such as an algebraic constructor or DAG blueprint.
    ResolverOnly,
    /// Reject a term that requires a concrete instance boundary.
    Reject(PureImportRejection),
}

/// Why a term cannot cross a pure-import boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureImportRejection {
    Runtime,
    Assertion,
    Visualization,
}

impl PureImportRejection {
    /// Build the diagnostic shared by cross-file and inline-self imports.
    #[must_use]
    pub fn diagnostic(self, name: &NameAtom, src: SourceId, span: Span) -> SemanticError {
        let name = name.clone();
        match self {
            Self::Runtime => {
                SemanticError::located(src, span, ModuleError::ImportRuntimeItem { name })
            }
            Self::Assertion => {
                SemanticError::located(src, span, ModuleError::ImportAssertionItem { name })
            }
            Self::Visualization => {
                SemanticError::located(src, span, ModuleError::ImportPlotItem { name })
            }
        }
    }
}

impl PureImportTermDisposition {
    const fn precedence(self) -> u8 {
        match self {
            Self::Reject(PureImportRejection::Visualization) => 4,
            Self::Reject(PureImportRejection::Runtime) => 3,
            Self::Reject(PureImportRejection::Assertion) => 2,
            Self::BindConstant => 1,
            Self::ResolverOnly => 0,
        }
    }

    const fn combine(self, other: Self) -> Self {
        if self.precedence() >= other.precedence() {
            self
        } else {
            other
        }
    }

    /// Policy for one Term-namespace origin, or `None` for an origin that
    /// cannot occupy the Term namespace.
    const fn of(origin: InterfaceOrigin) -> Option<Self> {
        match origin {
            InterfaceOrigin::Declared(IntroducedKind::ConstNode) => Some(Self::BindConstant),
            InterfaceOrigin::Declared(IntroducedKind::Param | IntroducedKind::Node) => {
                Some(Self::Reject(PureImportRejection::Runtime))
            }
            InterfaceOrigin::Declared(IntroducedKind::Assert) => {
                Some(Self::Reject(PureImportRejection::Assertion))
            }
            InterfaceOrigin::Declared(
                IntroducedKind::Plot | IntroducedKind::Figure | IntroducedKind::Layer,
            ) => Some(Self::Reject(PureImportRejection::Visualization)),
            InterfaceOrigin::Declared(IntroducedKind::Dag | IntroducedKind::Constructor)
            | InterfaceOrigin::ReExport => Some(Self::ResolverOnly),
            InterfaceOrigin::Declared(
                IntroducedKind::BaseDimension
                | IntroducedKind::Dimension
                | IntroducedKind::Unit
                | IntroducedKind::Type
                | IntroducedKind::Index,
            ) => None,
        }
    }
}

/// One graph value a whole-instance include exposes under its prefix: every
/// `param` input port and every explicitly exported `const node` / `node`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueOutput {
    name: DeclName,
    kind: IntroducedKind,
}

impl ValueOutput {
    #[must_use]
    pub const fn name(&self) -> &DeclName {
        &self.name
    }

    /// Whether the output is a compile-time `const node`.
    #[must_use]
    pub const fn is_const(&self) -> bool {
        matches!(self.kind, IntroducedKind::ConstNode)
    }
}

/// The declared interface of one DAG module (a file root or an inline `dag`
/// body), computed once from its declarations.
#[derive(Debug, Clone, Default)]
pub struct ModuleInterface {
    /// Every importable name by namespace, in source order.
    slots: HashMap<InterfaceSlot, Vec<InterfaceEntry>>,
    /// Typed Static interfaces of dimension, type, and index declarations.
    /// The first declaration of a name wins.
    static_roles: HashMap<(StaticInputKind, NameAtom), StaticRole>,
    /// Static declarations in source order, for deterministic iteration.
    static_order: Vec<(StaticInputKind, NameAtom)>,
    /// `param` declarations without a default, in source order.
    required_params: Vec<DeclName>,
    value_outputs: Vec<ValueOutput>,
    /// Explicitly exported runtime (non-`const`) units.
    runtime_units: HashSet<UnitName>,
    /// Externally addressable names introduced by the module's own
    /// declarations; re-exports are excluded.
    declared_surface: ExternalDeclSurface,
    external_surface: ExternalDeclSurface,
}

impl ModuleInterface {
    /// Compute the interface of the module whose body is `declarations`.
    #[must_use]
    pub fn new(declarations: &[Declaration]) -> Self {
        let mut interface = Self::default();
        for declaration in declarations {
            interface.add_declaration(&declaration.kind);
        }
        interface
    }

    fn insert(&mut self, namespace: ImportItemNamespace, name: &NameAtom, entry: InterfaceEntry) {
        self.slots
            .entry(InterfaceSlot::new(namespace, name))
            .or_default()
            .push(entry);
    }

    fn insert_reexport(&mut self, namespace: ImportItemNamespace, name: &NameAtom) {
        self.insert(
            namespace,
            name,
            InterfaceEntry {
                origin: InterfaceOrigin::ReExport,
                exposure: DeclExposure::ExplicitExport,
            },
        );
        self.external_surface
            .record(namespace, name.clone(), DeclExposure::ExplicitExport);
    }

    fn add_declaration(&mut self, kind: &DeclKind) {
        for introduced in kind.introduced_names() {
            self.insert(
                introduced.namespace(),
                introduced.atom(),
                InterfaceEntry {
                    origin: InterfaceOrigin::Declared(introduced.kind()),
                    exposure: introduced.exposure(),
                },
            );
        }
        if let Some(declared) = kind.declared_name() {
            self.declared_surface.record_declared(declared);
            self.external_surface.record_declared(declared);
        }
        if let Some(interface) = static_interface(kind)
            && let Some(declared) = kind.declared_name()
        {
            let key = (interface.kind(), declared.atom().clone());
            if !self.static_roles.contains_key(&key) {
                self.static_roles.insert(key.clone(), interface.role());
                self.static_order.push(key);
            }
        }
        match kind {
            DeclKind::Param(param) => {
                if param.value.is_none() {
                    self.required_params.push(param.name.value.clone());
                }
                self.value_outputs.push(ValueOutput {
                    name: param.name.value.clone(),
                    kind: IntroducedKind::Param,
                });
            }
            DeclKind::ConstNode(constant) if constant.visibility.is_public() => {
                self.value_outputs.push(ValueOutput {
                    name: constant.name.value.clone(),
                    kind: IntroducedKind::ConstNode,
                });
            }
            DeclKind::Node(node) if node.visibility.is_public() => {
                self.value_outputs.push(ValueOutput {
                    name: node.name.value.clone(),
                    kind: IntroducedKind::Node,
                });
            }
            DeclKind::Unit(unit) if unit.visibility.is_public() && !unit.constness.is_const() => {
                self.runtime_units.insert(unit.name.value.clone());
            }
            DeclKind::Import(ImportDecl::Selective { items, .. }) => {
                for item in items.iter().filter(|item| item.visibility.is_public()) {
                    self.insert_reexport(item.namespace, item.local_name_atom());
                }
            }
            DeclKind::Import(ImportDecl::Module {
                visibility,
                path,
                alias,
            }) => {
                // A `pub` module import re-exports its alias as a Term of the
                // external surface; it is not a selectable import item.
                if visibility.is_public() {
                    let name = alias.as_ref().map_or_else(
                        || path.leaf().name.atom().clone(),
                        |alias| alias.value.atom().clone(),
                    );
                    self.external_surface.record(
                        ImportItemNamespace::Term,
                        name,
                        DeclExposure::ExplicitExport,
                    );
                }
            }
            DeclKind::Include(include) => match &include.kind {
                // Selected include items re-export instance outputs, which
                // always live in the Term namespace.
                ImportKind::Selective(items) => {
                    for item in items.iter().filter(|item| item.visibility.is_public()) {
                        self.insert_reexport(ImportItemNamespace::Term, item.local_name_atom());
                    }
                }
                ImportKind::Module { .. } => {}
            },
            DeclKind::ConstNode(_)
            | DeclKind::Node(_)
            | DeclKind::Unit(_)
            | DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Dag(_)
            | DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_) => {}
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(sugar) => crate::syntax::phase::never(*sugar),
        }
    }

    fn entries(
        &self,
        name: &NameAtom,
        namespace: ImportItemNamespace,
    ) -> impl Iterator<Item = InterfaceEntry> + '_ {
        self.slots
            .get(&InterfaceSlot::new(namespace, name))
            .into_iter()
            .flatten()
            .copied()
    }

    /// The most exposed role `name` has in `namespace`, or `None` when the
    /// module has no importable item of that name there.
    #[must_use]
    pub fn exposure(
        &self,
        name: &NameAtom,
        namespace: ImportItemNamespace,
    ) -> Option<DeclExposure> {
        self.entries(name, namespace)
            .map(|entry| entry.exposure)
            .max()
    }

    /// Whether the module has any importable item `name` in `namespace`.
    #[must_use]
    pub fn has_item(&self, name: &NameAtom, namespace: ImportItemNamespace) -> bool {
        self.exposure(name, namespace).is_some()
    }

    /// Whether an importer may select `name` in `namespace` as an output.
    #[must_use]
    pub fn exposes_item(&self, name: &NameAtom, namespace: ImportItemNamespace) -> bool {
        self.exposure(name, namespace)
            .is_some_and(DeclExposure::can_select_output)
    }

    /// Namespaces in which `name` is importable, in canonical marker order.
    #[must_use]
    pub fn namespaces_of(&self, name: &NameAtom) -> Option<NonEmpty<ImportItemNamespace>> {
        let namespaces = NAMESPACE_ORDER
            .into_iter()
            .filter(|namespace| self.has_item(name, *namespace))
            .collect();
        NonEmpty::try_from_vec(namespaces).ok()
    }

    /// Whether one of the module's own declarations introduces `name` with
    /// `kind` (in `kind`'s namespace), regardless of visibility.
    #[must_use]
    pub fn declares(&self, name: &NameAtom, kind: IntroducedKind) -> bool {
        self.entries(name, kind.namespace())
            .any(|entry| entry.origin == InterfaceOrigin::Declared(kind))
    }

    /// Whether one of the module's own declarations introduces `name` with
    /// `kind` and explicitly exports it.
    #[must_use]
    pub fn explicitly_exports(&self, name: &NameAtom, kind: IntroducedKind) -> bool {
        self.entries(name, kind.namespace()).any(|entry| {
            entry.origin == InterfaceOrigin::Declared(kind)
                && entry.exposure == DeclExposure::ExplicitExport
        })
    }

    /// Kinds of the module's own declarations introducing `name` in
    /// `namespace`, in source order.
    pub fn declared_kinds(
        &self,
        name: &NameAtom,
        namespace: ImportItemNamespace,
    ) -> impl Iterator<Item = IntroducedKind> + '_ {
        self.entries(name, namespace)
            .filter_map(|entry| match entry.origin {
                InterfaceOrigin::Declared(kind) => Some(kind),
                InterfaceOrigin::ReExport => None,
            })
    }

    /// Classify a Term by the action a pure import must take.
    ///
    /// `None` means nothing occupies the Term namespace under `name`.
    /// Visibility is an independent boundary and must be checked before
    /// applying this policy.
    #[must_use]
    pub fn pure_import_term_disposition(
        &self,
        name: &NameAtom,
    ) -> Option<PureImportTermDisposition> {
        self.entries(name, ImportItemNamespace::Term)
            .filter_map(|entry| PureImportTermDisposition::of(entry.origin))
            .reduce(PureImportTermDisposition::combine)
    }

    /// Typed Static interface of the local dimension, type, or index `name`.
    #[must_use]
    pub fn static_interface(
        &self,
        kind: StaticInputKind,
        name: &NameAtom,
    ) -> Option<StaticInterface> {
        self.static_roles
            .get(&(kind, name.clone()))
            .map(|role| StaticInterface::new(kind, *role))
    }

    /// Every local Static declaration with its role, in source order.
    pub fn static_declarations(
        &self,
    ) -> impl Iterator<Item = (StaticInputKind, &NameAtom, StaticRole)> + '_ {
        self.static_order
            .iter()
            .map(|key| (key.0, &key.1, self.static_roles[key]))
    }

    /// `param` input ports without a default, in source order.
    #[must_use]
    pub fn required_params(&self) -> &[DeclName] {
        &self.required_params
    }

    /// Graph values a whole-instance include exposes, in source order.
    #[must_use]
    pub fn value_outputs(&self) -> &[ValueOutput] {
        &self.value_outputs
    }

    /// Explicitly exported runtime (`unit`, not `const unit`) units.
    #[must_use]
    pub const fn runtime_units(&self) -> &HashSet<UnitName> {
        &self.runtime_units
    }

    /// Externally addressable declarations of the module itself: explicit
    /// exports and `param` input ports, without re-exports.
    #[must_use]
    pub const fn declared_surface(&self) -> &ExternalDeclSurface {
        &self.declared_surface
    }

    /// Externally addressable declarations: explicit exports (including
    /// `pub` re-exports) and `param` input ports.
    #[must_use]
    pub const fn external_surface(&self) -> &ExternalDeclSurface {
        &self.external_surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_error::SemanticErrorKind;
    use crate::syntax::parser::Parser;

    fn interface(source: &str) -> ModuleInterface {
        let parsed = Parser::new(source).parse_file().expect("source parses");
        ModuleInterface::new(&crate::desugar::desugared_ast::File::from(parsed).declarations)
    }

    fn atom(name: &str) -> NameAtom {
        NameAtom::parse(name).unwrap()
    }

    #[test]
    fn namespaces_preserve_legal_same_name_categories() {
        let interface = interface(
            "pub const node JPY: Dimensionless = 1.0;\n\
             pub base unit JPY: Dimensionless;\n\
             pub type Student { Student }\n",
        );

        assert_eq!(
            interface
                .namespaces_of(&atom("JPY"))
                .expect("JPY categories")
                .as_slice(),
            &[ImportItemNamespace::Term, ImportItemNamespace::Unit]
        );
        assert_eq!(
            interface
                .namespaces_of(&atom("Student"))
                .expect("Student categories")
                .as_slice(),
            &[ImportItemNamespace::Term, ImportItemNamespace::Type]
        );
        assert!(interface.namespaces_of(&atom("missing")).is_none());
    }

    #[test]
    fn pure_term_policy_classifies_every_supported_declaration_role() {
        let interface = interface(
            "pub const node constant: Dimensionless = 1.0;\n\
             param parameter: Dimensionless = 1.0;\n\
             pub node runtime_node: Dimensionless = @parameter;\n\
             pub assert assertion = true;\n\
             pub plot chart = { mark: point, encode: { x: 1.0 } };\n\
             pub figure board = { plots: [chart] };\n\
             pub type Choice { Pick }\n\
             pub dag blueprint { pub node output: Dimensionless = 1.0; }\n\
             import pkg.core::{ pub shared };\n\
             pub dim Length2 = Length;\n",
        );

        let cases = [
            ("constant", Some(PureImportTermDisposition::BindConstant)),
            (
                "parameter",
                Some(PureImportTermDisposition::Reject(
                    PureImportRejection::Runtime,
                )),
            ),
            (
                "runtime_node",
                Some(PureImportTermDisposition::Reject(
                    PureImportRejection::Runtime,
                )),
            ),
            (
                "assertion",
                Some(PureImportTermDisposition::Reject(
                    PureImportRejection::Assertion,
                )),
            ),
            (
                "chart",
                Some(PureImportTermDisposition::Reject(
                    PureImportRejection::Visualization,
                )),
            ),
            (
                "board",
                Some(PureImportTermDisposition::Reject(
                    PureImportRejection::Visualization,
                )),
            ),
            ("Pick", Some(PureImportTermDisposition::ResolverOnly)),
            ("blueprint", Some(PureImportTermDisposition::ResolverOnly)),
            ("shared", Some(PureImportTermDisposition::ResolverOnly)),
            ("Length2", None),
            ("Choice", None),
        ];

        for (name, expected) in cases {
            assert_eq!(
                interface.pure_import_term_disposition(&atom(name)),
                expected,
                "unexpected pure-import disposition for {name}"
            );
        }
    }

    #[test]
    fn pure_term_policy_prefers_the_strictest_rejection() {
        assert_eq!(
            PureImportTermDisposition::BindConstant.combine(PureImportTermDisposition::Reject(
                PureImportRejection::Runtime
            )),
            PureImportTermDisposition::Reject(PureImportRejection::Runtime)
        );
        assert_eq!(
            PureImportTermDisposition::Reject(PureImportRejection::Visualization).combine(
                PureImportTermDisposition::Reject(PureImportRejection::Runtime)
            ),
            PureImportTermDisposition::Reject(PureImportRejection::Visualization)
        );
        assert_eq!(
            PureImportTermDisposition::ResolverOnly.combine(PureImportTermDisposition::Reject(
                PureImportRejection::Assertion
            )),
            PureImportTermDisposition::Reject(PureImportRejection::Assertion)
        );
        assert_eq!(
            PureImportTermDisposition::ResolverOnly
                .combine(PureImportTermDisposition::BindConstant),
            PureImportTermDisposition::BindConstant
        );
    }

    #[test]
    fn rejection_diagnostics_name_their_boundary() {
        let src = crate::source_registry::SourceRegistry::new()
            .register("main.gcl", std::sync::Arc::new(String::new()));
        let span = Span::new(0, 0);
        assert!(matches!(
            PureImportRejection::Runtime.diagnostic(&atom("x"), src, span),
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportRuntimeItem { .. }),
                ..
            })
        ));
        assert!(matches!(
            PureImportRejection::Assertion.diagnostic(&atom("x"), src, span),
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportAssertionItem { .. }),
                ..
            })
        ));
        assert!(matches!(
            PureImportRejection::Visualization.diagnostic(&atom("x"), src, span),
            SemanticError::Located(crate::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Module(ModuleError::ImportPlotItem { .. }),
                ..
            })
        ));
    }

    #[test]
    fn namespaced_use_sites_never_widen_the_external_surface() {
        let interface = interface(
            "import pkg.core;\n\
             include pkg.engine() as engine;\n\
             import pkg.core::{ pub dim Exposed, helper };\n\
             include pkg.engine()::{ pub thrust, fuel_flow };\n",
        );
        let surface = interface.external_surface();

        assert!(surface.is_explicit_export(&DeclName::expect_valid("thrust")));
        assert!(surface.is_static_explicit_export(&atom("Exposed")));
        for private in ["Exposed", "core", "engine", "helper", "fuel_flow"] {
            assert!(
                !surface.is_externally_nameable(&DeclName::expect_valid(private)),
                "{private} must remain private"
            );
        }
        assert_eq!(
            interface.exposure(&atom("thrust"), ImportItemNamespace::Term),
            Some(DeclExposure::ExplicitExport)
        );
        assert_eq!(
            interface.exposure(&atom("Exposed"), ImportItemNamespace::Dimension),
            Some(DeclExposure::ExplicitExport)
        );
        assert_eq!(
            interface.exposure(&atom("helper"), ImportItemNamespace::Term),
            None
        );
    }

    #[test]
    fn pub_module_imports_export_their_alias_without_becoming_items() {
        let interface = interface("pub import pkg.core as kernel;\npub import pkg.shared;\n");
        let surface = interface.external_surface();
        assert!(surface.is_explicit_export(&DeclName::expect_valid("kernel")));
        assert!(surface.is_explicit_export(&DeclName::expect_valid("shared")));
        assert!(!interface.has_item(&atom("kernel"), ImportItemNamespace::Term));
    }

    #[test]
    fn declared_surface_excludes_reexports() {
        let interface = interface(
            "import pkg.core::{ pub dim Speed };\n\
             pub import pkg.shared;\n\
             param input: Dimensionless = 1.0;\n\
             pub dim Length2 = Length^2;\n\
             dim Hidden = Length;\n",
        );
        let declared = interface.declared_surface();
        let external = interface.external_surface();

        assert!(declared.is_input_port(&DeclName::expect_valid("input")));
        assert!(declared.is_static_explicit_export(&atom("Length2")));
        assert!(!declared.is_static_explicit_export(&atom("Hidden")));
        assert!(!declared.is_static_explicit_export(&atom("Speed")));
        assert!(!declared.is_explicit_export(&DeclName::expect_valid("shared")));
        assert!(external.is_static_explicit_export(&atom("Speed")));
        assert!(external.is_explicit_export(&DeclName::expect_valid("shared")));
    }

    #[test]
    fn external_surface_keeps_param_input_ports_separate_from_explicit_exports() {
        let interface = interface(
            "param input: Dimensionless = 1.0;\n\
             pub node output: Dimensionless = @input;\n\
             node helper: Dimensionless = @input;\n\
             pub const unit km2: Length = 1000.0 m;\n\
             pub type Shape { Circle }\n",
        );
        let surface = interface.external_surface();
        let input = DeclName::expect_valid("input");
        let output = DeclName::expect_valid("output");
        let helper = DeclName::expect_valid("helper");

        assert!(surface.is_input_port(&input));
        assert!(surface.can_select_output(&input));
        assert!(!surface.is_explicit_export(&input));
        assert!(surface.is_explicit_export(&output));
        assert!(!surface.is_input_port(&output));
        assert!(!surface.is_externally_nameable(&helper));
        assert!(surface.is_unit_explicit_export(&atom("km2")));
        assert!(surface.is_static_explicit_export(&atom("Shape")));
        // Constructors are importable items but not surface declarations.
        assert!(!surface.is_explicit_export(&DeclName::expect_valid("Circle")));

        let term = ImportItemNamespace::Term;
        assert_eq!(
            interface.exposure(&atom("input"), term),
            Some(DeclExposure::InputPort)
        );
        assert_eq!(
            interface.exposure(&atom("output"), term),
            Some(DeclExposure::ExplicitExport)
        );
        assert_eq!(
            interface.exposure(&atom("helper"), term),
            Some(DeclExposure::Private)
        );
        assert!(interface.exposes_item(&atom("input"), term));
        assert!(!interface.exposes_item(&atom("helper"), term));
        assert!(interface.has_item(&atom("helper"), term));
        assert!(interface.exposes_item(&atom("Circle"), term));
    }

    #[test]
    fn value_outputs_and_required_params_follow_source_order() {
        let interface = interface(
            "param a: Dimensionless;\n\
             pub const node c: Dimensionless = 1.0;\n\
             const node hidden: Dimensionless = 1.0;\n\
             param b: Dimensionless = 2.0;\n\
             pub node n: Dimensionless = @a;\n\
             node private_node: Dimensionless = @a;\n\
             param z: Dimensionless;\n",
        );
        assert_eq!(
            interface
                .value_outputs()
                .iter()
                .map(|output| (output.name().to_string(), output.is_const()))
                .collect::<Vec<_>>(),
            vec![
                ("a".to_string(), false),
                ("c".to_string(), true),
                ("b".to_string(), false),
                ("n".to_string(), false),
                ("z".to_string(), false),
            ]
        );
        assert_eq!(
            interface
                .required_params()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["a".to_string(), "z".to_string()]
        );
        assert!(interface.explicitly_exports(&atom("c"), IntroducedKind::ConstNode));
        assert!(!interface.explicitly_exports(&atom("hidden"), IntroducedKind::ConstNode));
        assert!(!interface.explicitly_exports(&atom("c"), IntroducedKind::Node));
        assert!(interface.declares(&atom("hidden"), IntroducedKind::ConstNode));
        assert!(!interface.declares(&atom("hidden"), IntroducedKind::Node));
        assert_eq!(
            interface
                .declared_kinds(&atom("n"), ImportItemNamespace::Term)
                .collect::<Vec<_>>(),
            vec![IntroducedKind::Node]
        );
    }

    #[test]
    fn static_roles_and_runtime_units_are_indexed_once() {
        let interface = interface(
            "pub(bind) type Element;\n\
             pub(bind) dim Basis = Length;\n\
             index Axis = { X, Y };\n\
             pub(bind) index Port;\n\
             base dim Money;\n\
             pub unit EUR: Money = 1.0 USD;\n\
             pub base unit USD: Money;\n\
             unit hidden_rate: Money = 2.0 USD;\n",
        );
        assert_eq!(
            interface.static_interface(StaticInputKind::Type, &atom("Element")),
            Some(StaticInterface::new(
                StaticInputKind::Type,
                StaticRole::RequiredInput
            ))
        );
        assert_eq!(
            interface.static_interface(StaticInputKind::Dimension, &atom("Basis")),
            Some(StaticInterface::new(
                StaticInputKind::Dimension,
                StaticRole::OptionalInput
            ))
        );
        assert_eq!(
            interface.static_interface(StaticInputKind::Dimension, &atom("Money")),
            Some(StaticInterface::new(
                StaticInputKind::Dimension,
                StaticRole::Fixed
            ))
        );
        assert_eq!(
            interface.static_interface(StaticInputKind::Index, &atom("Element")),
            None
        );
        assert_eq!(
            interface
                .static_declarations()
                .map(|(kind, name, role)| (kind, name.to_string(), role))
                .collect::<Vec<_>>(),
            vec![
                (
                    StaticInputKind::Type,
                    "Element".to_string(),
                    StaticRole::RequiredInput
                ),
                (
                    StaticInputKind::Dimension,
                    "Basis".to_string(),
                    StaticRole::OptionalInput
                ),
                (
                    StaticInputKind::Index,
                    "Axis".to_string(),
                    StaticRole::Fixed
                ),
                (
                    StaticInputKind::Index,
                    "Port".to_string(),
                    StaticRole::RequiredInput
                ),
                (
                    StaticInputKind::Dimension,
                    "Money".to_string(),
                    StaticRole::Fixed
                ),
            ]
        );
        assert_eq!(
            interface.runtime_units(),
            &HashSet::from([UnitName::classify(atom("EUR"))])
        );
    }

    #[test]
    fn first_static_declaration_of_a_name_wins() {
        let interface = interface("pub(bind) dim D;\ndim D = Length;\n");
        assert_eq!(
            interface.static_interface(StaticInputKind::Dimension, &atom("D")),
            Some(StaticInterface::new(
                StaticInputKind::Dimension,
                StaticRole::RequiredInput
            ))
        );
        assert_eq!(interface.static_declarations().count(), 1);
    }
}

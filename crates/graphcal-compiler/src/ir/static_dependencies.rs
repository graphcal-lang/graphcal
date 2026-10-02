//! Pure transitive Static dependency analysis for external declaration surfaces.
//!
//! A declaration's Static dependencies are the dimensions, types, and indexes
//! its semantic signature names. Each name is resolved by the
//! [`ModuleResolver`] in the declaring module's scope, so a bare type-position
//! name that the parser cannot classify (`x: Foo` may name a type or a
//! dimension) has exactly one resolved target: a module's Static slot holds at
//! most one symbol per leaf.

use std::collections::{HashMap, HashSet};

use crate::dag_id::DagId;
use crate::desugar::desugared_ast::{DeclKind, Declaration, IndexExpr, TypeExpr, TypeExprKind};
use crate::resolve::ModuleResolver;
use crate::resolved_name::ResolvedStaticName;
use crate::static_interface::{
    StaticImportRejections, StaticInputKind, StaticRole, static_interface,
};
use crate::syntax::ast::{GenericArg, ImportItemNamespace};
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::phase::never;

/// The module scope in which a declaration's Static references resolve.
#[derive(Debug, Clone, Copy)]
pub struct StaticScope<'a> {
    owner: &'a DagId,
    resolver: &'a ModuleResolver,
}

impl<'a> StaticScope<'a> {
    /// Resolve references of declarations owned by `owner`.
    #[must_use]
    pub const fn new(owner: &'a DagId, resolver: &'a ModuleResolver) -> Self {
        Self { owner, resolver }
    }

    /// The module whose declarations are analyzed.
    #[must_use]
    pub const fn owner(&self) -> &'a DagId {
        self.owner
    }

    /// The resolver references of the module resolve through.
    #[must_use]
    pub const fn resolver(&self) -> &'a ModuleResolver {
        self.resolver
    }
}

/// A module's declarations paired with the scope their references resolve in.
#[derive(Debug, Clone, Copy)]
pub struct ModuleDeclarations<'a> {
    declarations: &'a [Declaration],
    scope: StaticScope<'a>,
}

impl<'a> ModuleDeclarations<'a> {
    /// Pair `declarations` with the module scope that declares them.
    #[must_use]
    pub const fn new(declarations: &'a [Declaration], scope: StaticScope<'a>) -> Self {
        Self {
            declarations,
            scope,
        }
    }

    /// The declarations in source order.
    #[must_use]
    pub const fn declarations(&self) -> &'a [Declaration] {
        self.declarations
    }

    /// The scope the declarations resolve in.
    #[must_use]
    pub const fn scope(&self) -> StaticScope<'a> {
        self.scope
    }
}

/// One Static name referenced by a declaration signature, with its source
/// spelling and its resolved target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticReference {
    path: NamePath,
    target: ResolvedStaticName,
}

impl StaticReference {
    /// The source spelling of the reference.
    #[must_use]
    pub const fn path(&self) -> &NamePath {
        &self.path
    }

    /// The resolved Static symbol.
    #[must_use]
    pub const fn target(&self) -> &ResolvedStaticName {
        &self.target
    }

    /// The Static input kind of the resolved symbol.
    #[must_use]
    pub const fn kind(&self) -> StaticInputKind {
        static_input_kind(&self.target)
    }
}

/// The Static input kind of a resolved Static symbol.
#[must_use]
pub const fn static_input_kind(target: &ResolvedStaticName) -> StaticInputKind {
    match target {
        ResolvedStaticName::Dimension(_) => StaticInputKind::Dimension,
        ResolvedStaticName::Type(_) => StaticInputKind::Type,
        ResolvedStaticName::Index(_) => StaticInputKind::Index,
    }
}

/// Which Static symbols a syntactic reference position admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaticPosition {
    /// A multi-term dimension expression or a dimension-only position.
    Dimension,
    /// A type-application head.
    Type,
    /// An index position in an indexed type.
    Index,
    /// A single-name type annotation: a type or a dimension.
    TypeOrDimension,
    /// A bare generic argument: an index, a type, or a dimension.
    Any,
}

impl StaticPosition {
    const fn admits(self, target: &ResolvedStaticName) -> bool {
        matches!(
            (self, target),
            (Self::Any, _)
                | (
                    Self::Dimension | Self::TypeOrDimension,
                    ResolvedStaticName::Dimension(_)
                )
                | (
                    Self::Type | Self::TypeOrDimension,
                    ResolvedStaticName::Type(_)
                )
                | (Self::Index, ResolvedStaticName::Index(_))
        )
    }
}

/// Collects resolved references while walking one declaration signature.
struct Collector<'a> {
    scope: StaticScope<'a>,
    /// Generic parameters of the enclosing `type` declaration; they are
    /// lexical binders, not module Static symbols.
    generic_parameters: HashSet<NameAtom>,
    references: Vec<StaticReference>,
}

impl Collector<'_> {
    fn path(&mut self, path: &NamePath, position: StaticPosition) {
        if path
            .as_bare()
            .is_some_and(|name| self.generic_parameters.contains(name))
        {
            return;
        }
        // An unresolvable name is diagnosed where the signature is lowered;
        // it contributes no dependency here.
        if let Ok(target) = self
            .scope
            .resolver
            .resolve_static_path(self.scope.owner, path)
            && position.admits(&target)
        {
            self.references.push(StaticReference {
                path: path.clone(),
                target,
            });
        }
    }

    fn type_expr(&mut self, type_expr: &TypeExpr) {
        match &type_expr.element.kind {
            TypeExprKind::DimExpr(dim_expr) => match dim_expr.terms.as_slice() {
                [item] if item.term.power.is_none() => {
                    self.path(&item.term.name.value, StaticPosition::TypeOrDimension);
                }
                terms => {
                    for item in terms {
                        self.path(&item.term.name.value, StaticPosition::Dimension);
                    }
                }
            },
            TypeExprKind::TypeApplication { name, generic_args } => {
                self.path(&name.value, StaticPosition::Type);
                for argument in generic_args {
                    self.generic_arg(argument);
                }
            }
            TypeExprKind::ComplexApplication { generic_args }
            | TypeExprKind::KeyApplication { generic_args } => {
                for argument in generic_args {
                    self.generic_arg(argument);
                }
            }
            TypeExprKind::DatetimeApplication { type_args } => {
                for argument in type_args {
                    self.type_expr(argument);
                }
            }
            TypeExprKind::IndexLabel { .. }
            | TypeExprKind::Dimensionless
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime => {}
        }
        for index in type_expr.indexes.iter().flatten() {
            if let IndexExpr::Name(path) = index {
                self.path(&path.value, StaticPosition::Index);
            }
        }
    }

    fn generic_arg(&mut self, argument: &GenericArg<crate::syntax::phase::Desugared>) {
        match argument {
            GenericArg::Type(type_expr) => self.type_expr(type_expr),
            GenericArg::Index(IndexExpr::Name(path)) => {
                self.path(&path.value, StaticPosition::Index);
            }
            GenericArg::Index(IndexExpr::Finite { .. } | IndexExpr::BareNat(_))
            | GenericArg::Nat(_) => {}
            GenericArg::Ambiguous(ambiguous) => self.ambiguous_generic_arg(ambiguous),
        }
    }

    fn ambiguous_generic_arg(
        &mut self,
        argument: &crate::desugar::desugared_ast::AmbiguousGenericArg,
    ) {
        match argument {
            crate::desugar::desugared_ast::AmbiguousGenericArg::Name(identifier) => {
                self.path(
                    &NamePath::local(identifier.name.atom().clone()),
                    StaticPosition::Any,
                );
            }
            crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
                for operand in operands {
                    self.ambiguous_generic_arg(operand);
                }
            }
        }
    }

    fn dim_expr(&mut self, dim_expr: &crate::desugar::desugared_ast::DimExpr) {
        for item in &dim_expr.terms {
            self.path(&item.term.name.value, StaticPosition::Dimension);
        }
    }

    fn declaration(&mut self, kind: &DeclKind) {
        match kind {
            DeclKind::Param(param) => self.type_expr(&param.type_ann),
            DeclKind::Node(node) => self.type_expr(&node.type_ann),
            DeclKind::ConstNode(constant) => self.type_expr(&constant.type_ann),
            DeclKind::Unit(unit) => self.dim_expr(&unit.dim_type),
            DeclKind::Dimension(dimension) => {
                if let Some(definition) = &dimension.definition {
                    self.dim_expr(definition);
                }
            }
            DeclKind::Type(type_decl) => {
                let enclosing = std::mem::replace(
                    &mut self.generic_parameters,
                    type_decl
                        .generic_params
                        .iter()
                        .map(|parameter| parameter.name.value.atom().clone())
                        .collect(),
                );
                if let crate::desugar::desugared_ast::TypeDeclBody::Constructors(members) =
                    &type_decl.body
                {
                    for member in members {
                        for field in member.payload.iter().flatten() {
                            self.type_expr(&field.type_ann);
                        }
                    }
                }
                for default in type_decl
                    .generic_params
                    .iter()
                    .filter_map(|parameter| parameter.default.as_ref())
                {
                    self.generic_arg(default);
                }
                self.generic_parameters = enclosing;
            }
            DeclKind::Index(index) => {
                if let crate::desugar::desugared_ast::IndexDeclKind::RequiredCoordinate {
                    dimension,
                } = &index.kind
                {
                    self.dim_expr(dimension);
                }
            }
            DeclKind::Dag(dag) => {
                // An inline DAG body resolves in its own module scope.
                let owner = self.scope.owner.inline_dag_child(dag.name.value.clone());
                let mut body = Collector {
                    scope: StaticScope::new(&owner, self.scope.resolver),
                    generic_parameters: HashSet::new(),
                    references: Vec::new(),
                };
                for declaration in &dag.body {
                    body.declaration(&declaration.kind);
                }
                self.references.extend(body.references);
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_) => {}
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(s) => never(*s),
        }
    }
}

/// Collect the resolved Static references of one declaration's semantic
/// signature, in source order.
#[must_use]
pub fn declaration_static_references(
    kind: &DeclKind,
    scope: StaticScope<'_>,
) -> Vec<StaticReference> {
    let mut collector = Collector {
        scope,
        generic_parameters: HashSet::new(),
        references: Vec::new(),
    };
    collector.declaration(kind);
    collector.references
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StaticDeclarationKey {
    kind: StaticInputKind,
    name: NameAtom,
}

impl StaticDeclarationKey {
    const fn new(kind: StaticInputKind, name: NameAtom) -> Self {
        Self { kind, name }
    }
}

/// First unresolved required Static dependency reached from an import target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredStaticDependency {
    kind: StaticInputKind,
    name: NameAtom,
}

impl RequiredStaticDependency {
    #[must_use]
    pub const fn kind(&self) -> StaticInputKind {
        self.kind
    }

    #[must_use]
    pub const fn name(&self) -> &NameAtom {
        &self.name
    }
}

fn declaration_name_in_namespace(
    declaration: &Declaration,
    name: &NameAtom,
    namespace: ImportItemNamespace,
) -> bool {
    declaration
        .kind
        .introduced_names()
        .any(|introduced| introduced.namespace() == namespace && introduced.atom() == name)
}

/// Key and role of one declaration that has a typed Static interface.
fn static_declaration(declaration: &Declaration) -> Option<(StaticDeclarationKey, StaticRole)> {
    let interface = static_interface(&declaration.kind)?;
    let introduced = declaration.kind.declared_name()?;
    Some((
        StaticDeclarationKey::new(interface.kind(), introduced.atom().clone()),
        interface.role(),
    ))
}

/// The module-local Static declaration a resolved reference names, if any.
///
/// References that resolve into another module are external semantic
/// identities and are outside this module-local closure.
fn local_candidate(
    reference: &StaticReference,
    scope: StaticScope<'_>,
    interfaces: &HashMap<StaticDeclarationKey, StaticRole>,
) -> Option<StaticDeclarationKey> {
    (reference.target().owner() == scope.owner())
        .then(|| StaticDeclarationKey::new(reference.kind(), reference.target().atom().clone()))
        .filter(|candidate| interfaces.contains_key(candidate))
}

/// Typed reason a declaration cannot cross a blueprint-only import boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticImportRejection {
    RequiredInput {
        kind: StaticInputKind,
        name: NameAtom,
    },
    UnresolvedDependency(RequiredStaticDependency),
}

/// Classify Static-input reasons that reject one direct or qualified import.
#[must_use]
pub fn static_import_rejection(
    module: ModuleDeclarations<'_>,
    selected_name: &NameAtom,
    selected_namespace: ImportItemNamespace,
) -> Option<StaticImportRejection> {
    if let Some(interface) = module.declarations.iter().find_map(|declaration| {
        if !declaration_name_in_namespace(declaration, selected_name, selected_namespace) {
            return None;
        }
        static_interface(&declaration.kind)
    }) && interface.role() == StaticRole::RequiredInput
    {
        return Some(StaticImportRejection::RequiredInput {
            kind: interface.kind(),
            name: selected_name.clone(),
        });
    }
    first_required_static_dependency(module, selected_name, selected_namespace)
        .map(StaticImportRejection::UnresolvedDependency)
}

/// Classify every name `module` declares for pure-import capability.
#[must_use]
pub fn static_import_rejections(module: ModuleDeclarations<'_>) -> StaticImportRejections {
    StaticImportRejections::new(
        module
            .declarations
            .iter()
            .flat_map(|declaration| declaration.kind.introduced_names())
            .filter(|introduced| {
                static_import_rejection(module, introduced.atom(), introduced.namespace()).is_some()
            })
            .map(|introduced| (introduced.namespace(), introduced.atom().clone())),
    )
}

fn visit_references(
    references: impl IntoIterator<Item = StaticReference>,
    scope: StaticScope<'_>,
    interfaces: &HashMap<StaticDeclarationKey, StaticRole>,
    declarations: &HashMap<StaticDeclarationKey, &Declaration>,
    visited: &mut HashSet<StaticDeclarationKey>,
) -> Option<RequiredStaticDependency> {
    for reference in references {
        if let Some(candidate) = local_candidate(&reference, scope, interfaces) {
            if !visited.insert(candidate.clone()) {
                continue;
            }
            let role = interfaces[&candidate];
            if role == StaticRole::RequiredInput {
                return Some(RequiredStaticDependency {
                    kind: candidate.kind,
                    name: candidate.name,
                });
            }
            if let Some(declaration) = declarations.get(&candidate)
                && let Some(required) = visit_references(
                    declaration_static_references(&declaration.kind, scope),
                    scope,
                    interfaces,
                    declarations,
                    visited,
                )
            {
                return Some(required);
            }
        }
    }
    None
}

/// Find the first transitive required Static input in one selected declaration.
///
/// Every reference is resolved in `scope`, so a syntactically ambiguous bare
/// name visits exactly the Static declaration it denotes. References into
/// other modules are external semantic identities and are outside this
/// module-local closure.
#[must_use]
pub fn first_required_static_dependency(
    module: ModuleDeclarations<'_>,
    selected_name: &NameAtom,
    selected_namespace: ImportItemNamespace,
) -> Option<RequiredStaticDependency> {
    let ModuleDeclarations {
        declarations,
        scope,
    } = module;
    let interfaces = declarations
        .iter()
        .filter_map(static_declaration)
        .collect::<HashMap<_, _>>();
    let declaration_by_key = declarations
        .iter()
        .filter_map(|declaration| Some((static_declaration(declaration)?.0, declaration)))
        .collect::<HashMap<_, _>>();
    let selected = declarations.iter().find(|declaration| {
        declaration_name_in_namespace(declaration, selected_name, selected_namespace)
    })?;

    visit_references(
        declaration_static_references(&selected.kind, scope),
        scope,
        &interfaces,
        &declaration_by_key,
        &mut HashSet::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> crate::desugar::desugared_ast::File {
        let file = Parser::new(source).parse_file().expect("source parses");
        crate::desugar::desugared_ast::File::from(file)
    }

    fn owner() -> DagId {
        DagId::from_virtual_relative_path(std::path::Path::new("test.gcl")).unwrap()
    }

    fn resolver(file: &crate::desugar::desugared_ast::File) -> ModuleResolver {
        let mut tables = crate::resolve::builder::SymbolTables::default();
        tables.add_file(owner(), &file.declarations).unwrap();
        tables
            .scopes(&crate::resolve::builder::NoModuleTargets)
            .unwrap()
            .freeze()
            .unwrap()
    }

    fn first_required(
        source: &str,
        name: &str,
        namespace: ImportItemNamespace,
    ) -> Option<RequiredStaticDependency> {
        let file = parse(source);
        let resolver = resolver(&file);
        let owner = owner();
        first_required_static_dependency(
            ModuleDeclarations::new(&file.declarations, StaticScope::new(&owner, &resolver)),
            &NameAtom::parse(name).unwrap(),
            namespace,
        )
    }

    #[test]
    fn finds_required_inputs_through_transitive_static_signatures() {
        let required = first_required(
            "pub(bind) type Element;\n\
             pub type Box { Box(value: Element) }\n\
             pub type Wrapper { Wrapper(value: Box) }",
            "Wrapper",
            ImportItemNamespace::Type,
        )
        .expect("required dependency");
        assert_eq!(required.kind(), StaticInputKind::Type);
        assert_eq!(required.name().as_str(), "Element");
    }

    #[test]
    fn finds_required_dimensions_and_indexes_by_exact_category() {
        let required_dimension = first_required(
            "pub(bind) dim Basis;\n\
             pub dim Derived = Basis / Time;",
            "Derived",
            ImportItemNamespace::Dimension,
        )
        .expect("required dimension dependency");
        assert_eq!(required_dimension.kind(), StaticInputKind::Dimension);
        assert_eq!(required_dimension.name().as_str(), "Basis");

        let required_index = first_required(
            "pub(bind) index Axis;\n\
             pub type Samples { Samples(value: Dimensionless[Axis]) }",
            "Samples",
            ImportItemNamespace::Type,
        )
        .expect("required index dependency");
        assert_eq!(required_index.kind(), StaticInputKind::Index);
        assert_eq!(required_index.name().as_str(), "Axis");
    }

    #[test]
    fn ambiguous_type_position_names_resolve_to_their_static_symbol() {
        let required = first_required(
            "pub(bind) dim Basis;\n\
             pub type Reading { Reading(value: Basis) }",
            "Reading",
            ImportItemNamespace::Type,
        )
        .expect("required dimension dependency");
        assert_eq!(required.kind(), StaticInputKind::Dimension);
        assert_eq!(required.name().as_str(), "Basis");
    }

    #[test]
    fn generic_parameters_shadow_module_static_symbols() {
        assert!(
            first_required(
                "pub(bind) dim T;\n\
                 pub type Holder<T: Dim> { Holder(value: T) }",
                "Holder",
                ImportItemNamespace::Type,
            )
            .is_none()
        );
    }

    #[test]
    fn optional_inputs_are_closed_through_their_defaults() {
        assert!(
            first_required(
                "pub(bind) type Element { Element }\n\
                 pub type Box { Box(value: Element) }",
                "Box",
                ImportItemNamespace::Type,
            )
            .is_none()
        );
    }
}

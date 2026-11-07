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
use crate::ir::static_substitution::StaticSubstitution;
use crate::resolve::ModuleResolver;
use crate::resolved_name::{ResolvedStaticName, ResolvedStructTypeName};
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
                    if let Some(path) = item.term.name.as_path() {
                        self.path(&path.value, StaticPosition::TypeOrDimension);
                    }
                }
                _ => self.dim_expr(dim_expr),
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
        for path in dim_expr
            .terms
            .iter()
            .filter_map(|item| item.term.name.as_path())
        {
            self.path(&path.value, StaticPosition::Dimension);
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

/// Collect the resolved Static references of one type annotation, in source
/// order.
#[must_use]
pub fn type_expr_static_references(
    type_expr: &TypeExpr,
    scope: StaticScope<'_>,
) -> Vec<StaticReference> {
    let mut collector = Collector {
        scope,
        generic_parameters: HashSet::new(),
        references: Vec::new(),
    };
    collector.type_expr(type_expr);
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

/// One item a selective include exposes to its importer, as the template
/// declares it.
#[derive(Debug, Clone, Copy)]
pub enum IncludeSurfaceItem<'a> {
    /// A selected value, through its type annotation.
    Value(&'a TypeExpr),
    /// A selected Static declaration, through its whole definition.
    Static(&'a DeclKind),
}

/// A template nominal type that a selective include exposes without
/// specializing it, although its definition depends on a port the include
/// rebinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnspecializedReboundType<Item> {
    nominal: ResolvedStructTypeName,
    port: ResolvedStaticName,
    item: Item,
}

impl<Item> UnspecializedReboundType<Item> {
    /// The template's nominal type.
    #[must_use]
    pub const fn nominal(&self) -> &ResolvedStructTypeName {
        &self.nominal
    }

    /// One rebound port the type's definition depends on.
    #[must_use]
    pub const fn port(&self) -> &ResolvedStaticName {
        &self.port
    }

    /// The first exposed item that reaches the type.
    #[must_use]
    pub const fn item(&self) -> &Item {
        &self.item
    }
}

/// Every template nominal type that `surface` reaches but the include does
/// not select, whose definition depends (transitively) on a port the include
/// rebinds, in the order the surface reaches them.
///
/// An unselected type keeps the template's identity and definition in the
/// importer, while the instance's values follow the rebound ports. Such a
/// type would let the importer read an instance value under the template's
/// definition (an index of the wrong length, or a dimension of the default),
/// so the include must select it to obtain its specialized projection.
///
/// `rebound` is the include's own Static bindings, keyed by the template's
/// ports. A rebound port itself needs no selection: it is substituted
/// wherever it appears. `selected` are the template types the include
/// projects; their definitions belong to `surface` too, because a projection
/// specializes only the ports and the selected types its definition names.
#[must_use]
pub fn unspecialized_rebound_types<'a, Item: Clone>(
    template: ModuleDeclarations<'_>,
    surface: impl IntoIterator<Item = (Item, IncludeSurfaceItem<'a>)>,
    rebound: &StaticSubstitution,
    selected: &[ResolvedStructTypeName],
) -> Vec<UnspecializedReboundType<Item>> {
    let mut closure = ReboundClosure {
        scope: template.scope,
        rebound,
        declarations: template
            .declarations
            .iter()
            .filter_map(|declaration| Some((static_declaration(declaration)?.0, declaration)))
            .collect(),
        dependencies: HashMap::new(),
    };
    let mut reached = HashSet::new();
    let mut found = Vec::new();
    for (item, exposed) in surface {
        let mut pending = std::collections::VecDeque::from(match exposed {
            IncludeSurfaceItem::Value(type_expr) => {
                type_expr_static_references(type_expr, template.scope)
            }
            IncludeSurfaceItem::Static(declaration) => {
                declaration_static_references(declaration, template.scope)
            }
        });
        while let Some(reference) = pending.pop_front() {
            let Some(key) = closure.local_key(&reference) else {
                continue;
            };
            if !reached.insert(key.clone()) {
                continue;
            }
            if let ResolvedStaticName::Type(nominal) = reference.target()
                && !selected.contains(nominal)
                && let Some(port) = closure.port_dependency(&key)
            {
                found.push(UnspecializedReboundType {
                    nominal: nominal.clone(),
                    port,
                    item: item.clone(),
                });
            }
            if let Some(declaration) = closure.declarations.get(&key) {
                pending.extend(declaration_static_references(
                    &declaration.kind,
                    template.scope,
                ));
            }
        }
    }
    found
}

/// Memoized dependency of template declarations on rebound ports.
struct ReboundClosure<'a> {
    scope: StaticScope<'a>,
    rebound: &'a StaticSubstitution,
    declarations: HashMap<StaticDeclarationKey, &'a Declaration>,
    /// The first rebound port each visited declaration depends on. A
    /// declaration still being visited reads as independent, which only
    /// matters for a definition cycle, and those are rejected on their own.
    dependencies: HashMap<StaticDeclarationKey, Option<ResolvedStaticName>>,
}

impl ReboundClosure<'_> {
    fn is_rebound(&self, target: &ResolvedStaticName) -> bool {
        match target {
            ResolvedStaticName::Dimension(name) => self.rebound.dimensions.contains_key(name),
            ResolvedStaticName::Type(name) => self.rebound.types.contains_key(name),
            ResolvedStaticName::Index(name) => self.rebound.indexes.contains_key(name),
        }
    }

    /// The template declaration a reference names, unless it names another
    /// module or a rebound port.
    fn local_key(&self, reference: &StaticReference) -> Option<StaticDeclarationKey> {
        (reference.target().owner() == self.scope.owner() && !self.is_rebound(reference.target()))
            .then(|| StaticDeclarationKey::new(reference.kind(), reference.target().atom().clone()))
    }

    fn port_dependency(&mut self, key: &StaticDeclarationKey) -> Option<ResolvedStaticName> {
        if let Some(dependency) = self.dependencies.get(key) {
            return dependency.clone();
        }
        self.dependencies.insert(key.clone(), None);
        // A template symbol with no source declaration (a type the template's
        // own include projects) has no definition here to depend on a port.
        let declaration = self.declarations.get(key)?;
        let references = declaration_static_references(&declaration.kind, self.scope);
        let dependency = references.iter().find_map(|reference| {
            if reference.target().owner() != self.scope.owner() {
                None
            } else if self.is_rebound(reference.target()) {
                Some(reference.target().clone())
            } else {
                let key = self.local_key(reference)?;
                self.port_dependency(&key)
            }
        });
        self.dependencies.insert(key.clone(), dependency.clone());
        dependency
    }
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

    /// The template types the selected values reach that depend on one of
    /// `ports`, given the `selected` types, as `(type, port, item)`.
    fn unspecialized(
        source: &str,
        ports: &[&str],
        values: &[&str],
        selected: &[&str],
    ) -> Vec<(String, String, String)> {
        let file = parse(source);
        let resolver = resolver(&file);
        let owner = owner();
        let resolve = |name: &str| {
            resolver
                .resolve_static_path(&owner, &NamePath::local(NameAtom::parse(name).unwrap()))
                .unwrap()
        };
        let mut rebound = StaticSubstitution::default();
        for port in ports {
            match resolve(port) {
                ResolvedStaticName::Dimension(name) => {
                    rebound.dimensions.insert(name.clone(), name);
                }
                ResolvedStaticName::Type(name) => {
                    rebound.types.insert(name.clone(), name);
                }
                ResolvedStaticName::Index(name) => {
                    rebound.indexes.insert(
                        name.clone(),
                        crate::ir::static_substitution::InstanceIndexBindingTarget::Declared(name),
                    );
                }
            }
        }
        let selected = selected
            .iter()
            .map(|name| match resolve(name) {
                ResolvedStaticName::Type(name) => name,
                other => panic!("`{name}` is not a type: {other:?}"),
            })
            .collect::<Vec<_>>();
        let surface = file
            .declarations
            .iter()
            .filter_map(|declaration| {
                let introduced = declaration.kind.declared_name()?;
                let type_expr = match &declaration.kind {
                    DeclKind::Node(node) => &node.type_ann,
                    DeclKind::Param(param) => &param.type_ann,
                    _ => return None,
                };
                values.contains(&introduced.atom().as_str()).then(|| {
                    (
                        introduced.atom().to_string(),
                        IncludeSurfaceItem::Value(type_expr),
                    )
                })
            })
            .collect::<Vec<_>>();
        unspecialized_rebound_types(
            ModuleDeclarations::new(&file.declarations, StaticScope::new(&owner, &resolver)),
            surface,
            &rebound,
            &selected,
        )
        .into_iter()
        .map(|found| {
            (
                found.nominal().atom().to_string(),
                found.port().atom().to_string(),
                found.item().clone(),
            )
        })
        .collect()
    }

    const TEMPLATE: &str = "pub(bind) index I = { a, b };\n\
         pub(bind) dim Q;\n\
         pub dim QR = Q / Time;\n\
         pub type Rows { Rows(x: Length[I]) }\n\
         pub type Rate { Rate(r: QR) }\n\
         pub type Both { Both(rows: Rows, rate: Rate) }\n\
         pub type Plain { Plain(x: Length) }\n\
         param rows: Rows;\n\
         param both: Both;\n\
         param plain: Plain;\n\
         param raw: Length[I];\n";

    #[test]
    fn rebound_ports_reach_types_through_fields_and_derived_dimensions() {
        let found = |item: &str, from: &str| (item.to_owned(), from.to_owned(), "both".to_owned());
        assert_eq!(
            unspecialized(TEMPLATE, &["I", "Q"], &["both"], &[]),
            vec![found("Both", "I"), found("Rows", "I"), found("Rate", "Q")]
        );
        assert_eq!(
            unspecialized(TEMPLATE, &["Q"], &["both"], &[]),
            vec![found("Both", "Q"), found("Rate", "Q")]
        );
    }

    #[test]
    fn selected_types_and_independent_types_are_not_reported() {
        assert_eq!(
            unspecialized(TEMPLATE, &["I"], &["both"], &["Both", "Rows"]),
            Vec::new()
        );
        assert_eq!(
            unspecialized(TEMPLATE, &["I", "Q"], &["plain", "raw"], &[]),
            Vec::new()
        );
        assert_eq!(unspecialized(TEMPLATE, &[], &["rows"], &[]), Vec::new());
    }

    #[test]
    fn each_type_is_reported_once_for_the_first_item_reaching_it() {
        assert_eq!(
            unspecialized(TEMPLATE, &["I"], &["rows", "both"], &[]),
            vec![
                ("Rows".to_owned(), "I".to_owned(), "rows".to_owned()),
                ("Both".to_owned(), "I".to_owned(), "both".to_owned()),
            ]
        );
    }
}

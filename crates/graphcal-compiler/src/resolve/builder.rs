//! The typestate that builds an immutable [`ModuleResolver`].
//!
//! 1. [`SymbolTables`] collects every source module (file root or inline
//!    `dag`): its own declarations, its declared aliases, and the check that
//!    no two source modules share a module-path spelling.
//! 2. [`SymbolTables::scopes`] connects each source module's `import` /
//!    `include` declarations to the modules they name (as decided by the
//!    loader through [`ModuleTargets`]) and expands every include into its
//!    concrete instance modules. Each module now has a scope source: a
//!    source module builds its own scope from its edges (`ScopeSource::Own`);
//!    an instance inherits its template's (`ScopeSource::InheritFrom`).
//! 3. [`ScopeBuilder::freeze`] computes every scope, dependencies first, and
//!    returns the immutable [`ModuleResolver`].
//!
//! Callers never order registrations: each stage consumes the previous one,
//! and the order in which scopes are completed is derived here from the
//! edges themselves.

use std::collections::HashMap;

use crate::dag_id::{DagId, DagPackageId};
use crate::dependency_graph::DependencyGraph;
use crate::desugar::desugared_ast as ast;
use crate::syntax::ast::ModulePath;
use crate::syntax::module_path_key::ModulePathKey;

use super::error::ModuleResolveError;
use super::module_table::{ModuleHandle, ModuleTable};
use super::scope::{ModuleScope, declare_aliases};
use super::symbols::ModuleSymbols;
use super::{ModuleEntry, ModuleResolver};

/// Loader decisions about which module each `import` / `include` path names.
///
/// The resolver never interprets module paths as files; the loader resolves
/// them and answers here. `None` means the path names no loaded module; such
/// a declaration introduces nothing into the owner's scope.
pub trait ModuleTargets {
    /// The module an `import` of `path` in `owner` names.
    fn import_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId>;

    /// The template module an `include` of `path` in `owner` instantiates.
    fn include_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId>;
}

/// No module path names a loaded module (a single buffer with no loader).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoModuleTargets;

impl ModuleTargets for NoModuleTargets {
    fn import_target(&self, _owner: &DagId, _path: &ModulePath) -> Option<DagId> {
        None
    }

    fn include_target(&self, _owner: &DagId, _path: &ModulePath) -> Option<DagId> {
        None
    }
}

/// Targets decided by one function for imports and includes alike.
impl<F> ModuleTargets for F
where
    F: Fn(&DagId, &ModulePath) -> Option<DagId>,
{
    fn import_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId> {
        self(owner, path)
    }

    fn include_target(&self, owner: &DagId, path: &ModulePath) -> Option<DagId> {
        self(owner, path)
    }
}

/// One source module's declarations and declared symbols.
#[derive(Debug)]
struct SourceModule<'a> {
    declarations: &'a [ast::Declaration],
    entry: ModuleEntry,
}

/// Module-path spelling of a source module within its package.
type PackageModulePath = (DagPackageId, ModulePathKey);

/// Stage 1: the declarations of every source module.
///
/// Each module takes its [`ModuleHandle`] when it is added; the resolver this
/// stage builds keeps that handle for it.
#[derive(Debug, Default)]
pub struct SymbolTables<'a> {
    modules: ModuleTable<SourceModule<'a>>,
    module_paths: HashMap<PackageModulePath, DagId>,
}

impl<'a> SymbolTables<'a> {
    /// Add one source module (a file root or an inline `dag`).
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateModule`] when `owner` was already
    /// added, [`ModuleResolveError::AmbiguousModulePath`] when another source
    /// module of the package has the same module-path spelling (a file
    /// submodule and an inline `dag` of one name), and the first
    /// symbol-collection error of the module's own declarations and aliases.
    /// The module is not added on error.
    ///
    /// The returned handle names the module in the resolver this stage
    /// builds.
    pub fn add_module(
        &mut self,
        owner: DagId,
        declarations: &'a [ast::Declaration],
    ) -> Result<ModuleHandle, ModuleResolveError> {
        let path_key = self.new_module_path(&owner)?;
        let (entry, errors) = collected_entry(owner.clone(), declarations);
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        Ok(self.insert(owner, path_key, declarations, entry))
    }

    /// Add one source module even when some of its declarations cannot be
    /// recorded, for editor tooling on incomplete code.
    ///
    /// Each duplicate declaration or alias is skipped and its error returned;
    /// the module keeps every other declaration.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::DuplicateModule`] or
    /// [`ModuleResolveError::AmbiguousModulePath`] as [`Self::add_module`]
    /// does, without adding the module.
    pub fn add_module_lenient(
        &mut self,
        owner: DagId,
        declarations: &'a [ast::Declaration],
    ) -> Result<Vec<ModuleResolveError>, ModuleResolveError> {
        let path_key = self.new_module_path(&owner)?;
        let (entry, errors) = collected_entry(owner.clone(), declarations);
        self.insert(owner, path_key, declarations, entry);
        Ok(errors)
    }

    /// Check that `owner` is new and its module path is unambiguous,
    /// returning the path key it will claim.
    fn new_module_path(
        &self,
        owner: &DagId,
    ) -> Result<Option<PackageModulePath>, ModuleResolveError> {
        if self.modules.contains_key(owner) {
            return Err(ModuleResolveError::DuplicateModule {
                owner: owner.clone(),
            });
        }
        let path_key = owner
            .module_path_key()
            .map(|key| (owner.package().clone(), key));
        if let Some(first) = path_key.as_ref().and_then(|key| self.module_paths.get(key)) {
            return Err(ModuleResolveError::AmbiguousModulePath {
                first: first.clone(),
                second: owner.clone(),
            });
        }
        Ok(path_key)
    }

    fn insert(
        &mut self,
        owner: DagId,
        path_key: Option<PackageModulePath>,
        declarations: &'a [ast::Declaration],
        entry: ModuleEntry,
    ) -> ModuleHandle {
        if let Some(key) = path_key {
            self.module_paths.insert(key, owner.clone());
        }
        self.modules.insert(
            owner,
            SourceModule {
                declarations,
                entry,
            },
        )
    }

    /// Add a file root and, in source preorder, every inline `dag` nested in
    /// it.
    ///
    /// # Errors
    ///
    /// Returns the first error of [`Self::add_module`].
    pub fn add_file(
        &mut self,
        root: DagId,
        declarations: &'a [ast::Declaration],
    ) -> Result<(), ModuleResolveError> {
        let mut pending = vec![(root, declarations)];
        while let Some((owner, declarations)) = pending.pop() {
            self.add_module(owner.clone(), declarations)?;
            pending.extend(declarations.iter().rev().filter_map(|declaration| {
                match &declaration.kind {
                    ast::DeclKind::Dag(dag) => Some((
                        owner.inline_dag_child(dag.name.value.clone()),
                        dag.body.as_slice(),
                    )),
                    _ => None,
                }
            }));
        }
        Ok(())
    }

    /// Connect every source module's edges to the modules `targets` names and
    /// expand each include into its concrete instance modules.
    ///
    /// An include instantiates its template, and every include of the
    /// template again inside the new instance, so the whole expansion is
    /// finite exactly when the template include graph is acyclic.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleResolveError::RecursiveIncludeExpansion`] for a cycle
    /// of includes, [`ModuleResolveError::UnknownModule`] when an include
    /// names a module that was not added, and
    /// [`ModuleResolveError::DuplicateModule`] when two includes of one module
    /// allocate the same instance.
    pub fn scopes(
        self,
        targets: &impl ModuleTargets,
    ) -> Result<ScopeBuilder<'a>, ModuleResolveError> {
        let Self { modules, .. } = self;
        let order = modules.owners().cloned().collect::<Vec<_>>();
        let declarations = modules
            .iter()
            .map(|(owner, module)| (owner.clone(), module.declarations))
            .collect::<HashMap<_, _>>();
        // Every source module keeps the handle it took when it was added.
        let entries = modules.map(|module| module.entry);
        let own_edges = order
            .iter()
            .cloned()
            .filter_map(|owner| {
                let edges = module_edges(&owner, declarations.get(&owner)?, targets);
                Some((owner, edges))
            })
            .collect::<Vec<_>>();
        reject_recursive_includes(&order, &own_edges)?;
        let mut resolver = ModuleResolver { modules: entries };
        let mut instances = Vec::new();
        {
            let edges = own_edges
                .iter()
                .map(|(owner, edges)| (owner, edges.as_slice()))
                .collect::<HashMap<_, _>>();
            for (root, _) in &own_edges {
                expand_instances(root, &edges, &declarations, &mut resolver, &mut instances)?;
            }
        }
        let sources = own_edges
            .into_iter()
            .map(|(owner, edges)| (owner, ScopeSource::Own(edges)))
            .chain(
                instances
                    .into_iter()
                    .map(|(instance, template)| (instance, ScopeSource::InheritFrom(template))),
            )
            .collect();
        Ok(ScopeBuilder { resolver, sources })
    }
}

/// A module's declared symbols and the aliases its declarations claim, with
/// the error of each declaration that could not be recorded.
fn collected_entry(
    owner: DagId,
    declarations: &[ast::Declaration],
) -> (ModuleEntry, Vec<ModuleResolveError>) {
    let (symbols, mut errors) = ModuleSymbols::collect(owner, declarations);
    let mut scope = ModuleScope::default();
    errors.extend(declare_aliases(&mut scope, &symbols, declarations));
    (ModuleEntry { symbols, scope }, errors)
}

/// One `import` / `include` declaration of a source module, with the module
/// the loader resolved its path to.
#[derive(Debug, Clone)]
enum Edge<'a> {
    /// An `import` whose path names a loaded module. An unresolved import
    /// introduces nothing.
    Import {
        import: &'a ast::ImportDecl,
        target: DagId,
    },
    /// An `include` whose path names a loaded template module. An unresolved
    /// include introduces nothing either.
    Include {
        include: &'a ast::IncludeDecl,
        template: DagId,
    },
}

impl Edge<'_> {
    /// The source module whose completed scope registering this edge reads.
    const fn dependency(&self) -> &DagId {
        match self {
            Self::Import { target, .. } => target,
            Self::Include { template, .. } => template,
        }
    }

    /// The include's declaration and template, when this edge is an include.
    const fn instantiation(&self) -> Option<(&ast::IncludeDecl, &DagId)> {
        match self {
            Self::Include { include, template } => Some((include, template)),
            Self::Import { .. } => None,
        }
    }
}

/// The edges of one source module, in declaration order.
fn module_edges<'a>(
    owner: &DagId,
    declarations: &'a [ast::Declaration],
    targets: &impl ModuleTargets,
) -> Vec<Edge<'a>> {
    declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            ast::DeclKind::Import(import) => targets
                .import_target(owner, import.path())
                .map(|target| Edge::Import { import, target }),
            ast::DeclKind::Include(include) => targets
                .include_target(owner, &include.path)
                .map(|template| Edge::Include { include, template }),
            _ => None,
        })
        .collect()
}

/// Reject a cycle of includes among the source modules, whose expansion
/// would never end.
///
/// The search visits source modules in `order` and each module's includes in
/// declaration order, so it reports the cycle that expanding the modules in
/// that order would enter first.
fn reject_recursive_includes(
    order: &[DagId],
    own_edges: &[(DagId, Vec<Edge<'_>>)],
) -> Result<(), ModuleResolveError> {
    let mut includes = DependencyGraph::new();
    for owner in order {
        includes.add_node(owner.clone());
    }
    for (owner, edges) in own_edges {
        for (_, template) in edges.iter().filter_map(Edge::instantiation) {
            includes.add_dependency(owner.clone(), template.clone());
        }
    }
    includes
        .into_depth_first_order()
        .map(drop)
        .map_err(|cycle| ModuleResolveError::RecursiveIncludeExpansion { cycle })
}

/// One template being instantiated while expanding includes.
struct Expansion {
    /// The module the template's includes allocate instances in.
    base: DagId,
    template: DagId,
    /// Index of the next instantiating edge of `template` to expand.
    next: usize,
}

/// Add the instance modules of every include reachable from the source module
/// `root`, recording each with its template.
///
/// The include graph is acyclic ([`reject_recursive_includes`]), so the
/// expansion is finite. The walk is iterative, so deep include chains cannot
/// overflow the stack.
fn expand_instances(
    root: &DagId,
    edges: &HashMap<&DagId, &[Edge<'_>]>,
    declarations: &HashMap<DagId, &[ast::Declaration]>,
    resolver: &mut ModuleResolver,
    instances: &mut Vec<(DagId, DagId)>,
) -> Result<(), ModuleResolveError> {
    let mut open = vec![Expansion {
        base: root.clone(),
        template: root.clone(),
        next: 0,
    }];
    while let Some(expansion) = open.last_mut() {
        let Some((include, template)) = edges.get(&expansion.template).and_then(|edges| {
            edges
                .iter()
                .filter_map(Edge::instantiation)
                .nth(expansion.next)
        }) else {
            open.pop();
            continue;
        };
        expansion.next += 1;
        let instance = expansion.base.instance_child(include.instance_scope());
        let template_declarations =
            declarations
                .get(template)
                .ok_or_else(|| ModuleResolveError::UnknownModule {
                    owner: template.clone(),
                })?;
        if resolver.modules.contains_key(&instance) {
            return Err(ModuleResolveError::DuplicateModule { owner: instance });
        }
        let (entry, errors) = collected_entry(instance.clone(), template_declarations);
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        resolver.modules.insert(instance.clone(), entry);
        instances.push((instance.clone(), template.clone()));
        open.push(Expansion {
            base: instance,
            template: template.clone(),
            next: 0,
        });
    }
    Ok(())
}

/// How one module's scope is obtained.
#[derive(Debug)]
enum ScopeSource<'a> {
    /// A source module builds its scope from its own `import` / `include`
    /// edges, in declaration order.
    Own(Vec<Edge<'a>>),
    /// A concrete include instance has exactly its template's scope.
    InheritFrom(DagId),
}

/// Stage 2: every module (source and instance) with the source of its scope.
#[derive(Debug)]
pub struct ScopeBuilder<'a> {
    resolver: ModuleResolver,
    sources: Vec<(DagId, ScopeSource<'a>)>,
}

impl ScopeBuilder<'_> {
    /// Complete every scope and return the immutable resolver.
    ///
    /// A source module's edges are registered once the modules they read are
    /// complete: an imported module (its public re-exports are selectable)
    /// and an included template (its instance inherits its scope). Modules
    /// that depend on each other only see the edges registered so far. Each
    /// instance finally receives its template's completed scope.
    ///
    /// # Errors
    ///
    /// Returns the first [`ModuleResolveError`] of registering an edge:
    /// unknown, private, or wrongly categorized items, duplicate local names,
    /// and invalid include projections.
    pub fn freeze(self) -> Result<ModuleResolver, ModuleResolveError> {
        let Self {
            mut resolver,
            sources,
        } = self;
        let own = sources
            .iter()
            .filter_map(|(owner, source)| match source {
                ScopeSource::Own(edges) => Some((owner, edges.as_slice())),
                ScopeSource::InheritFrom(_) => None,
            })
            .collect::<HashMap<_, _>>();
        let mut started = std::collections::HashSet::new();
        for (root, edges) in sources.iter().filter_map(|(owner, source)| match source {
            ScopeSource::Own(edges) => Some((owner, edges.as_slice())),
            ScopeSource::InheritFrom(_) => None,
        }) {
            if !started.insert(root) {
                continue;
            }
            let mut open = vec![(root, edges, 0_usize)];
            while let Some((owner, edges, next)) = open.last_mut() {
                if let Some(dependency) = edges.get(*next).map(Edge::dependency) {
                    *next += 1;
                    if let Some((&dependency, &dependency_edges)) = own.get_key_value(dependency)
                        && started.insert(dependency)
                    {
                        open.push((dependency, dependency_edges, 0));
                    }
                    continue;
                }
                resolver.complete_own_scope(owner, edges)?;
                open.pop();
            }
        }
        for (instance, source) in &sources {
            if let ScopeSource::InheritFrom(template) = source {
                resolver.inherit_scope(instance, template)?;
            }
        }
        Ok(resolver)
    }
}

impl ModuleResolver {
    /// Build a resolver for `modules` with no edge between them.
    ///
    /// # Errors
    ///
    /// Returns the first error of [`SymbolTables::add_module`].
    pub fn without_edges<'a>(
        modules: impl IntoIterator<Item = (DagId, &'a [ast::Declaration])>,
    ) -> Result<Self, ModuleResolveError> {
        Self::build(modules, &NoModuleTargets)
    }

    /// Build a resolver for `modules`, connected as `targets` decides.
    ///
    /// # Errors
    ///
    /// Returns the first error of [`SymbolTables::add_module`],
    /// [`SymbolTables::scopes`], or [`ScopeBuilder::freeze`].
    pub fn build<'a>(
        modules: impl IntoIterator<Item = (DagId, &'a [ast::Declaration])>,
        targets: &impl ModuleTargets,
    ) -> Result<Self, ModuleResolveError> {
        let mut tables = SymbolTables::default();
        for (owner, declarations) in modules {
            tables.add_module(owner, declarations)?;
        }
        tables.scopes(targets)?.freeze()
    }

    /// Register a source module's edges, in declaration order.
    fn complete_own_scope(
        &mut self,
        owner: &DagId,
        edges: &[Edge<'_>],
    ) -> Result<(), ModuleResolveError> {
        for edge in edges {
            match edge {
                Edge::Import { import, target } => self.register_import(owner, import, target)?,
                Edge::Include { include, template } => {
                    let instance = owner.instance_child(include.instance_scope());
                    self.inherit_scope(&instance, template)?;
                    self.register_include(owner, &include.path, &include.kind, &instance)?;
                    self.apply_include_static_projection_bindings(owner, template, include)?;
                }
            }
        }
        Ok(())
    }

    /// Give `instance` the current scope of its `template`.
    fn inherit_scope(
        &mut self,
        instance: &DagId,
        template: &DagId,
    ) -> Result<(), ModuleResolveError> {
        let scope = self.module_scope(template)?.clone();
        self.entry_mut(instance)?.scope = scope;
        Ok(())
    }
}

/// Modules and loader edges of one resolver under test.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct TestModules<'a> {
    modules: Vec<(DagId, &'a [ast::Declaration])>,
    edges: Vec<(DagId, Vec<crate::syntax::names::NameAtom>, DagId)>,
}

#[cfg(test)]
impl<'a> TestModules<'a> {
    fn segments(path: &ModulePath) -> Vec<crate::syntax::names::NameAtom> {
        path.segments()
            .iter()
            .map(|segment| segment.name.atom().clone())
            .collect()
    }

    /// Add one source module.
    pub(crate) fn add(&mut self, owner: DagId, declarations: &'a [ast::Declaration]) {
        self.modules.push((owner, declarations));
    }

    /// Add a file root and its nested inline `dag`s.
    pub(crate) fn add_file(&mut self, root: &DagId, declarations: &'a [ast::Declaration]) {
        self.add(root.clone(), declarations);
        for declaration in declarations {
            if let ast::DeclKind::Dag(dag) = &declaration.kind {
                self.add_file(&root.inline_dag_child(dag.name.value.clone()), &dag.body);
            }
        }
    }

    /// The loader resolves `path` in `owner` (an import or include) to `target`.
    pub(crate) fn edge(&mut self, owner: &DagId, path: &ModulePath, target: &DagId) {
        self.edges
            .push((owner.clone(), Self::segments(path), target.clone()));
    }

    /// The loader resolves `import` in `owner` to `target`.
    pub(crate) fn import(&mut self, owner: &DagId, import: &ast::ImportDecl, target: &DagId) {
        self.edge(owner, import.path(), target);
    }

    /// Build the resolver.
    pub(crate) fn build(&self) -> Result<ModuleResolver, ModuleResolveError> {
        ModuleResolver::build(
            self.modules
                .iter()
                .map(|(owner, declarations)| (owner.clone(), *declarations)),
            &|owner: &DagId, path: &ModulePath| {
                let segments = Self::segments(path);
                self.edges
                    .iter()
                    .find(|(edge_owner, edge_path, _)| {
                        edge_owner == owner && *edge_path == segments
                    })
                    .map(|(_, _, target)| target.clone())
            },
        )
    }
}

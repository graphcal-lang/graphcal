use crate::config::RoleMap;
use crate::discover::SourceTree;
use crate::model::{Edge, EdgeKey, Evidence, EvidenceForm, ModuleId, Package, Role};
use proc_macro2::TokenTree;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};
use syn::{Item, ItemExternCrate, ItemMod, Path as SynPath, UseTree};
use thiserror::Error;

#[derive(Debug)]
pub struct Analysis {
    pub edges: BTreeMap<EdgeKey, Edge>,
}

#[derive(Debug, Error)]
pub enum AnalyzeError {
    #[error("unsupported {construct} in {file}:{line}")]
    UnsupportedConstruct {
        construct: &'static str,
        file: PathBuf,
        line: usize,
    },
    #[error("cyclic re-export/import resolution at {module}::{name}")]
    ResolutionCycle { module: ModuleId, name: String },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PathSpec {
    components: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum BindingTarget {
    Path(PathSpec),
    Declared(ModuleId),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Binding {
    owner: ModuleId,
    name: String,
    target: BindingTarget,
    test_only: bool,
    providers: BTreeSet<ModuleId>,
    lexical_depth: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ScopedPath {
    owner: ModuleId,
    path: PathSpec,
    test_only: bool,
    lexical_depth: usize,
}

#[derive(Clone, Debug, Default)]
struct Scope {
    lexical_depth: usize,
    bindings: BTreeMap<String, Vec<Binding>>,
    globs: Vec<ScopedPath>,
    public_globs: Vec<ScopedPath>,
    exports: BTreeMap<String, Vec<Binding>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ResolutionKey {
    module: ModuleId,
    name: String,
    lexical_depth: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PathResolution {
    Module(ModuleId),
    Item(ModuleId),
    Untracked,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ResolvedPath {
    resolution: PathResolution,
    test_only: bool,
    providers: BTreeSet<ModuleId>,
}

pub fn analyze(tree: &SourceTree) -> Result<Analysis, AnalyzeError> {
    let contexts = module_test_contexts(tree);
    let mut state = Analyzer::new(tree, contexts)?;
    let sources = tree.modules.values().collect::<Vec<_>>();
    sources.iter().try_for_each(|source| {
        let inherited_test = *state
            .test_context
            .get(&source.id)
            .expect("every discovered module has a cfg context");
        state.scan_items(
            &source.id,
            &source.syntax.items,
            inherited_test,
            &source.file,
        )
    })?;
    Ok(Analysis { edges: state.edges })
}

struct Analyzer {
    edges: BTreeMap<EdgeKey, Edge>,
    modules: BTreeSet<ModuleId>,
    scopes: BTreeMap<ModuleId, Scope>,
    test_context: BTreeMap<ModuleId, bool>,
    lexical_scopes: BTreeMap<ModuleId, Vec<Scope>>,
}

impl Analyzer {
    fn new(
        tree: &SourceTree,
        test_context: BTreeMap<ModuleId, bool>,
    ) -> Result<Self, AnalyzeError> {
        let modules = tree.modules.keys().cloned().collect::<BTreeSet<_>>();
        let mut scopes = modules
            .iter()
            .cloned()
            .map(|module| (module, Scope::default()))
            .collect::<BTreeMap<_, _>>();

        tree.modules.values().try_for_each(|source| {
            let scope = scopes
                .get_mut(&source.id)
                .expect("every source has an initialized scope");
            source.syntax.items.iter().try_for_each(|item| {
                let item_test = cfg_test_only(item.attrs());
                match item {
                    Item::Use(use_item) => {
                        collect_use_bindings(
                            &use_item.tree,
                            Vec::new(),
                            use_item.vis.clone(),
                            item_test,
                            &source.id,
                            scope,
                        );
                    }
                    Item::ExternCrate(extern_crate) => {
                        collect_extern_binding(extern_crate, item_test, &source.id, scope);
                    }
                    _ => collect_declarations(item, item_test, &source.id, scope),
                }
                Ok::<(), AnalyzeError>(())
            })
        })?;

        Ok(Self {
            edges: BTreeMap::new(),
            modules,
            scopes,
            test_context,
            lexical_scopes: BTreeMap::new(),
        })
    }

    fn scan_items(
        &mut self,
        current: &ModuleId,
        items: &[Item],
        inherited_test: bool,
        file: &Path,
    ) -> Result<(), AnalyzeError> {
        items.iter().try_for_each(|item| {
            let item_test = inherited_test || cfg_test_only(item.attrs());
            match item {
                Item::Mod(module) => self.scan_mod(current, module, item_test, file),
                _ => {
                    let mut visitor = ModuleVisitor {
                        analyzer: self,
                        current: current.clone(),
                        test_only: item_test,
                        file: file.to_path_buf(),
                        error: Ok(()),
                    };
                    match item {
                        Item::Use(item_use) => visitor.visit_item_use(item_use),
                        Item::Macro(item_macro) => visitor.visit_macro(&item_macro.mac),
                        _ => visitor.visit_item(item),
                    }
                    visitor.finish()
                }
            }
        })
    }

    fn scan_mod(
        &mut self,
        current: &ModuleId,
        module: &ItemMod,
        test_only: bool,
        file: &Path,
    ) -> Result<(), AnalyzeError> {
        let child = self.child_module(current, &module.ident.to_string());
        if has_path_attribute(&module.attrs) && !self.modules.contains(&child) {
            return Err(AnalyzeError::UnsupportedConstruct {
                construct: "unmapped #[path] module",
                file: file.to_path_buf(),
                line: module.ident.span().start().line,
            });
        }
        self.add_edge(
            current,
            &child,
            test_only,
            file,
            module.ident.span().start().line,
            EvidenceForm::ModuleDeclaration,
        );
        Ok(())
    }

    fn child_module(&self, current: &ModuleId, name: &str) -> ModuleId {
        let mut path = current.path.clone();
        path.push(name.to_owned());
        ModuleId::new(current.package.clone(), path)
    }

    fn resolve_segments(
        &self,
        current: &ModuleId,
        segments: &[String],
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        self.resolve_segments_in_scope(
            current,
            segments,
            &mut BTreeSet::new(),
            self.lexical_scopes.get(current).map_or(0, Vec::len),
        )
    }

    fn resolve_segments_in_scope(
        &self,
        current: &ModuleId,
        segments: &[String],
        stack: &mut BTreeSet<ResolutionKey>,
        lexical_depth: usize,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        let Some(first) = segments.first() else {
            return Ok(vec![ResolvedPath {
                resolution: PathResolution::Untracked,
                test_only: false,
                providers: BTreeSet::new(),
            }]);
        };

        if let Some(package) = Package::from_name(first) {
            return self.apply_segments(ModuleId::new(package, Vec::new()), &segments[1..], stack);
        }
        if first == "crate" {
            return self.apply_segments(
                ModuleId::new(current.package.clone(), Vec::new()),
                &segments[1..],
                stack,
            );
        }
        if first == "self" {
            return self.apply_segments(current.clone(), &segments[1..], stack);
        }
        if first == "super" {
            let mut parent = current.path.clone();
            let mut index = 0;
            while segments.get(index).is_some_and(|part| part == "super") {
                if parent.pop().is_none() {
                    return Ok(Vec::new());
                }
                index += 1;
            }
            return self.apply_segments(
                ModuleId::new(current.package.clone(), parent),
                &segments[index..],
                stack,
            );
        }

        let child = self.child_module(current, first);
        if let Some(bindings) = self.lexical_scopes.get(current).and_then(|scopes| {
            scopes
                .iter()
                .take(lexical_depth)
                .rev()
                .find_map(|scope| scope.bindings.get(first))
        }) {
            return self.resolve_bindings(current, first, bindings, &segments[1..], stack);
        }
        if self.modules.contains(&child) {
            return self.apply_segments(child, &segments[1..], stack);
        }
        if let Some(bindings) = self.scope(current).bindings.get(first) {
            return self.resolve_bindings(current, first, bindings, &segments[1..], stack);
        }
        let mut globs = self
            .lexical_scopes
            .get(current)
            .into_iter()
            .flat_map(|scopes| scopes.iter().take(lexical_depth).rev())
            .flat_map(|scope| scope.globs.clone())
            .collect::<Vec<_>>();
        globs.extend(self.scope(current).globs.clone());
        let mut results = Vec::new();
        let glob_key = ResolutionKey {
            module: current.clone(),
            name: first.clone(),
            lexical_depth,
        };
        if stack.insert(glob_key.clone()) {
            let glob_result = (|| {
                for glob in globs {
                    let glob_modules = self.resolve_scoped_path(&glob, stack)?;
                    for glob_module in glob_modules {
                        if let PathResolution::Module(module) = glob_module.resolution {
                            for binding in self.export_targets(&module, first, stack)? {
                                for mut resolved in self.resolve_binding(first, &binding, stack)? {
                                    resolved.test_only |= glob.test_only || glob_module.test_only;
                                    results.extend(self.apply_resolved(
                                        resolved,
                                        &segments[1..],
                                        stack,
                                    )?);
                                }
                            }
                        }
                    }
                }
                Ok::<(), AnalyzeError>(())
            })();
            stack.remove(&glob_key);
            glob_result?;
        }
        Ok(deduplicate(results))
    }

    fn resolve_scoped_path(
        &self,
        scoped: &ScopedPath,
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        let mut paths = self.resolve_segments_in_scope(
            &scoped.owner,
            &scoped.path.components,
            stack,
            scoped.lexical_depth,
        )?;
        paths
            .iter_mut()
            .for_each(|path| path.test_only |= scoped.test_only);
        Ok(paths)
    }

    fn resolve_bindings(
        &self,
        owner: &ModuleId,
        name: &str,
        bindings: &[Binding],
        rest: &[String],
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        let mut results = Vec::new();
        for binding in bindings {
            for resolved in self.resolve_binding(name, binding, stack)? {
                results.extend(self.apply_resolved(resolved, rest, stack)?);
            }
        }
        if results.is_empty() {
            let _ = owner;
        }
        Ok(deduplicate(results))
    }

    fn resolve_binding(
        &self,
        name: &str,
        binding: &Binding,
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        let key = ResolutionKey {
            module: binding.owner.clone(),
            name: name.to_owned(),
            lexical_depth: binding.lexical_depth,
        };
        if !stack.insert(key.clone()) {
            return Err(AnalyzeError::ResolutionCycle {
                module: key.module,
                name: key.name,
            });
        }
        let result = match &binding.target {
            BindingTarget::Declared(module) => Ok(vec![ResolvedPath {
                resolution: PathResolution::Item(module.clone()),
                test_only: binding.test_only,
                providers: binding.providers.clone(),
            }]),
            BindingTarget::Path(path) => self
                .resolve_segments_in_scope(
                    &binding.owner,
                    &path.components,
                    stack,
                    binding.lexical_depth,
                )
                .map(|mut paths| {
                    if paths.is_empty() {
                        paths.push(ResolvedPath {
                            resolution: PathResolution::Untracked,
                            test_only: false,
                            providers: BTreeSet::new(),
                        });
                    }
                    paths.iter_mut().for_each(|path| {
                        path.test_only |= binding.test_only;
                        path.providers.insert(binding.owner.clone());
                        path.providers.extend(binding.providers.iter().cloned());
                    });
                    paths
                }),
        };
        stack.remove(&key);
        result
    }

    fn apply_segments(
        &self,
        base: ModuleId,
        rest: &[String],
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        let Some(segment) = rest.first() else {
            return Ok(vec![ResolvedPath {
                resolution: PathResolution::Module(base),
                test_only: false,
                providers: BTreeSet::new(),
            }]);
        };
        let child = self.child_module(&base, segment);
        if self.modules.contains(&child) {
            return self.apply_segments(child, &rest[1..], stack);
        }
        let mut results = Vec::new();
        for binding in self.export_targets(&base, segment, stack)? {
            for resolved in self.resolve_binding(segment, &binding, stack)? {
                results.extend(self.apply_resolved(resolved, &rest[1..], stack)?);
            }
        }
        if results.is_empty()
            && let Some(bindings) = self.scope(&base).bindings.get(segment)
        {
            for binding in bindings {
                for resolved in self.resolve_binding(segment, binding, stack)? {
                    results.extend(self.apply_resolved(resolved, &rest[1..], stack)?);
                }
            }
        }
        if results.is_empty() {
            // The final component may be an external or associated item that
            // is not declared in this source tree. Keep the namespace module
            // as the conservative producer, but never walk into a sibling
            // module after an item has been selected.
            Ok(vec![ResolvedPath {
                resolution: PathResolution::Module(base),
                test_only: false,
                providers: BTreeSet::new(),
            }])
        } else {
            Ok(deduplicate(results))
        }
    }

    fn apply_resolved(
        &self,
        resolution: ResolvedPath,
        rest: &[String],
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<ResolvedPath>, AnalyzeError> {
        match resolution.resolution {
            PathResolution::Module(module) => {
                self.apply_segments(module, rest, stack).map(|mut paths| {
                    paths.iter_mut().for_each(|path| {
                        path.test_only |= resolution.test_only;
                        path.providers.extend(resolution.providers.iter().cloned());
                    });
                    paths
                })
            }
            // An item suffix is an associated/UFCS-like name. It is not a Rust
            // namespace that this source-only guard may resolve, but it still
            // belongs to the item's defining module for layer analysis.
            PathResolution::Item(module) => Ok(vec![ResolvedPath {
                resolution: PathResolution::Item(module),
                test_only: resolution.test_only,
                providers: resolution.providers,
            }]),
            PathResolution::Untracked => Ok(vec![resolution]),
        }
    }

    fn scope(&self, module: &ModuleId) -> &Scope {
        self.scopes
            .get(module)
            .expect("all resolved modules have a scope")
    }

    fn add_edge(
        &mut self,
        from: &ModuleId,
        to: &ModuleId,
        test_only: bool,
        file: &Path,
        line: usize,
        form: EvidenceForm,
    ) {
        if from == to {
            return;
        }
        let key = EdgeKey {
            from: from.clone(),
            to: to.clone(),
            test_only,
        };
        let evidence = Evidence {
            file: file.to_path_buf(),
            line,
            form,
        };
        self.edges
            .entry(key.clone())
            .or_insert_with(|| Edge {
                key,
                evidence: Vec::new(),
            })
            .evidence
            .push(evidence);
    }

    fn add_resolution_edges(
        &mut self,
        from: &ModuleId,
        resolutions: &[ResolvedPath],
        inherited_test: bool,
        file: &Path,
        line: usize,
        form: EvidenceForm,
    ) {
        resolutions.iter().for_each(|resolved| {
            resolved.providers.iter().for_each(|provider| {
                self.add_edge(
                    from,
                    provider,
                    inherited_test || resolved.test_only,
                    file,
                    line,
                    form,
                )
            });
            let target = match &resolved.resolution {
                PathResolution::Module(module) | PathResolution::Item(module) => module,
                PathResolution::Untracked => return,
            };
            self.add_edge(
                from,
                target,
                inherited_test || resolved.test_only,
                file,
                line,
                form,
            );
        });
    }

    fn export_targets(
        &self,
        module: &ModuleId,
        name: &str,
        stack: &mut BTreeSet<ResolutionKey>,
    ) -> Result<Vec<Binding>, AnalyzeError> {
        let key = ResolutionKey {
            module: module.clone(),
            name: name.to_owned(),
            lexical_depth: 0,
        };
        if !stack.insert(key.clone()) {
            return Err(AnalyzeError::ResolutionCycle {
                module: key.module,
                name: key.name,
            });
        }
        let direct = self
            .scope(module)
            .exports
            .get(name)
            .map_or_else(Vec::new, Clone::clone);
        let mut result = direct;
        for glob in self.scope(module).public_globs.clone() {
            for resolved in self.resolve_scoped_path(&glob, stack)? {
                if let PathResolution::Module(glob_module) = resolved.resolution {
                    let mut visited = BTreeSet::new();
                    for mut binding in self.exported_bindings(&glob_module, &mut visited)? {
                        if binding.name == name {
                            binding.test_only |= resolved.test_only;
                            binding.providers.insert(module.clone());
                            binding.providers.extend(resolved.providers.iter().cloned());
                            result.push(binding);
                        }
                    }
                }
            }
        }
        stack.remove(&key);
        Ok(deduplicate_bindings(result))
    }

    fn exported_bindings(
        &self,
        module: &ModuleId,
        visited: &mut BTreeSet<ModuleId>,
    ) -> Result<Vec<Binding>, AnalyzeError> {
        if !visited.insert(module.clone()) {
            return Err(AnalyzeError::ResolutionCycle {
                module: module.clone(),
                name: "*".to_owned(),
            });
        }
        let mut result = self
            .scope(module)
            .exports
            .values()
            .flat_map(|bindings| bindings.iter().cloned())
            .collect::<Vec<_>>();
        for glob in self.scope(module).public_globs.clone() {
            for resolved in self.resolve_scoped_path(&glob, &mut BTreeSet::new())? {
                if let PathResolution::Module(glob_module) = resolved.resolution {
                    for mut binding in self.exported_bindings(&glob_module, visited)? {
                        binding.test_only |= resolved.test_only;
                        binding.providers.insert(module.clone());
                        binding.providers.extend(resolved.providers.iter().cloned());
                        result.push(binding);
                    }
                }
            }
        }
        visited.remove(module);
        Ok(deduplicate_bindings(result))
    }

    fn add_glob_export_edges(
        &mut self,
        from: &ModuleId,
        module: &ModuleId,
        test_only: bool,
        file: &Path,
        line: usize,
    ) -> Result<(), AnalyzeError> {
        let exports = self.exported_bindings(module, &mut BTreeSet::new())?;
        for binding in exports {
            let resolutions =
                self.resolve_binding(&binding.name, &binding, &mut BTreeSet::new())?;
            self.add_resolution_edges(
                from,
                &resolutions,
                test_only,
                file,
                line,
                EvidenceForm::UseGlob,
            );
        }
        Ok(())
    }
}

fn deduplicate(paths: Vec<ResolvedPath>) -> Vec<ResolvedPath> {
    paths
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn deduplicate_bindings(bindings: Vec<Binding>) -> Vec<Binding> {
    bindings
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

struct ModuleVisitor<'a> {
    analyzer: &'a mut Analyzer,
    current: ModuleId,
    test_only: bool,
    file: PathBuf,
    error: Result<(), AnalyzeError>,
}

impl ModuleVisitor<'_> {
    fn finish(self) -> Result<(), AnalyzeError> {
        self.error
    }

    fn fail(&mut self, error: AnalyzeError) {
        if self.error.is_ok() {
            self.error = Err(error);
        }
    }

    fn path_edge(&mut self, path: &SynPath, form: EvidenceForm) {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let line = path
            .segments
            .first()
            .map_or(0, |segment| segment.ident.span().start().line);
        match self.analyzer.resolve_segments(&self.current, &segments) {
            Ok(resolutions) => self.analyzer.add_resolution_edges(
                &self.current,
                &resolutions,
                self.test_only,
                &self.file,
                line,
                form,
            ),
            Err(error) => self.fail(error),
        }
    }

    fn use_edges(&mut self, tree: &UseTree, prefix: Vec<String>, line: usize) {
        match tree {
            UseTree::Path(path) => {
                let mut next = prefix;
                next.push(path.ident.to_string());
                self.use_edges(&path.tree, next, line);
            }
            UseTree::Name(name) => {
                let mut full = prefix;
                let local_is_self = name.ident == "self" && !full.is_empty();
                full.push(name.ident.to_string());
                if local_is_self {
                    full.pop();
                }
                self.resolve_use(&full, line, EvidenceForm::UseImport);
            }
            UseTree::Rename(rename) => {
                let mut full = prefix;
                full.push(rename.ident.to_string());
                self.resolve_use(&full, line, EvidenceForm::UseImport);
            }
            UseTree::Glob(_) => match self.analyzer.resolve_segments(&self.current, &prefix) {
                Ok(resolutions) => {
                    for resolution in resolutions {
                        if let PathResolution::Module(module) = resolution.resolution {
                            self.analyzer.add_edge(
                                &self.current,
                                &module,
                                self.test_only || resolution.test_only,
                                &self.file,
                                line,
                                EvidenceForm::UseGlob,
                            );
                            if let Err(error) = self.analyzer.add_glob_export_edges(
                                &self.current,
                                &module,
                                self.test_only || resolution.test_only,
                                &self.file,
                                line,
                            ) {
                                self.fail(error);
                            }
                        }
                    }
                }
                Err(error) => self.fail(error),
            },
            UseTree::Group(group) => group
                .items
                .iter()
                .for_each(|item| self.use_edges(item, prefix.clone(), line)),
        }
    }

    fn resolve_use(&mut self, segments: &[String], line: usize, form: EvidenceForm) {
        match self.analyzer.resolve_segments(&self.current, segments) {
            Ok(resolutions) => self.analyzer.add_resolution_edges(
                &self.current,
                &resolutions,
                self.test_only,
                &self.file,
                line,
                form,
            ),
            Err(error) => self.fail(error),
        }
    }

    fn macro_edges(&mut self, tokens: &proc_macro2::TokenStream, line: usize) {
        let trees = tokens.clone().into_iter().collect::<Vec<_>>();
        trees.iter().for_each(|tree| {
            if let TokenTree::Group(group) = tree {
                self.macro_edges(&group.stream(), line);
            }
        });
        (0..trees.len()).for_each(|index| {
            let (mut segments, mut cursor) = if index + 2 < trees.len()
                && is_colon(&trees[index])
                && is_colon(&trees[index + 1])
            {
                match &trees[index + 2] {
                    TokenTree::Ident(first) => (vec![first.to_string()], index + 3),
                    _ => return,
                }
            } else if index + 1 < trees.len()
                && is_dollar(&trees[index])
                && matches!(&trees[index + 1], TokenTree::Ident(ident) if ident == "crate")
            {
                (vec!["crate".to_owned()], index + 2)
            } else {
                let TokenTree::Ident(first) = &trees[index] else {
                    return;
                };
                (vec![first.to_string()], index + 1)
            };
            while cursor + 2 < trees.len()
                && is_colon(&trees[cursor])
                && is_colon(&trees[cursor + 1])
            {
                if let TokenTree::Ident(next) = &trees[cursor + 2] {
                    segments.push(next.to_string());
                    cursor += 3;
                } else {
                    break;
                }
            }
            if segments.len() > 1 {
                match self.analyzer.resolve_segments(&self.current, &segments) {
                    Ok(resolutions) => self.analyzer.add_resolution_edges(
                        &self.current,
                        &resolutions,
                        self.test_only,
                        &self.file,
                        line,
                        EvidenceForm::MacroTokenPath,
                    ),
                    Err(error) => self.fail(error),
                }
            }
        });
    }
}

impl<'ast> Visit<'ast> for ModuleVisitor<'_> {
    // Visibility paths describe access scope, not a consumed implementation.
    // In particular, pub(crate) must not fabricate an edge to the crate facade.
    fn visit_visibility(&mut self, _node: &'ast syn::Visibility) {}

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        self.fail(AnalyzeError::UnsupportedConstruct {
            construct: "block-local module",
            file: self.file.clone(),
            line: node.ident.span().start().line,
        });
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.use_edges(&node.tree, Vec::new(), node.use_token.span.start().line);
    }

    fn visit_item(&mut self, node: &'ast Item) {
        let previous_test = self.test_only;
        self.test_only |= cfg_test_only(node.attrs());
        visit::visit_item(self, node);
        self.test_only = previous_test;
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        let mut scope = Scope {
            lexical_depth: self
                .analyzer
                .lexical_scopes
                .get(&self.current)
                .map_or(0, Vec::len)
                .saturating_add(1),
            ..Scope::default()
        };
        node.stmts.iter().for_each(|statement| {
            if let syn::Stmt::Item(Item::Use(item_use)) = statement {
                collect_use_bindings(
                    &item_use.tree,
                    Vec::new(),
                    syn::Visibility::Inherited,
                    cfg_test_only(&item_use.attrs),
                    &self.current,
                    &mut scope,
                );
            }
        });
        self.analyzer
            .lexical_scopes
            .entry(self.current.clone())
            .or_default()
            .push(scope);
        visit::visit_block(self, node);
        self.analyzer
            .lexical_scopes
            .get_mut(&self.current)
            .expect("lexical scope was pushed for this block")
            .pop();
    }

    fn visit_stmt(&mut self, node: &'ast syn::Stmt) {
        let attrs = match node {
            syn::Stmt::Local(local) => &local.attrs,
            syn::Stmt::Item(item) => item.attrs(),
            syn::Stmt::Macro(mac) => &mac.attrs,
            syn::Stmt::Expr(_, _) => &[] as &[syn::Attribute],
        };
        let previous = self.test_only;
        self.test_only |= cfg_test_only(attrs);
        visit::visit_stmt(self, node);
        self.test_only = previous;
    }

    fn visit_path(&mut self, node: &'ast SynPath) {
        self.path_edge(node, EvidenceForm::QualifiedPath);
        visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let line = node
            .path
            .segments
            .first()
            .map_or(0, |segment| segment.ident.span().start().line);
        if node.path.is_ident("include") || contains_include_macro(&node.tokens) {
            self.fail(AnalyzeError::UnsupportedConstruct {
                construct: "include! macro",
                file: self.file.clone(),
                line,
            });
            return;
        }
        self.path_edge(&node.path, EvidenceForm::MacroInvocation);
        self.macro_edges(&node.tokens, line);
    }
}

fn collect_use_bindings(
    tree: &UseTree,
    prefix: Vec<String>,
    visibility: syn::Visibility,
    test_only: bool,
    owner: &ModuleId,
    scope: &mut Scope,
) {
    match tree {
        UseTree::Path(path) => {
            let mut next = prefix;
            next.push(path.ident.to_string());
            collect_use_bindings(&path.tree, next, visibility, test_only, owner, scope);
        }
        UseTree::Name(name) => {
            let mut target = prefix;
            let local = if name.ident == "self" && !target.is_empty() {
                target
                    .last()
                    .cloned()
                    .unwrap_or_else(|| name.ident.to_string())
            } else {
                target.push(name.ident.to_string());
                name.ident.to_string()
            };
            insert_binding(scope, local, target, visibility, test_only, owner);
        }
        UseTree::Rename(rename) => {
            let mut target = prefix;
            target.push(rename.ident.to_string());
            insert_binding(
                scope,
                rename.rename.to_string(),
                target,
                visibility,
                test_only,
                owner,
            );
        }
        UseTree::Glob(_) => {
            let scoped = ScopedPath {
                owner: owner.clone(),
                path: PathSpec { components: prefix },
                test_only,
                lexical_depth: scope.lexical_depth,
            };
            scope.globs.push(scoped.clone());
            if is_exported_visibility(&visibility) {
                scope.public_globs.push(scoped);
            }
        }
        UseTree::Group(group) => group.items.iter().for_each(|item| {
            collect_use_bindings(
                item,
                prefix.clone(),
                visibility.clone(),
                test_only,
                owner,
                scope,
            )
        }),
    }
}

fn insert_binding(
    scope: &mut Scope,
    local: String,
    target: Vec<String>,
    visibility: syn::Visibility,
    test_only: bool,
    owner: &ModuleId,
) {
    let binding = Binding {
        owner: owner.clone(),
        name: local.clone(),
        target: BindingTarget::Path(PathSpec { components: target }),
        test_only,
        providers: BTreeSet::new(),
        lexical_depth: scope.lexical_depth,
    };
    scope
        .bindings
        .entry(local.clone())
        .or_default()
        .push(binding.clone());
    if is_exported_visibility(&visibility) {
        scope.exports.entry(local).or_default().push(binding);
    }
}

fn collect_extern_binding(
    item: &ItemExternCrate,
    test_only: bool,
    owner: &ModuleId,
    scope: &mut Scope,
) {
    let name = item
        .rename
        .as_ref()
        .map_or_else(|| item.ident.to_string(), |rename| rename.1.to_string());
    if Package::from_name(&item.ident.to_string()).is_some() {
        let binding = Binding {
            owner: owner.clone(),
            name: name.clone(),
            target: BindingTarget::Path(PathSpec {
                components: vec![item.ident.to_string()],
            }),
            test_only,
            providers: BTreeSet::new(),
            lexical_depth: scope.lexical_depth,
        };
        scope
            .bindings
            .entry(name.clone())
            .or_default()
            .push(binding.clone());
        if is_exported_visibility(&item.vis) {
            scope.exports.entry(name).or_default().push(binding);
        }
    }
}

fn is_exported_visibility(visibility: &syn::Visibility) -> bool {
    matches!(
        visibility,
        syn::Visibility::Public(_) | syn::Visibility::Restricted(_)
    )
}

fn collect_declarations(item: &Item, test_only: bool, owner: &ModuleId, scope: &mut Scope) {
    let (name, visibility) = match item {
        Item::Const(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Enum(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Fn(item) => (Some(item.sig.ident.to_string()), &item.vis),
        Item::Static(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Struct(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Trait(item) => (Some(item.ident.to_string()), &item.vis),
        Item::TraitAlias(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Type(item) => (Some(item.ident.to_string()), &item.vis),
        Item::Union(item) => (Some(item.ident.to_string()), &item.vis),
        _ => (None, &syn::Visibility::Inherited),
    };
    let Some(name) = name else { return };
    let binding = Binding {
        owner: owner.clone(),
        name: name.clone(),
        target: BindingTarget::Declared(owner.clone()),
        test_only,
        providers: BTreeSet::new(),
        lexical_depth: scope.lexical_depth,
    };
    scope
        .bindings
        .entry(name.clone())
        .or_default()
        .push(binding.clone());
    if is_exported_visibility(visibility) {
        scope.exports.entry(name).or_default().push(binding);
    }
}

fn module_test_contexts(tree: &SourceTree) -> BTreeMap<ModuleId, bool> {
    let mut contexts = tree
        .modules
        .values()
        .map(|source| (source.id.clone(), cfg_test_only(&source.syntax.attrs)))
        .collect::<BTreeMap<_, _>>();
    let sources = tree.modules.values().collect::<Vec<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        sources.iter().for_each(|source| {
            let parent_test = contexts.get(&source.id).copied().unwrap_or(false);
            source.syntax.items.iter().for_each(|item| {
                if let Item::Mod(module) = item {
                    let child = child_id(&source.id, &module.ident.to_string());
                    let child_test = parent_test || cfg_test_only(&module.attrs);
                    let entry = contexts.entry(child).or_insert(false);
                    if child_test && !*entry {
                        *entry = true;
                        changed = true;
                    }
                }
            });
        });
    }
    contexts
}

fn child_id(current: &ModuleId, name: &str) -> ModuleId {
    let mut path = current.path.clone();
    path.push(name.to_owned());
    ModuleId::new(current.package.clone(), path)
}

fn has_path_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("path"))
}

#[derive(Clone, Copy)]
struct TruthSet {
    can_true: bool,
    can_false: bool,
}

fn cfg_test_only(attrs: &[syn::Attribute]) -> bool {
    let values = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("cfg") {
                return None;
            }
            let syn::Meta::List(list) = &attr.meta else {
                return Some(TruthSet {
                    can_true: true,
                    can_false: true,
                });
            };
            Some(list.parse_args::<syn::Meta>().map_or(
                TruthSet {
                    can_true: true,
                    can_false: true,
                },
                cfg_truth,
            ))
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        return false;
    }
    // Separate cfg attributes are a conjunction. The predicate can be true
    // in a production build only if every cofactor can be true there.
    let conjunction = TruthSet {
        can_true: values.iter().all(|value| value.can_true),
        can_false: values.iter().any(|value| value.can_false),
    };
    !conjunction.can_true
}

fn cfg_truth(meta: syn::Meta) -> TruthSet {
    match meta {
        syn::Meta::Path(path) if path.is_ident("test") => TruthSet {
            can_true: false,
            can_false: true,
        },
        syn::Meta::Path(_) | syn::Meta::NameValue(_) => TruthSet {
            can_true: true,
            can_false: true,
        },
        syn::Meta::List(list) => {
            let values = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .map(|metas| metas.into_iter().map(cfg_truth).collect::<Vec<_>>());
            let Ok(values) = values else {
                return TruthSet {
                    can_true: true,
                    can_false: true,
                };
            };
            match list
                .path
                .get_ident()
                .map(|ident| ident.to_string())
                .as_deref()
            {
                Some("any") => TruthSet {
                    can_true: values.iter().any(|value| value.can_true),
                    can_false: values.iter().all(|value| value.can_false),
                },
                Some("all") => TruthSet {
                    can_true: values.iter().all(|value| value.can_true),
                    can_false: values.iter().any(|value| value.can_false),
                },
                Some("not") if values.len() == 1 => TruthSet {
                    can_true: values[0].can_false,
                    can_false: values[0].can_true,
                },
                // Unknown predicate forms are conservatively production-capable.
                _ => TruthSet {
                    can_true: true,
                    can_false: true,
                },
            }
        }
    }
}

trait ItemAttrs {
    fn attrs(&self) -> &[syn::Attribute];
}

impl ItemAttrs for Item {
    fn attrs(&self) -> &[syn::Attribute] {
        match self {
            Item::Const(item) => &item.attrs,
            Item::Enum(item) => &item.attrs,
            Item::ExternCrate(item) => &item.attrs,
            Item::Fn(item) => &item.attrs,
            Item::ForeignMod(item) => &item.attrs,
            Item::Impl(item) => &item.attrs,
            Item::Macro(item) => &item.attrs,
            Item::Mod(item) => &item.attrs,
            Item::Static(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Trait(item) => &item.attrs,
            Item::TraitAlias(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Use(item) => &item.attrs,
            Item::Verbatim(_) => &[],
            _ => &[],
        }
    }
}

fn contains_include_macro(tokens: &proc_macro2::TokenStream) -> bool {
    let trees = tokens.clone().into_iter().collect::<Vec<_>>();
    trees.windows(2).any(|pair| {
        matches!(&pair[0], TokenTree::Ident(ident) if ident == "include")
            && matches!(&pair[1], TokenTree::Punct(punct) if punct.as_char() == '!')
    }) || trees.iter().any(|tree| match tree {
        TokenTree::Group(group) => contains_include_macro(&group.stream()),
        _ => false,
    })
}

fn is_colon(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ':')
}

fn is_dollar(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(punct) if punct.as_char() == '$')
}

pub fn forbidden(from: Role, to: Role) -> bool {
    matches!(
        (from, to),
        (
            Role::Contracts,
            Role::Checking | Role::Interpreter | Role::Loading | Role::Facade
        ) | (Role::Checking, Role::Facade | Role::Loading)
            | (
                Role::Interpreter,
                Role::Checking | Role::Facade | Role::Loading
            )
    )
}

pub fn violations(analysis: &Analysis, roles: &RoleMap) -> Vec<Edge> {
    analysis
        .edges
        .values()
        .filter(|edge| {
            roles.role_for(&edge.key.from).is_some_and(|from| {
                roles
                    .role_for(&edge.key.to)
                    .is_some_and(|to| forbidden(from, to))
            })
        })
        .cloned()
        .collect()
}

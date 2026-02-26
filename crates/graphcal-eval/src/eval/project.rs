//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{DeclKind, Expr, ExprKind, ImportPath};
use graphcal_syntax::names::{DeclName, FnName, Spanned};
use graphcal_syntax::span::Span;
use graphcal_syntax::visitor::ExprVisitorMut;

use crate::declared_type::DeclaredType;
use crate::error::GraphcalError;
use crate::registry::{Registry, RegistryBuilder};
use crate::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use crate::runtime_value::RuntimeValue;

use super::runtime::evaluate_plan;
use super::types::{AssertResult, CompileError, EvalResult};

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// Resolve an import path to a canonical file path within an already-loaded project.
///
/// This function is **I/O-free**: it resolves paths by looking up canonical keys
/// in `project.files` rather than calling the filesystem.
///
/// - `FilePath`: joins with `parent_dir` and matches against project file keys.
/// - `ModulePath`: matches against project file keys by suffix.
fn resolve_import_to_canonical(
    import_path: &ImportPath,
    parent_dir: &Path,
    project: &crate::loader::LoadedProject,
    src: &NamedSource<Arc<String>>,
) -> Result<PathBuf, CompileError> {
    match import_path {
        ImportPath::FilePath { path, span } => {
            let file_path = parent_dir.join(path);
            // Look up the resolved path among already-loaded project files.
            // The loader stored all files under canonical keys, so we match
            // by checking which canonical key ends with our joined path's
            // file components. This avoids filesystem I/O.
            for canonical_path in project.files.keys() {
                if paths_match(canonical_path, &file_path) {
                    return Ok(canonical_path.clone());
                }
            }
            Err(CompileError::Eval(GraphcalError::ImportFileNotFound {
                path: path.clone(),
                src: src.clone(),
                span: (*span).into(),
            }))
        }
        ImportPath::ModulePath { segments, span } => {
            // Module paths are resolved by the loader. Since the project is already loaded,
            // we need to find the canonical path from project.files.
            let search_suffix = segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("/")
                + ".gcl";

            for canonical_path in project.files.keys() {
                if canonical_path.ends_with(&search_suffix) {
                    return Ok(canonical_path.clone());
                }
            }

            Err(CompileError::Eval(GraphcalError::ImportFileNotFound {
                path: import_path.display_path(),
                src: src.clone(),
                span: (*span).into(),
            }))
        }
    }
}

/// Check whether a canonical path matches a (possibly relative) file path.
///
/// Returns `true` if `canonical` ends with the same sequence of path components
/// as the **normalized** `target`. Normalization resolves `.` and `..` components
/// logically (without filesystem access) so that paths like
/// `/foo/child/../lib.gcl` correctly match `/foo/lib.gcl`.
///
/// This also handles the macOS `/tmp` → `/private/tmp` symlink case where
/// `target` and `canonical` share a common suffix but differ in prefix.
fn paths_match(canonical: &Path, target: &Path) -> bool {
    let normalized = normalize_path(target);
    // Fast path: exact match.
    if canonical == normalized {
        return true;
    }
    // Compare trailing components. The target path (from parent_dir.join(import))
    // may differ from the canonical path only in prefix (symlink resolution).
    // Match by comparing components from the end.
    let c_components: Vec<_> = canonical.components().collect();
    let n_components: Vec<_> = normalized.components().collect();
    if n_components.len() > c_components.len() {
        return false;
    }
    c_components
        .iter()
        .rev()
        .zip(n_components.iter().rev())
        .all(|(a, b)| a == b)
}

/// Normalize a path by resolving `.` and `..` components logically,
/// without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last normal component if possible.
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            Component::CurDir => {
                // Skip `.` components.
            }
            _ => {
                components.push(component);
            }
        }
    }
    components.iter().collect()
}

/// Helper function to derive a module name from an `ImportPath`.
///
/// For `FilePath`, uses the filename stem.
/// For `ModulePath`, uses the last segment as the module name.
fn derive_module_name_from_import_path(
    import_path: &ImportPath,
    src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match import_path {
        ImportPath::FilePath { path, span } => {
            crate::loader::derive_module_name(path).map_err(|stem| {
                CompileError::Eval(GraphcalError::InvalidModuleName {
                    stem,
                    src: src.clone(),
                    span: (*span).into(),
                })
            })
        }
        ImportPath::ModulePath { segments, .. } => {
            // For module paths, the last segment is the module name
            Ok(segments
                .last()
                .map_or_else(|| "module".to_string(), |seg| seg.name.clone()))
        }
    }
}

/// Visitor that rewrites qualified references to flat names.
struct QualifiedRefRewriter;

impl ExprVisitorMut for QualifiedRefRewriter {
    type Error = std::convert::Infallible;

    fn visit_qualified_graph_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        let old_kind = std::mem::replace(&mut expr.kind, ExprKind::Number(0.0));
        expr.kind = match old_kind {
            ExprKind::QualifiedGraphRef { module, name } => {
                let flat = DeclName::new(format!("{}::{}", module.name, name.value));
                ExprKind::GraphRef(Spanned {
                    value: flat,
                    span: name.span,
                })
            }
            other => other,
        };
        Ok(())
    }

    fn visit_qualified_const_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        let old_kind = std::mem::replace(&mut expr.kind, ExprKind::Number(0.0));
        expr.kind = match old_kind {
            ExprKind::QualifiedConstRef { module, name } => {
                let flat = DeclName::new(format!("{}::{}", module.name, name.value));
                ExprKind::ConstRef(Spanned {
                    value: flat,
                    span: name.span,
                })
            }
            other => other,
        };
        Ok(())
    }

    fn visit_qualified_fn_call_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // First recurse into args
        if let ExprKind::QualifiedFnCall { args, .. } = &mut expr.kind {
            for arg in args {
                self.visit_expr_mut(arg)?;
            }
        }
        // Then rewrite the node itself
        let old_kind = std::mem::replace(&mut expr.kind, ExprKind::Number(0.0));
        expr.kind = match old_kind {
            ExprKind::QualifiedFnCall { module, name, args } => {
                let flat = FnName::new(format!("{}::{}", module.name, name.value));
                ExprKind::FnCall {
                    name: Spanned {
                        value: flat,
                        span: name.span,
                    },
                    args,
                }
            }
            other => other,
        };
        Ok(())
    }
}

/// Rewrite qualified references to flat names in-place.
///
/// Replaces `QualifiedGraphRef { module: "m", name: "x" }` with `GraphRef("m::x")`,
/// `QualifiedConstRef` with `ConstRef`, and `QualifiedFnCall` with `FnCall`.
pub(super) fn rewrite_qualified_refs(expr: &mut Expr) {
    let mut rewriter = QualifiedRefRewriter;
    let _ = rewriter.visit_expr_mut(expr);
}

// ---------------------------------------------------------------------------
// Per-file evaluation types and pipeline
// ---------------------------------------------------------------------------

/// The result of evaluating a single file in the per-file pipeline.
struct EvaluatedFile {
    /// Evaluated runtime values (params + nodes): name → `RuntimeValue`.
    values: HashMap<String, RuntimeValue>,
    /// Evaluated const values: name → `RuntimeValue`.
    const_values: HashMap<String, RuntimeValue>,
    /// Declared types for all consts/params/nodes in this file.
    declared_types: HashMap<String, DeclaredType>,
    /// Assertion results from this file: name → (result, span).
    assertions: HashMap<DeclName, (AssertResult, Span)>,
    /// The file's frozen registry (for type-system import by downstream files).
    registry: Registry,
    /// Functions declared in this file.
    functions: Vec<graphcal_ir::ir::FunctionEntry>,
}

impl EvaluatedFile {
    /// Check whether this file declares an assertion with the given name.
    fn has_assert(&self, name: &str) -> bool {
        self.assertions.keys().any(|n| n.as_str() == name)
    }
}

/// The result of compiling a single file within a project context.
///
/// Produced by [`compile_single_file_in_project`] and consumed by the
/// per-file evaluation and TIR compilation pipelines.
struct CompiledFile {
    tir: crate::tir::TIR,
    declared_types: HashMap<String, DeclaredType>,
    /// Imported values for this file (cloned before being consumed by IR).
    /// Used by the root file to enrich output with imported value names.
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    /// Imported value categories in source order (for root output).
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
}

/// Return type for [`build_dep_imported_values`].
type DepImportedValues = (
    ImportedValueNames,
    HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
);

/// An instantiated import that needs IR merging (deferred until after lowering).
struct DeferredInstantiatedImport {
    /// Canonical path of the dependency file.
    dep_path: PathBuf,
    /// The prefix for all merged declarations (from alias or filename).
    prefix: String,
    /// Param bindings: `param_name` → binding expression.
    bindings: HashMap<String, Expr>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports (all names are accessible via `prefix::`).
    selective_names: Option<Vec<(String, String)>>, // (orig_name, local_name)
    /// Span of the import declaration (for diagnostics).
    import_span: Span,
}

/// Compile a single file within a project, using pre-evaluated values from dependencies.
///
/// Builds import bindings, lowers to IR, applies overrides, and type-resolves to TIR.
/// Both [`evaluate_project_perfile`] and [`compile_to_tir_project_perfile`] call this
/// for each file in the project.
#[expect(
    clippy::too_many_lines,
    reason = "per-file compilation is a single logical pipeline"
)]
fn compile_single_file_in_project(
    project: &crate::loader::LoadedProject,
    file_path: &Path,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    override_targets: &HashMap<DeclName, (PathBuf, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    let loaded_file = &project.files[file_path];
    let file_src = &loaded_file.named_source;
    let file_dir = file_path.parent().unwrap_or_else(|| Path::new("."));

    // Build ImportedValueNames and imported_values from this file's import declarations.
    let mut imported_names = ImportedValueNames::default();
    let mut imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)> = HashMap::new();
    // Track imported value categories for output (source order).
    let mut imported_source_order: Vec<(ScopedName, DeclCategory)> = Vec::new();
    // Track type-system declarations to import from dependency registries.
    let mut imported_type_system_names: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    // Module imports: module_name → (canonical_path, span).
    let mut module_map: HashMap<String, (PathBuf, Span)> = HashMap::new();
    // Track extra RegistryBuilder entries to merge from dependencies.
    let mut extra_registry_builders: Vec<&Registry> = Vec::new();
    // Deferred instantiated imports (processed after lowering).
    let mut deferred_instantiated: Vec<DeferredInstantiatedImport> = Vec::new();

    for decl in &loaded_file.ast.declarations {
        if let DeclKind::Import(import_decl) = &decl.kind {
            let import_canonical =
                resolve_import_to_canonical(&import_decl.path, file_dir, project, file_src)?;

            // Instantiated import: defer to post-lowering IR merging.
            if !import_decl.param_bindings.is_empty() {
                let dep_loaded = &project.files[&import_canonical];

                // Determine the prefix (namespace) for the merged declarations.
                let prefix = match &import_decl.kind {
                    graphcal_syntax::ast::ImportKind::Module { alias } => {
                        if let Some(alias_ident) = alias {
                            alias_ident.name.clone()
                        } else {
                            derive_module_name_from_import_path(&import_decl.path, file_src)?
                        }
                    }
                    graphcal_syntax::ast::ImportKind::Selective(_) => {
                        // For selective instantiated imports, we still need a prefix
                        // for the merged declarations. Derive from filename.
                        derive_module_name_from_import_path(&import_decl.path, file_src)?
                    }
                };

                // Check for duplicate module names (instantiated imports occupy the same namespace).
                if let Some((_, first_span)) = module_map.get(&prefix) {
                    return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                        name: prefix,
                        src: file_src.clone(),
                        span: import_decl.path.span().into(),
                        first: (*first_span).into(),
                    }));
                }
                module_map.insert(
                    prefix.clone(),
                    (import_canonical.clone(), import_decl.path.span()),
                );

                // Validate param bindings against the dependency's AST.
                let mut bindings = HashMap::new();
                for binding in &import_decl.param_bindings {
                    let binding_name = &binding.name.name;
                    // Check that the binding name is a param in the dependency.
                    let is_param = dep_loaded.ast.declarations.iter().any(|d| {
                        matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == binding_name)
                    });
                    if !is_param {
                        // Check if it's some other kind of declaration.
                        let actual_kind = dep_loaded.ast.declarations.iter().find_map(|d| match &d
                            .kind
                        {
                            DeclKind::Node(n) if n.name.value.as_str() == binding_name => {
                                Some("node")
                            }
                            DeclKind::Const(c) if c.name.value.as_str() == binding_name => {
                                Some("const")
                            }
                            DeclKind::Assert(a) if a.name.value.as_str() == binding_name => {
                                Some("assert")
                            }
                            DeclKind::Fn(f) if f.name.value.as_str() == binding_name => Some("fn"),
                            _ => None,
                        });
                        if let Some(kind) = actual_kind {
                            return Err(CompileError::Eval(GraphcalError::BindingNotAParam {
                                name: binding_name.clone(),
                                actual_kind: kind.to_string(),
                                src: file_src.clone(),
                                span: binding.name.span.into(),
                            }));
                        }
                        return Err(CompileError::Eval(GraphcalError::UnknownParamBinding {
                            name: binding_name.clone(),
                            file_path: import_decl.path.display_path(),
                            src: file_src.clone(),
                            span: binding.name.span.into(),
                        }));
                    }
                    bindings.insert(binding_name.clone(), binding.value.clone());
                }

                // Register the dependency's declaration names in the importer's scope
                // so that the resolver recognizes references to them.
                let selective_names = match &import_decl.kind {
                    graphcal_syntax::ast::ImportKind::Selective(names) => {
                        let mut selective = Vec::new();
                        for import_item in names {
                            let orig_name = &import_item.name.name;
                            let local_name = import_item.local_name().to_string();

                            // Verify the name exists in the dependency.
                            if !file_has_declaration(&dep_loaded.ast, orig_name) {
                                return Err(CompileError::Eval(
                                    GraphcalError::ImportNameNotFound {
                                        name: orig_name.clone(),
                                        file_path: import_decl.path.display_path(),
                                        src: file_src.clone(),
                                        span: import_item.name.span.into(),
                                    },
                                ));
                            }

                            // Register the local name in scope for the resolver.
                            // Determine the category from the dep's AST.
                            let is_const = dep_loaded.ast.declarations.iter().any(|d| {
                                matches!(&d.kind, DeclKind::Const(c) if c.name.value.as_str() == orig_name)
                            });
                            let is_runtime = dep_loaded.ast.declarations.iter().any(|d| {
                                matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                                    || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name)
                            });
                            let scoped = ScopedName::Local(local_name.clone());
                            let span = import_item.name.span;
                            if is_const {
                                imported_names.const_names.push((scoped, span));
                            } else if is_runtime {
                                imported_names.param_names.push((scoped, span));
                            } else {
                                // Type-system declarations (dim/unit/index/type) are not
                                // registered in imported_names; handled via registry merge.
                            }
                            // Type-system declarations from instantiated imports also need registration.
                            let is_type_system =
                                dep_loaded.ast.declarations.iter().any(|d| match &d.kind {
                                    DeclKind::Dimension(dim) => {
                                        dim.name.value.as_str() == orig_name
                                    }
                                    DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
                                    DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
                                    DeclKind::Type(t) => t.name.value.as_str() == orig_name,
                                    _ => false,
                                });
                            if is_type_system {
                                imported_type_system_names
                                    .entry(import_canonical.clone())
                                    .or_default()
                                    .insert(orig_name.clone());
                            }

                            selective.push((orig_name.clone(), local_name));
                        }
                        Some(selective)
                    }
                    graphcal_syntax::ast::ImportKind::Module { .. } => {
                        // Register all dep names under the prefix for scope checking.
                        let import_span = import_decl.path.span();
                        for dep_decl in &dep_loaded.ast.declarations {
                            let (dep_name, is_const) = match &dep_decl.kind {
                                DeclKind::Const(c) => (Some(c.name.value.to_string()), true),
                                DeclKind::Param(p) => (Some(p.name.value.to_string()), false),
                                DeclKind::Node(n) => (Some(n.name.value.to_string()), false),
                                _ => (None, false),
                            };
                            if let Some(name) = dep_name {
                                let scoped = ScopedName::Qualified {
                                    module: prefix.clone(),
                                    member: name,
                                };
                                if is_const {
                                    imported_names.const_names.push((scoped, import_span));
                                } else {
                                    imported_names.param_names.push((scoped, import_span));
                                }
                            }
                        }
                        // Import type-system declarations.
                        if let Some(dep_eval) = evaluated_files.get(&import_canonical) {
                            extra_registry_builders.push(&dep_eval.registry);
                        }
                        None
                    }
                };

                // Strict check: when any binding is provided, ALL params of the
                // imported file must be explicitly bound unless #[allow_defaults].
                let allow_defaults = decl
                    .attributes
                    .iter()
                    .any(|a| a.name.name == "allow_defaults");
                if !allow_defaults {
                    for dep_decl in &dep_loaded.ast.declarations {
                        if let DeclKind::Param(p) = &dep_decl.kind
                            && p.value.is_some()
                            && !bindings.contains_key(p.name.value.as_str())
                        {
                            return Err(CompileError::Eval(
                                GraphcalError::DefaultParamNotProvided {
                                    name: p.name.value.to_string(),
                                    src: file_src.clone(),
                                    span: import_decl.path.span().into(),
                                    help: format!(
                                        "provide `{name} = <value>` in the import binding or add `#[allow_defaults]` to the import",
                                        name = p.name.value,
                                    ),
                                },
                            ));
                        }
                    }
                }

                deferred_instantiated.push(DeferredInstantiatedImport {
                    dep_path: import_canonical,
                    prefix,
                    bindings,
                    selective_names,
                    import_span: decl.span,
                });
                continue;
            }

            let dep = evaluated_files.get(&import_canonical).ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!(
                        "internal: dependency {} not yet evaluated",
                        import_canonical.display()
                    ),
                    src: file_src.clone(),
                    span: import_decl.path.span().into(),
                })
            })?;

            match &import_decl.kind {
                graphcal_syntax::ast::ImportKind::Selective(names) => {
                    for import_item in names {
                        let orig_name = &import_item.name.name;
                        let local_name = import_item.local_name().to_string();

                        // Check if it's a value (const/param/node) or type-system decl.
                        if let Some(rv) = dep.const_values.get(orig_name) {
                            let scoped = ScopedName::Local(local_name.clone());
                            imported_names
                                .const_names
                                .push((scoped.clone(), import_item.name.span));
                            let dt = dep.declared_types.get(orig_name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_source_order.push((scoped.clone(), DeclCategory::Const));
                            imported_values.insert(scoped, (rv.clone(), dt));
                        } else if let Some(rv) = dep.values.get(orig_name) {
                            let scoped = ScopedName::Local(local_name.clone());
                            let span = import_item.name.span;
                            imported_names.param_names.push((scoped.clone(), span));
                            let dt = dep.declared_types.get(orig_name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_source_order.push((scoped.clone(), DeclCategory::Param));
                            imported_values.insert(scoped, (rv.clone(), dt));
                        } else if let Some(fn_entry) =
                            dep.functions.iter().find(|entry| entry.name == *orig_name)
                        {
                            imported_names.functions.push(
                                graphcal_registry::resolve_types::ResolvedFunctionEntry {
                                    name: local_name,
                                    decl: fn_entry.decl.clone(),
                                    span: fn_entry.span,
                                },
                            );
                        } else if dep.has_assert(orig_name) {
                            // Assert is already evaluated in the dep file.
                            // We just need to make the name visible for #[assumes].
                            imported_names
                                .assert_names
                                .push((local_name, import_item.name.span));
                        } else {
                            // Check if it's a type-system declaration in the dep's file.
                            let dep_loaded = &project.files[&import_canonical];
                            if file_has_declaration(&dep_loaded.ast, orig_name) {
                                // Type-system declaration (dim/unit/index/type).
                                imported_type_system_names
                                    .entry(import_canonical.clone())
                                    .or_default()
                                    .insert(orig_name.clone());
                            } else {
                                return Err(CompileError::Eval(
                                    GraphcalError::ImportNameNotFound {
                                        name: orig_name.clone(),
                                        file_path: import_decl.path.display_path(),
                                        src: file_src.clone(),
                                        span: import_item.name.span.into(),
                                    },
                                ));
                            }
                        }
                    }
                }
                graphcal_syntax::ast::ImportKind::Module { alias } => {
                    let module_name = if let Some(alias_ident) = alias {
                        alias_ident.name.clone()
                    } else {
                        derive_module_name_from_import_path(&import_decl.path, file_src)?
                    };
                    if let Some((_, first_span)) = module_map.get(&module_name) {
                        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                            name: module_name,
                            src: file_src.clone(),
                            span: import_decl.path.span().into(),
                            first: (*first_span).into(),
                        }));
                    }
                    module_map.insert(
                        module_name.clone(),
                        (import_canonical.clone(), import_decl.path.span()),
                    );

                    // Import all values under module::name prefix.
                    let import_span = import_decl.path.span();
                    for (name, rv) in &dep.const_values {
                        let scoped = ScopedName::Qualified {
                            module: module_name.clone(),
                            member: name.clone(),
                        };
                        imported_names
                            .const_names
                            .push((scoped.clone(), import_span));
                        let dt =
                            dep.declared_types
                                .get(name)
                                .cloned()
                                .unwrap_or(DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ));
                        imported_source_order.push((scoped.clone(), DeclCategory::Const));
                        imported_values.insert(scoped, (rv.clone(), dt));
                    }
                    for (name, rv) in &dep.values {
                        let scoped = ScopedName::Qualified {
                            module: module_name.clone(),
                            member: name.clone(),
                        };
                        imported_names
                            .param_names
                            .push((scoped.clone(), import_span));
                        let dt =
                            dep.declared_types
                                .get(name)
                                .cloned()
                                .unwrap_or(DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ));
                        imported_source_order.push((scoped.clone(), DeclCategory::Param));
                        imported_values.insert(scoped, (rv.clone(), dt));
                    }
                    for fn_entry in &dep.functions {
                        let flat = format!("{module_name}::{}", fn_entry.name);
                        imported_names.functions.push(
                            graphcal_registry::resolve_types::ResolvedFunctionEntry {
                                name: flat,
                                decl: fn_entry.decl.clone(),
                                span: fn_entry.span,
                            },
                        );
                    }
                    // Import all type-system declarations from dep's registry.
                    extra_registry_builders.push(&dep.registry);
                }
            }
        }
    }

    // For module imports, resolve qualified references in expressions.
    let file_ast = if module_map.is_empty() {
        std::borrow::Cow::Borrowed(&loaded_file.ast)
    } else {
        let mut ast = loaded_file.ast.clone();
        for decl in &mut ast.declarations {
            match &mut decl.kind {
                DeclKind::Const(c) => rewrite_qualified_refs(&mut c.value),
                DeclKind::Param(p) => {
                    if let Some(ref mut value) = p.value {
                        rewrite_qualified_refs(value);
                    }
                }
                DeclKind::Node(n) => rewrite_qualified_refs(&mut n.value),
                DeclKind::Assert(a) => match &mut a.body {
                    graphcal_syntax::ast::AssertBody::Expr(e) => rewrite_qualified_refs(e),
                    graphcal_syntax::ast::AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => {
                        rewrite_qualified_refs(actual);
                        rewrite_qualified_refs(expected);
                        rewrite_qualified_refs(tolerance);
                    }
                },
                DeclKind::Fn(f) => match &mut f.body {
                    graphcal_syntax::ast::FnBody::Short(e) => rewrite_qualified_refs(e),
                    graphcal_syntax::ast::FnBody::Block { stmts, expr } => {
                        for stmt in stmts {
                            rewrite_qualified_refs(&mut stmt.value);
                        }
                        rewrite_qualified_refs(expr);
                    }
                },
                _ => {}
            }
        }
        // Also rewrite qualified refs in param binding expressions.
        for decl in &mut ast.declarations {
            if let DeclKind::Import(import_decl) = &mut decl.kind {
                for binding in &mut import_decl.param_bindings {
                    rewrite_qualified_refs(&mut binding.value);
                }
            }
        }
        std::borrow::Cow::Owned(ast)
    };

    // Lower to IR using per-file evaluation path.
    let saved_imported_values = imported_values.clone();

    let (mut builder, mut unfrozen) = crate::ir::lower_to_builder_with_imported_values(
        &file_ast,
        file_src,
        &imported_names,
        imported_values,
    )?;

    // Register type-system declarations from selectively imported files.
    for (dep_path, names) in &imported_type_system_names {
        let dep_loaded = &project.files[dep_path];
        crate::ir::register_selected_declarations(
            &dep_loaded.ast,
            &mut builder,
            &dep_loaded.named_source,
            names,
        )?;
    }

    // Merge type-system declarations from module-imported registries.
    for dep_registry in &extra_registry_builders {
        merge_registry_into_builder(&mut builder, dep_registry);
    }

    // Process deferred instantiated imports: compile dep to IR and merge.
    for deferred in &deferred_instantiated {
        let dep_loaded = &project.files[&deferred.dep_path];
        let dep_src = &dep_loaded.named_source;

        // Build imported values for the dependency from its own transitive imports.
        let dep_imported = build_dep_imported_values(project, &deferred.dep_path, evaluated_files)?;

        // Compile the dependency to IR.
        let (dep_builder, dep_unfrozen) = crate::ir::lower_to_builder_with_imported_values(
            &dep_loaded.ast,
            dep_src,
            &dep_imported.0,
            dep_imported.1,
        )?;

        // Merge the dependency's type-system declarations into the importer's registry.
        let dep_registry = dep_builder.build();
        merge_registry_into_builder(&mut builder, &dep_registry);

        // Collect all declaration names in the dependency (for prefix_expr_refs).
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.clone());
        }
        // Also include function names.
        for fn_entry in &dep_unfrozen.functions {
            dep_names.insert(fn_entry.name.clone());
        }

        // Merge the dependency's IR into the importer's IR.
        unfrozen.merge_dependency(
            dep_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
        );

        // For selective instantiated imports, add alias nodes that reference
        // the prefixed declarations. E.g., `delta_v` → `@prefix::delta_v`.
        if let Some(selective) = &deferred.selective_names {
            for (orig_name, local_name) in selective {
                let prefixed_name = format!("{}::{}", deferred.prefix, orig_name);

                // Find the type annotation from the dependency's AST.
                let type_ann = dep_loaded
                    .ast
                    .declarations
                    .iter()
                    .find_map(|d| match &d.kind {
                        DeclKind::Const(c) if c.name.value.as_str() == orig_name => {
                            Some(c.type_ann.clone())
                        }
                        DeclKind::Param(p) if p.name.value.as_str() == orig_name => {
                            Some(p.type_ann.clone())
                        }
                        DeclKind::Node(n) if n.name.value.as_str() == orig_name => {
                            Some(n.type_ann.clone())
                        }
                        _ => None,
                    });

                if let Some(type_ann) = type_ann {
                    // Determine if this is a const or runtime declaration.
                    let is_const = dep_loaded.ast.declarations.iter().any(|d| {
                        matches!(&d.kind, DeclKind::Const(c) if c.name.value.as_str() == orig_name)
                    });

                    // Create an alias expression: `@prefix::orig_name` (or `PREFIX::CONST`)
                    let alias_expr = if is_const {
                        Expr {
                            kind: ExprKind::ConstRef(Spanned::new(
                                DeclName::new(&prefixed_name),
                                deferred.import_span,
                            )),
                            span: deferred.import_span,
                        }
                    } else {
                        Expr {
                            kind: ExprKind::GraphRef(Spanned::new(
                                DeclName::new(&prefixed_name),
                                deferred.import_span,
                            )),
                            span: deferred.import_span,
                        }
                    };

                    // Add the alias as a declaration in the importer's IR.
                    if is_const {
                        unfrozen.add_const_alias(
                            local_name.clone(),
                            type_ann,
                            alias_expr,
                            deferred.import_span,
                            prefixed_name,
                        );
                    } else {
                        unfrozen.add_node_alias(
                            local_name.clone(),
                            type_ann,
                            alias_expr,
                            deferred.import_span,
                            prefixed_name,
                        );
                    }
                }
            }
        }
    }

    let ir = unfrozen.freeze(builder.build());

    // Apply overrides routed to this file (using original param names).
    let mut ir = ir;
    let file_overrides: HashMap<DeclName, graphcal_syntax::ast::Expr> = override_targets
        .iter()
        .filter(|(_, (target_path, _))| target_path == file_path)
        .map(|(name, (_, orig_name))| (orig_name.clone(), overrides[name].clone()))
        .collect();
    if !file_overrides.is_empty() {
        apply_overrides(&mut ir, &file_overrides)?;
    }

    // Type-resolve, check dimensions.
    let tir = crate::tir::type_resolve(ir, file_src)?;
    crate::fn_check::check_no_recursion(&tir.registry.functions, file_src)?;
    crate::dim_check::check_dimensions_tir(&tir, file_src)?;

    let declared_types = tir.build_declared_types(file_src)?;

    for (override_name, override_expr) in &file_overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
            &tir.registry,
            &tir.resolved_fn_sigs,
            file_src,
        )?;
    }

    Ok(CompiledFile {
        tir,
        declared_types,
        imported_values: saved_imported_values,
        imported_source_order,
    })
}

/// Build imported value names and values for a dependency file from its own transitive imports.
///
/// This mirrors the import-processing logic in `compile_single_file_in_project` but
/// only for non-instantiated imports (the dependency's own transitive deps are already
/// evaluated and stored in `evaluated_files`).
#[expect(
    clippy::too_many_lines,
    reason = "mirrors compile_single_file_in_project import logic for transitive deps"
)]
fn build_dep_imported_values(
    project: &crate::loader::LoadedProject,
    dep_path: &Path,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
) -> Result<DepImportedValues, CompileError> {
    let dep_loaded = &project.files[dep_path];
    let dep_src = &dep_loaded.named_source;
    let dep_dir = dep_path.parent().unwrap_or_else(|| Path::new("."));

    let mut imported_names = ImportedValueNames::default();
    let mut imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)> = HashMap::new();

    for decl in &dep_loaded.ast.declarations {
        if let DeclKind::Import(import_decl) = &decl.kind {
            // Only handle non-instantiated imports (transitive deps are pre-evaluated).
            if !import_decl.param_bindings.is_empty() {
                // Nested instantiated imports are not supported in this initial implementation.
                // The dependency itself would need to be compiled with instantiation support.
                // For now, return an error.
                return Err(CompileError::Eval(GraphcalError::EvalError {
                    message: "nested instantiated imports are not yet supported".to_string(),
                    src: dep_src.clone(),
                    span: import_decl.path.span().into(),
                }));
            }

            let trans_canonical =
                resolve_import_to_canonical(&import_decl.path, dep_dir, project, dep_src)?;

            let trans_dep = evaluated_files.get(&trans_canonical).ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!(
                        "internal: transitive dependency {} not yet evaluated",
                        trans_canonical.display()
                    ),
                    src: dep_src.clone(),
                    span: import_decl.path.span().into(),
                })
            })?;

            match &import_decl.kind {
                graphcal_syntax::ast::ImportKind::Selective(names) => {
                    for import_item in names {
                        let orig_name = &import_item.name.name;
                        let local_name = import_item.local_name().to_string();

                        if let Some(rv) = trans_dep.const_values.get(orig_name) {
                            let scoped = ScopedName::Local(local_name);
                            imported_names
                                .const_names
                                .push((scoped.clone(), import_item.name.span));
                            let dt = trans_dep.declared_types.get(orig_name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_values.insert(scoped, (rv.clone(), dt));
                        } else if let Some(rv) = trans_dep.values.get(orig_name) {
                            let scoped = ScopedName::Local(local_name);
                            imported_names
                                .param_names
                                .push((scoped.clone(), import_item.name.span));
                            let dt = trans_dep.declared_types.get(orig_name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_values.insert(scoped, (rv.clone(), dt));
                        } else if let Some(fn_entry) = trans_dep
                            .functions
                            .iter()
                            .find(|entry| entry.name == *orig_name)
                        {
                            imported_names.functions.push(
                                graphcal_registry::resolve_types::ResolvedFunctionEntry {
                                    name: local_name,
                                    decl: fn_entry.decl.clone(),
                                    span: fn_entry.span,
                                },
                            );
                        } else {
                            // Type-system declarations are handled by the registry, not imported_values.
                        }
                    }
                }
                graphcal_syntax::ast::ImportKind::Module { alias } => {
                    let module_name = alias.as_ref().map_or_else(
                        || {
                            derive_module_name_from_import_path(&import_decl.path, dep_src)
                                .unwrap_or_else(|_| "dep".to_string())
                        },
                        |alias_ident| alias_ident.name.clone(),
                    );
                    let import_span = import_decl.path.span();
                    for (name, rv) in &trans_dep.const_values {
                        let scoped = ScopedName::Qualified {
                            module: module_name.clone(),
                            member: name.clone(),
                        };
                        imported_names
                            .const_names
                            .push((scoped.clone(), import_span));
                        let dt = trans_dep.declared_types.get(name).cloned().unwrap_or(
                            DeclaredType::Scalar(
                                graphcal_syntax::dimension::Dimension::dimensionless(),
                            ),
                        );
                        imported_values.insert(scoped, (rv.clone(), dt));
                    }
                    for (name, rv) in &trans_dep.values {
                        let scoped = ScopedName::Qualified {
                            module: module_name.clone(),
                            member: name.clone(),
                        };
                        imported_names
                            .param_names
                            .push((scoped.clone(), import_span));
                        let dt = trans_dep.declared_types.get(name).cloned().unwrap_or(
                            DeclaredType::Scalar(
                                graphcal_syntax::dimension::Dimension::dimensionless(),
                            ),
                        );
                        imported_values.insert(scoped, (rv.clone(), dt));
                    }
                    for fn_entry in &trans_dep.functions {
                        let flat = format!("{module_name}::{}", fn_entry.name);
                        imported_names.functions.push(
                            graphcal_registry::resolve_types::ResolvedFunctionEntry {
                                name: flat,
                                decl: fn_entry.decl.clone(),
                                span: fn_entry.span,
                            },
                        );
                    }
                }
            }
        }
    }

    Ok((imported_names, imported_values))
}

/// Evaluate and store a non-root file, producing an [`EvaluatedFile`] for downstream imports.
fn evaluate_and_store_file(
    compiled: CompiledFile,
    file_path: &Path,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &mut HashMap<PathBuf, EvaluatedFile>,
) -> Result<(), CompileError> {
    let plan = crate::exec_plan::compile(&compiled.tir, file_src)?;
    let eval_result = evaluate_plan(&compiled.tir, &plan, &compiled.declared_types, file_src);
    let file_runtime_values =
        extract_runtime_values(&compiled.tir, &plan, &compiled.declared_types, file_src);

    evaluated_files.insert(
        file_path.to_path_buf(),
        EvaluatedFile {
            values: file_runtime_values,
            const_values: plan.const_values,
            declared_types: compiled.declared_types,
            assertions: eval_result
                .assertions
                .into_iter()
                .map(|(name, result, span)| (name, (result, span)))
                .collect(),
            registry: compiled.tir.registry,
            functions: compiled.tir.functions,
        },
    );
    Ok(())
}

/// Evaluate a project using per-file evaluation.
///
/// Each file is compiled and evaluated as an independent unit, in topological
/// order (dependencies first). Import declarations bind pre-evaluated values
/// from dependency files into the importing file's scope.
///
/// All assertions in all files are evaluated and aggregated.
#[expect(
    clippy::too_many_lines,
    reason = "sequential per-file evaluation steps"
)]
fn evaluate_project_perfile(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    allow_defaults: bool,
) -> Result<EvalResult, CompileError> {
    // Pre-compute override routing: map each override name to the file that owns
    // the param. Walk root file's imports to find the owning file for each override.
    let override_targets = route_overrides_to_files(project, overrides)?;

    // Strict param check: when overrides are provided and --allow-defaults is not set,
    // all overridable params (root file + selectively imported) must be explicitly provided.
    if !overrides.is_empty() && !allow_defaults {
        let root_file = &project.files[&project.root];
        let root_dir = project.root.parent().unwrap_or_else(|| Path::new("."));
        let root_src = &root_file.named_source;

        // Check root file's own params
        for decl in &root_file.ast.declarations {
            if let DeclKind::Param(p) = &decl.kind
                && p.value.is_some()
            {
                let is_overridden = override_targets.values().any(|(target_path, orig_name)| {
                    *target_path == project.root && orig_name.as_str() == p.name.value.as_str()
                });
                if !is_overridden {
                    return Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided {
                        name: p.name.value.to_string(),
                        src: root_src.clone(),
                        span: decl.span.into(),
                        help: format!(
                            "provide via `--set '{name}=<value>'` or use `--allow-defaults`",
                            name = p.name.value,
                        ),
                    }));
                }
            }
        }

        // Check params from non-parameterized selective imports
        for decl in &root_file.ast.declarations {
            if let DeclKind::Import(import_decl) = &decl.kind {
                // Only check non-parameterized imports (parameterized have their own check)
                if !import_decl.param_bindings.is_empty() {
                    continue;
                }
                if let graphcal_syntax::ast::ImportKind::Selective(names) = &import_decl.kind {
                    let import_canonical = resolve_import_to_canonical(
                        &import_decl.path,
                        root_dir,
                        project,
                        root_src,
                    )?;
                    let dep_file = &project.files[&import_canonical];
                    let dep_src = &dep_file.named_source;

                    // For each param in the dep that is selectively imported
                    for item in names {
                        let orig_name = item.name.name.as_str();
                        // Find the param declaration in the dep
                        for dep_decl in &dep_file.ast.declarations {
                            if let DeclKind::Param(p) = &dep_decl.kind
                                && p.name.value.as_str() == orig_name
                                && p.value.is_some()
                            {
                                let local_name = item.local_name();
                                let is_overridden =
                                    overrides.keys().any(|k| k.as_str() == local_name);
                                if !is_overridden {
                                    return Err(CompileError::Eval(
                                        GraphcalError::DefaultParamNotProvided {
                                            name: local_name.to_string(),
                                            src: dep_src.clone(),
                                            span: dep_decl.span.into(),
                                            help: format!(
                                                "provide via `--set '{local_name}=<value>'` or use `--allow-defaults`",
                                            ),
                                        },
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut evaluated_files: HashMap<PathBuf, EvaluatedFile> = HashMap::new();

    for file_path in &project.load_order {
        let is_root = *file_path == project.root;
        let compiled = compile_single_file_in_project(
            project,
            file_path,
            &evaluated_files,
            overrides,
            &override_targets,
        )?;

        // Files with required params (no default) cannot be evaluated standalone.
        // They are only consumed via instantiated imports where `merge_dependency`
        // provides the bindings. Skip standalone evaluation for these files.
        let has_required_params = compiled
            .tir
            .params
            .iter()
            .any(|entry| entry.default_expr.is_none());

        if !is_root && has_required_params {
            continue;
        }

        if is_root {
            let file_src = &project.files[file_path].named_source;
            let plan = crate::exec_plan::compile(&compiled.tir, file_src)?;
            let eval_result =
                evaluate_plan(&compiled.tir, &plan, &compiled.declared_types, file_src);

            // Build a mapping from each dependency file path to the root-level
            // import statement span that (directly or transitively) brought it in.
            let dep_import_spans = build_dep_import_spans(project);

            // Aggregate assertions from all dependency files, replacing the
            // assertion's original span with the root file's import statement span.
            let mut all_assertions: Vec<(DeclName, AssertResult, Span)> = Vec::new();
            for dep_path in &project.load_order {
                if *dep_path == project.root {
                    continue;
                }
                if let Some(dep_eval) = evaluated_files.get(dep_path) {
                    let import_span = dep_import_spans
                        .get(dep_path)
                        .copied()
                        .unwrap_or(Span::new(0, 0));
                    all_assertions.extend(dep_eval.assertions.iter().map(
                        |(name, (result, _span))| (name.clone(), result.clone(), import_span),
                    ));
                }
            }
            all_assertions.extend(eval_result.assertions);

            // Prepend imported values to the output so they appear in the
            // result just like in the old single-IR approach.
            let mut all_consts = Vec::new();
            let mut all_params = Vec::new();
            let mut all_all = Vec::new();

            for (name, cat) in &compiled.imported_source_order {
                if let Some((rv, dt)) = compiled.imported_values.get(name) {
                    let value =
                        super::runtime::runtime_to_value(rv, Some(dt), &compiled.tir.registry);
                    let decl_name = DeclName::new(name.to_string());
                    match cat {
                        DeclCategory::Const => {
                            all_consts.push((decl_name.clone(), value.clone()));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Const));
                        }
                        DeclCategory::Param => {
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Param));
                        }
                        DeclCategory::Node => {
                            // Imported nodes appear as params in the output.
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Node));
                        }
                        DeclCategory::Assert | DeclCategory::Plot | DeclCategory::Figure => {}
                    }
                }
            }

            all_consts.extend(eval_result.consts);
            all_params.extend(eval_result.params);
            let all_nodes = eval_result.nodes;
            all_all.extend(eval_result.all);

            return Ok(EvalResult {
                consts: all_consts,
                params: all_params,
                nodes: all_nodes,
                all: all_all,
                assertions: all_assertions,
                plots: eval_result.plots,
                figures: eval_result.figures,
                assumes_map: eval_result.assumes_map,
                base_dim_symbols: eval_result.base_dim_symbols,
                domain_constraints: eval_result.domain_constraints,
            });
        }

        let file_src = &project.files[file_path].named_source;
        evaluate_and_store_file(compiled, file_path, file_src, &mut evaluated_files)?;
    }

    // Should not reach here — root file should have returned above.
    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: NamedSource::new("internal", Arc::new(String::new())),
        span: (0, 0).into(),
    }))
}

/// Map each dependency file to the root-level import statement span that brought it in.
///
/// Direct imports get the span of their own `import` declaration in the root file.
/// Transitive imports inherit the root-level import span of the direct import
/// that started the chain.
fn build_dep_import_spans(project: &crate::loader::LoadedProject) -> HashMap<PathBuf, Span> {
    let root_file = &project.files[&project.root];
    let root_dir = project.root.parent().unwrap_or_else(|| Path::new("."));
    let mut spans: HashMap<PathBuf, Span> = HashMap::new();

    // Map root's direct imports.
    for decl in &root_file.ast.declarations {
        if let DeclKind::Import(import_decl) = &decl.kind {
            // Use the helper to resolve the import path, ignoring errors (best-effort)
            if let Ok(canonical) = resolve_import_to_canonical(
                &import_decl.path,
                root_dir,
                project,
                &root_file.named_source,
            ) {
                spans.entry(canonical).or_insert(decl.span);
            }
        }
    }

    // For transitive deps: walk load_order (topological, deps first).
    // If a dep is not yet mapped, find which already-mapped file imports it
    // and inherit that file's root-level span.
    for file_path in &project.load_order {
        if *file_path == project.root || spans.contains_key(file_path) {
            continue;
        }
        let mut found = false;
        for (mapped_path, root_span) in &spans.clone() {
            if let Some(mapped_file) = project.files.get(mapped_path) {
                let dir = mapped_path.parent().unwrap_or_else(|| Path::new("."));
                for decl in &mapped_file.ast.declarations {
                    if let DeclKind::Import(imp) = &decl.kind
                        && let Ok(c) = resolve_import_to_canonical(
                            &imp.path,
                            dir,
                            project,
                            &mapped_file.named_source,
                        )
                        && c == *file_path
                    {
                        spans.insert(file_path.clone(), *root_span);
                        found = true;
                        break;
                    }
                }
            }
            if found {
                break;
            }
        }
    }

    spans
}

/// Compile a project to TIR using per-file evaluation.
///
/// Non-root files are fully evaluated to produce `RuntimeValue`s for downstream
/// imports. The root file stops at TIR and returns it.
fn compile_to_tir_project_perfile(
    project: &crate::loader::LoadedProject,
) -> Result<crate::tir::TIR, CompileError> {
    let empty_overrides = HashMap::new();
    let empty_targets = HashMap::new();
    let mut evaluated_files: HashMap<PathBuf, EvaluatedFile> = HashMap::new();

    for file_path in &project.load_order {
        let is_root = *file_path == project.root;
        let compiled = compile_single_file_in_project(
            project,
            file_path,
            &evaluated_files,
            &empty_overrides,
            &empty_targets,
        )?;

        if is_root {
            return Ok(compiled.tir);
        }

        // Skip standalone evaluation for files with required params (no default).
        let has_required_params = compiled.tir.params.iter().any(|e| e.default_expr.is_none());
        if has_required_params {
            continue;
        }

        let file_src = &project.files[file_path].named_source;
        evaluate_and_store_file(compiled, file_path, file_src, &mut evaluated_files)?;
    }

    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: NamedSource::new("internal", Arc::new(String::new())),
        span: (0, 0).into(),
    }))
}

/// Route `--set` / `--input` overrides to the files that own the targeted params.
///
/// Returns a map: `override_name` → (`owning_file_path`, `original_param_name`).
/// The `original_param_name` may differ from `override_name` when an alias is used.
fn route_overrides_to_files(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<HashMap<DeclName, (PathBuf, DeclName)>, CompileError> {
    if overrides.is_empty() {
        return Ok(HashMap::new());
    }

    let root_file = &project.files[&project.root];
    let root_dir = project.root.parent().unwrap_or_else(|| Path::new("."));
    let root_src = &root_file.named_source;

    let mut result: HashMap<DeclName, (PathBuf, DeclName)> = HashMap::new();

    for override_name in overrides.keys() {
        let name_str = override_name.as_str();

        // Check if the root file itself declares this param.
        let found_in_root =
            root_file.ast.declarations.iter().any(
                |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == name_str),
            );
        if found_in_root {
            result.insert(
                override_name.clone(),
                (project.root.clone(), override_name.clone()),
            );
            continue;
        }

        // Check if the root file imports this param from a dependency.
        let mut found = false;
        for decl in &root_file.ast.declarations {
            if let DeclKind::Import(import_decl) = &decl.kind {
                if let graphcal_syntax::ast::ImportKind::Selective(names) = &import_decl.kind {
                    for item in names {
                        let local_name = item.local_name().to_string();
                        if local_name == name_str {
                            let orig_name = &item.name.name;
                            let import_canonical = resolve_import_to_canonical(
                                &import_decl.path,
                                root_dir,
                                project,
                                root_src,
                            )?;

                            // Verify it's actually a param in the source file.
                            let dep_file = &project.files[&import_canonical];
                            let is_param = dep_file.ast.declarations.iter().any(|d| {
                                matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                            });
                            if is_param {
                                result.insert(
                                    override_name.clone(),
                                    (import_canonical, DeclName::new(orig_name.clone())),
                                );
                                found = true;
                                break;
                            }
                        }
                    }
                }
                if found {
                    break;
                }
            }
        }

        if !found {
            // Check if the name matches a non-param declaration (node, const, assert)
            // in the root file to provide a better error message.
            for decl in &root_file.ast.declarations {
                let kind = match &decl.kind {
                    DeclKind::Node(n) if n.name.value.as_str() == name_str => {
                        Some(DeclCategory::Node)
                    }
                    DeclKind::Const(c) if c.name.value.as_str() == name_str => {
                        Some(DeclCategory::Const)
                    }
                    DeclKind::Assert(a) if a.name.value.as_str() == name_str => {
                        Some(DeclCategory::Assert)
                    }
                    _ => None,
                };
                if let Some(actual_kind) = kind {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind,
                    }));
                }
            }
            return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                name: override_name.clone(),
            }));
        }
    }

    Ok(result)
}

/// Extract `RuntimeValue`s from a plan evaluation for passing to downstream files.
///
/// Delegates to the shared [`run_eval_loop`](super::runtime::run_eval_loop) and
/// filters the result to only locally-defined param/node values.
fn extract_runtime_values(
    tir: &crate::tir::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> HashMap<String, RuntimeValue> {
    let result = super::runtime::run_eval_loop(plan, tir, declared_types, src);

    // Return only locally-defined param/node values (not imported, not consts).
    let local_runtime_names: HashSet<&str> = tir
        .params
        .iter()
        .map(|e| e.name.as_str())
        .chain(tir.nodes.iter().map(|e| e.name.as_str()))
        .collect();

    result
        .values
        .into_iter()
        .filter(|(name, _)| local_runtime_names.contains(name.as_str()))
        .collect()
}

/// Merge type-system declarations from a dependency's frozen Registry into a builder.
///
/// This imports dimensions, units, indexes, and struct types so that the
/// importing file can reference them.
fn merge_registry_into_builder(builder: &mut RegistryBuilder, dep_registry: &Registry) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        builder.register_base_dimension(graphcal_syntax::names::DimName::new(name), id.clone());
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        builder.register_dimension(name.clone(), dim.clone());
    }

    // Import base dimension symbols (for SI unit string display).
    for (id, symbol) in dep_registry.dimensions.base_dim_symbols() {
        builder.set_base_dim_symbol(id.clone(), symbol.clone());
    }

    // Import units.
    for (name, dim, scale) in dep_registry.units.all_units() {
        builder.register_unit((*name).clone(), dim.clone(), *scale);
    }

    // Import indexes.
    for idx_def in dep_registry.indexes.all_indexes() {
        builder.register_index(idx_def.clone());
    }

    // Import struct types.
    for type_def in dep_registry.types.all_types() {
        builder.register_type(type_def.clone());
    }

    // Import functions.
    for fn_def in dep_registry.functions.all_functions() {
        builder.register_function(fn_def.clone());
    }
}

/// Validate and apply parameter overrides to an IR.
pub(super) fn apply_overrides(
    ir: &mut crate::ir::IR,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<(), CompileError> {
    for (override_name, override_expr) in overrides {
        let name_str = override_name.as_str();
        if let Some((_, cat)) = ir.source_order.iter().find(|(n, _)| n == name_str) {
            match cat {
                DeclCategory::Param => {}
                non_param_cat => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: *non_param_cat,
                    }));
                }
            }
        } else {
            return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                name: override_name.clone(),
            }));
        }

        if let Some(entry) = ir.params.iter_mut().find(|e| e.name == name_str) {
            entry.default_expr = Some(override_expr.clone());
        }

        let all_runtime: std::collections::HashSet<&str> = ir
            .params
            .iter()
            .map(|e| e.name.as_str())
            .chain(ir.nodes.iter().map(|e| e.name.as_str()))
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        ir.runtime_deps.insert(name_str.to_string(), graph_refs);
    }
    Ok(())
}

/// Compile a [`LoadedProject`](crate::loader::LoadedProject) to TIR without evaluating.
///
/// Resolves imports from `use` declarations in the root file, lowers to IR,
/// type-resolves, and runs all checks (recursion, dimensions). The project may
/// have been loaded from disk, constructed from in-memory source, or a mix of
/// both (via [`crate::io::OverlayFileSystem`](crate::io) + [`crate::loader::load_project`]).
///
/// # Errors
///
/// Returns a [`CompileError`] if lowering, resolution, or checking fails.
pub fn compile_to_tir_from_project(
    project: &crate::loader::LoadedProject,
) -> Result<crate::tir::TIR, CompileError> {
    compile_to_tir_project_perfile(project)
}

/// Compile and evaluate a [`LoadedProject`](crate::loader::LoadedProject).
///
/// Uses per-file evaluation: each file is compiled and evaluated independently
/// in topological order. Import declarations bind pre-evaluated values from
/// dependency files. All assertions in all files are evaluated and aggregated.
///
/// # Errors
///
/// Returns a [`CompileError`] if any pipeline stage fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_from_project(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    allow_defaults: bool,
) -> Result<EvalResult, CompileError> {
    evaluate_project_perfile(project, overrides, allow_defaults)
}

// ---------------------------------------------------------------------------
// Convenience wrappers: existing public API, now delegating to project-based core
// ---------------------------------------------------------------------------

/// Full pipeline for multi-file projects with parameter overrides.
///
/// Loads all files referenced by `use` declarations starting from `root_path`,
/// collects imported declarations, and evaluates the root file with imports merged.
///
/// All filesystem access goes through the provided [`crate::io::FileSystemReader`].
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or evaluation fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_project<F: crate::io::FileSystemReader>(
    root_path: &Path,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    project_root: Option<&Path>,
    allow_defaults: bool,
    fs: &F,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    compile_and_eval_from_project(&project, overrides, allow_defaults)
}

/// Compile source to TIR without evaluating.
///
/// Runs the pipeline up through type resolution, function recursion check, and
/// dimension check, but does not build an execution plan or evaluate. This is
/// useful for tooling (e.g., LSP) that needs type information without execution.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing, lowering, or checking fails.
pub fn compile_to_tir(source: &str, name: &str) -> Result<crate::tir::TIR, CompileError> {
    let project = crate::loader::LoadedProject::from_source(source, name)?;
    compile_to_tir_from_project(&project)
}

/// Compile a multi-file project to TIR without evaluating.
///
/// Loads all files referenced by `use` declarations starting from `root_path`,
/// resolves imports, and runs the pipeline up through dimension checking.
///
/// All filesystem access goes through the provided [`crate::io::FileSystemReader`].
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or checking fails.
pub fn compile_to_tir_project<F: crate::io::FileSystemReader>(
    root_path: &Path,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<(crate::tir::TIR, crate::loader::LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

/// Check whether a file contains a declaration with the given name.
///
/// Returns `true` if the file has a type-system declaration (dimension, unit,
/// index, or struct type) with that name. This is used as a fallback when a
/// selective import name is not found among the dependency's evaluated values
/// or functions.
pub(super) fn file_has_declaration(file: &graphcal_syntax::ast::File, name: &str) -> bool {
    file.declarations.iter().any(|decl| match &decl.kind {
        DeclKind::Const(c) => c.name.value.as_str() == name,
        DeclKind::Param(p) => p.name.value.as_str() == name,
        DeclKind::Node(n) => n.name.value.as_str() == name,
        DeclKind::Fn(f) => f.name.value.as_str() == name,
        DeclKind::Assert(a) => a.name.value.as_str() == name,
        DeclKind::Dimension(d) => d.name.value.as_str() == name,
        DeclKind::Unit(u) => u.name.value.as_str() == name,
        DeclKind::Index(idx) => idx.name.value.as_str() == name,
        DeclKind::Type(t) => t.name.value.as_str() == name,
        DeclKind::Plot(p) => p.name.value.as_str() == name,
        DeclKind::Figure(f) => f.name.value.as_str() == name,
        DeclKind::Import(_) => false,
    })
}

/// Resolve a struct field's declared type, handling generic type parameter substitution.
///
/// If the field's type annotation references a generic type parameter (e.g., `D` in
/// `Vec3<D: Dim, F: Type>`), the substitution map provides the concrete type.
/// Otherwise, falls back to direct registry resolution.
pub(super) fn resolve_field_declared_type(
    field: &crate::registry::StructField,
    generic_sub: &HashMap<&str, &DeclaredType>,
    registry: &Registry,
) -> Option<DeclaredType> {
    // Check if the field type is a bare generic param reference (e.g., `D`)
    if let graphcal_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &field.type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        let name = &dim_expr.terms[0].term.name.name;
        if let Some(concrete) = generic_sub.get(name.as_str()) {
            return Some((*concrete).clone());
        }
    }
    // Non-generic: resolve directly from the registry
    registry
        .dimensions
        .resolve_type_expr(&field.type_ann)
        .map(DeclaredType::Scalar)
}

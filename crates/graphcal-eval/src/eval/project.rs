//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::syntax::ast::{DeclKind, Expr, ExprKind, ImportPath};
use graphcal_compiler::syntax::names::{DeclName, FnName, Spanned};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::visitor::ExprVisitorMut;

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
            ExprKind::QualifiedFnCall {
                module,
                name,
                type_args,
                args,
            } => {
                let flat = FnName::new(format!("{}::{}", module.name, name.value));
                ExprKind::FnCall {
                    name: Spanned {
                        value: flat,
                        span: name.span,
                    },
                    type_args,
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
    functions: Vec<graphcal_compiler::ir::ir::FunctionEntry>,
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
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    index_bindings: HashMap<String, String>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports (all names are accessible via `prefix::`).
    selective_names: Option<Vec<(String, String)>>, // (orig_name, local_name)
    /// Span of the import declaration (for diagnostics).
    import_span: Span,
    /// Per-import-item attributes (e.g., `#[expected_fail(...)]` on imported assertions).
    /// Key = original name in dep, Value = list of attributes from the import item.
    import_item_attributes: HashMap<String, Vec<graphcal_compiler::syntax::ast::Attribute>>,
}

/// Mutable state accumulated while processing import declarations.
///
/// Bundles the various collections that [`compile_single_file_in_project`] builds
/// during its import-processing loop, avoiding excessive parameter counts in the
/// extracted helper functions.
struct ImportContext<'a> {
    imported_names: ImportedValueNames,
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
    imported_type_system_names: HashMap<PathBuf, HashSet<String>>,
    module_map: HashMap<String, (PathBuf, Span)>,
    extra_registry_builders: Vec<&'a Registry>,
    deferred_instantiated: Vec<DeferredInstantiatedImport>,
}

/// Compile a single file within a project, using pre-evaluated values from dependencies.
///
/// Builds import bindings, lowers to IR, applies overrides, and type-resolves to TIR.
/// Both [`evaluate_project_perfile`] and [`compile_to_tir_project_perfile`] call this
/// for each file in the project.
fn compile_single_file_in_project(
    project: &crate::loader::LoadedProject,
    file_path: &Path,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    override_targets: &HashMap<DeclName, (PathBuf, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    let loaded_file = &project.files[file_path];
    let file_src = &loaded_file.named_source;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_values: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        extra_registry_builders: Vec::new(),
        deferred_instantiated: Vec::new(),
    };

    // Process all import declarations using loader-resolved canonical paths.
    for (decl, import_decl, import_canonical) in loaded_file.imports_with_paths() {
        let import_canonical = import_canonical.to_path_buf();
        if import_decl.param_bindings.is_empty() {
            process_non_instantiated_import(
                project,
                &import_canonical,
                import_decl,
                file_src,
                evaluated_files,
                &mut ctx,
            )?;
        } else {
            process_instantiated_import(
                project,
                file_path,
                &import_canonical,
                import_decl,
                decl,
                file_src,
                evaluated_files,
                &mut ctx,
            )?;
        }
    }

    // For module imports, resolve qualified references in expressions.
    let file_ast = rewrite_qualified_refs_in_ast(&loaded_file.ast, &ctx.module_map);

    // Lower to IR and finalize compilation.
    lower_and_finalize(
        project,
        file_path,
        file_src,
        &file_ast,
        ctx,
        evaluated_files,
        overrides,
        override_targets,
    )
}

/// Process an instantiated import (one with param bindings), deferring it for
/// post-lowering IR merging.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and allow_defaults check form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to project, importer, dep, and context"
)]
fn process_instantiated_import<'a>(
    project: &'a crate::loader::LoadedProject,
    importer_path: &Path,
    import_canonical: &PathBuf,
    import_decl: &graphcal_compiler::syntax::ast::ImportDecl,
    decl: &graphcal_compiler::syntax::ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files[import_canonical];
    let importer_loaded = &project.files[importer_path];

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &import_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            if let Some(alias_ident) = alias {
                alias_ident.name.clone()
            } else {
                derive_module_name_from_import_path(&import_decl.path, file_src)?
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Selective(_) => {
            // For selective instantiated imports, we still need a prefix
            // for the merged declarations. Derive from filename.
            derive_module_name_from_import_path(&import_decl.path, file_src)?
        }
    };

    // Check for duplicate module names (instantiated imports occupy the same namespace).
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
            name: prefix,
            src: file_src.clone(),
            span: import_decl.path.span().into(),
            first: (*first_span).into(),
        }));
    }
    ctx.module_map.insert(
        prefix.clone(),
        (import_canonical.clone(), import_decl.path.span()),
    );

    // Classify and validate bindings against the dependency's AST.
    // Each binding is either a param binding (name targets a `param`) or an
    // index binding (name targets a `cat`/`range` index).
    let mut bindings = HashMap::new();
    let mut index_bindings = HashMap::new();
    for binding in &import_decl.param_bindings {
        let binding_name = &binding.name.name;

        // Check if the binding name is a param in the dependency.
        let is_param = dep_loaded.ast.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == binding_name),
        );
        if is_param {
            bindings.insert(binding_name.clone(), binding.value.clone());
            continue;
        }

        // Check if it's an index in the dependency.
        let dep_index = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Index(idx) if idx.name.value.as_str() == binding_name => Some(idx),
                _ => None,
            });
        if let Some(dep_idx) = dep_index {
            // Index binding: extract the RHS index name from the expression.
            let rhs_name =
                extract_index_name_from_binding_expr(&binding.value, binding_name, file_src)?;

            // Validate the RHS resolves to an index in the importer's scope.
            // Check 1: importer's own AST.
            let importer_idx_ast =
                importer_loaded
                    .ast
                    .declarations
                    .iter()
                    .find_map(|d| match &d.kind {
                        DeclKind::Index(idx) if idx.name.value.as_str() == rhs_name => Some(idx),
                        _ => None,
                    });
            // Check 2: already-evaluated dependency registries.
            let importer_idx_from_registry = if importer_idx_ast.is_none() {
                ctx.extra_registry_builders
                    .iter()
                    .find_map(|reg| reg.indexes.get_index(&rhs_name))
                    .or_else(|| {
                        evaluated_files
                            .values()
                            .find_map(|ef| ef.registry.indexes.get_index(&rhs_name))
                    })
            } else {
                None
            };

            if importer_idx_ast.is_none() && importer_idx_from_registry.is_none() {
                return Err(CompileError::Eval(GraphcalError::IndexBindingNotAnIndex {
                    dep_index: binding_name.clone(),
                    value: rhs_name,
                    src: file_src.clone(),
                    span: binding.value.span.into(),
                }));
            }

            // Validate kind matching (named-to-named, range-to-range).
            let dep_is_named = matches!(
                dep_idx.kind,
                graphcal_compiler::syntax::ast::IndexDeclKind::Named { .. }
                    | graphcal_compiler::syntax::ast::IndexDeclKind::RequiredNamed
            );
            let imp_is_named = importer_idx_ast.map_or_else(
                || {
                    importer_idx_from_registry
                        .map(graphcal_compiler::registry::registry::IndexDef::is_named)
                },
                |imp_idx| {
                    Some(matches!(
                        imp_idx.kind,
                        graphcal_compiler::syntax::ast::IndexDeclKind::Named { .. }
                            | graphcal_compiler::syntax::ast::IndexDeclKind::RequiredNamed
                    ))
                },
            );
            if let Some(imp_named) = imp_is_named
                && dep_is_named != imp_named
            {
                return Err(CompileError::Eval(GraphcalError::IndexKindMismatch {
                    dep_index: binding_name.clone(),
                    dep_kind: if dep_is_named { "named" } else { "range" }.to_string(),
                    bound_index: rhs_name,
                    bound_kind: if imp_named { "named" } else { "range" }.to_string(),
                    src: file_src.clone(),
                    span: binding.name.span.into(),
                }));
            }
            // Dimension matching for range indexes is deferred to
            // process_deferred_instantiated_imports() where registries are available.

            index_bindings.insert(binding_name.clone(), rhs_name);
            continue;
        }

        // Check if it's some other kind of declaration.
        let actual_kind = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Node(n) if n.name.value.as_str() == binding_name => Some("node"),
                DeclKind::Const(c) if c.name.value.as_str() == binding_name => Some("const"),
                DeclKind::Assert(a) if a.name.value.as_str() == binding_name => Some("assert"),
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

    // Register the dependency's declaration names in the importer's scope
    // so that the resolver recognizes references to them.
    let mut import_item_attributes: HashMap<
        String,
        Vec<graphcal_compiler::syntax::ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &import_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the dependency.
                if !file_has_declaration(&dep_loaded.ast, orig_name) {
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: import_decl.path.display_path(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                // Collect import-item attributes for deferred processing.
                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(orig_name.clone(), import_item.attributes.clone());
                }

                // Register the local name in scope for the resolver.
                // Determine the category from the dep's AST.
                let is_const = dep_loaded.ast.declarations.iter().any(
                    |d| matches!(&d.kind, DeclKind::Const(c) if c.name.value.as_str() == orig_name),
                );
                let is_runtime = dep_loaded.ast.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        || matches!(&d.kind, DeclKind::Node(n) if n.name.value.as_str() == orig_name)
                });
                let scoped = ScopedName::Local(local_name.clone());
                let span = import_item.name.span;
                if is_const {
                    ctx.imported_names.const_names.push((scoped, span));
                } else if is_runtime {
                    ctx.imported_names.param_names.push((scoped, span));
                } else {
                    // Type-system declarations (dim/unit/index/type) are not
                    // registered in imported_names; handled via registry merge.
                }
                // Type-system declarations from instantiated imports also need registration.
                let is_type_system = dep_loaded.ast.declarations.iter().any(|d| match &d.kind {
                    DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
                    DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
                    DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
                    DeclKind::Type(t) => t.name.value.as_str() == orig_name,
                    _ => false,
                });
                if is_type_system {
                    ctx.imported_type_system_names
                        .entry(import_canonical.clone())
                        .or_default()
                        .insert(orig_name.clone());
                }

                selective.push((orig_name.clone(), local_name));
            }
            Some(selective)
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
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
                        ctx.imported_names.const_names.push((scoped, import_span));
                    } else {
                        ctx.imported_names.param_names.push((scoped, import_span));
                    }
                }
            }
            // Import type-system declarations.
            if let Some(dep_eval) = evaluated_files.get(import_canonical) {
                ctx.extra_registry_builders.push(&dep_eval.registry);
            }
            None
        }
    };

    // Required indexes must always be bound (regardless of allow_defaults).
    for dep_decl in &dep_loaded.ast.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(
                GraphcalError::RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: import_decl.path.span().into(),
                },
            ));
        }
    }

    // Strict check: when any binding is provided, ALL params and indexes
    // with defaults must be explicitly bound unless #[allow_defaults].
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
                return Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided {
                    name: p.name.value.to_string(),
                    src: file_src.clone(),
                    span: import_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <value>` in the import binding or add `#[allow_defaults]` to the import",
                        name = p.name.value,
                    ),
                }));
            }
            // Indexes with defaults (Named/Range, not Required*) must also be bound.
            if let DeclKind::Index(idx) = &dep_decl.kind
                && !idx.kind.is_required()
                && !index_bindings.contains_key(idx.name.value.as_str())
            {
                return Err(CompileError::Eval(GraphcalError::DefaultIndexNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: import_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <IndexName>` in the import binding or add `#[allow_defaults]` to the import",
                        name = idx.name.value,
                    ),
                }));
            }
        }
    }

    ctx.deferred_instantiated.push(DeferredInstantiatedImport {
        dep_path: import_canonical.clone(),
        prefix,
        bindings,
        index_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
    });
    Ok(())
}

/// Process a non-instantiated import (no param bindings), importing values and
/// type-system declarations from the already-evaluated dependency.
fn process_non_instantiated_import<'a>(
    project: &crate::loader::LoadedProject,
    import_canonical: &PathBuf,
    import_decl: &graphcal_compiler::syntax::ast::ImportDecl,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep = evaluated_files.get(import_canonical).ok_or_else(|| {
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
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                match import_selective_item(
                    dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    &mut ctx.imported_names,
                    &mut ctx.imported_values,
                    Some(&mut ctx.imported_source_order),
                ) {
                    SelectiveImportResult::Const
                    | SelectiveImportResult::Runtime
                    | SelectiveImportResult::Function => {}
                    SelectiveImportResult::Assert => {
                        // Assert is already evaluated in the dep file.
                        // We just need to make the name visible for #[assumes].
                        ctx.imported_names
                            .assert_names
                            .push((local_name, import_item.name.span));
                    }
                    SelectiveImportResult::NotFound => {
                        // Check if it's a type-system declaration in the dep's file.
                        let dep_loaded = &project.files[import_canonical];
                        if file_has_declaration(&dep_loaded.ast, orig_name) {
                            // Type-system declaration (dim/unit/index/type).
                            ctx.imported_type_system_names
                                .entry(import_canonical.clone())
                                .or_default()
                                .insert(orig_name.clone());
                        } else {
                            return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                                name: orig_name.clone(),
                                file_path: import_decl.path.display_path(),
                                src: file_src.clone(),
                                span: import_item.name.span.into(),
                            }));
                        }
                    }
                }
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            let module_name = if let Some(alias_ident) = alias {
                alias_ident.name.clone()
            } else {
                derive_module_name_from_import_path(&import_decl.path, file_src)?
            };
            if let Some((_, first_span)) = ctx.module_map.get(&module_name) {
                return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                    name: module_name,
                    src: file_src.clone(),
                    span: import_decl.path.span().into(),
                    first: (*first_span).into(),
                }));
            }
            ctx.module_map.insert(
                module_name.clone(),
                (import_canonical.clone(), import_decl.path.span()),
            );

            // Import all values under module::name prefix.
            let import_span = import_decl.path.span();
            import_module_values(
                dep,
                &module_name,
                import_span,
                &mut ctx.imported_names,
                &mut ctx.imported_values,
                Some(&mut ctx.imported_source_order),
            );
            // Import all type-system declarations from dep's registry.
            ctx.extra_registry_builders.push(&dep.registry);
        }
    }
    Ok(())
}

/// Rewrite qualified references in the AST when module imports are present.
///
/// If there are no module imports, returns a borrowed reference to the original AST.
/// Otherwise, clones the AST and rewrites `QualifiedGraphRef`, `QualifiedConstRef`,
/// and `QualifiedFnCall` to their flat counterparts.
fn rewrite_qualified_refs_in_ast<'a>(
    ast: &'a graphcal_compiler::syntax::ast::File,
    module_map: &HashMap<String, (PathBuf, Span)>,
) -> std::borrow::Cow<'a, graphcal_compiler::syntax::ast::File> {
    if module_map.is_empty() {
        return std::borrow::Cow::Borrowed(ast);
    }

    let mut ast = ast.clone();
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
                graphcal_compiler::syntax::ast::AssertBody::Expr(e) => rewrite_qualified_refs(e),
                graphcal_compiler::syntax::ast::AssertBody::Tolerance {
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
                graphcal_compiler::syntax::ast::FnBody::Short(e) => rewrite_qualified_refs(e),
                graphcal_compiler::syntax::ast::FnBody::Block { stmts, expr } => {
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
}

/// Lower the AST to IR, process deferred instantiated imports, apply overrides,
/// and type-resolve to produce the final `CompiledFile`.
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline function threading project context through IR lowering stages"
)]
fn lower_and_finalize(
    project: &crate::loader::LoadedProject,
    file_path: &Path,
    file_src: &NamedSource<Arc<String>>,
    file_ast: &graphcal_compiler::syntax::ast::File,
    ctx: ImportContext<'_>,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    override_targets: &HashMap<DeclName, (PathBuf, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    let saved_imported_values = ctx.imported_values.clone();

    let (mut builder, mut unfrozen) = crate::ir::lower_to_builder_with_imported_values(
        file_ast,
        file_src,
        &ctx.imported_names,
        ctx.imported_values,
    )?;

    // Register type-system declarations from selectively imported files.
    for (dep_path, names) in &ctx.imported_type_system_names {
        let dep_loaded = &project.files[dep_path];
        crate::ir::register_selected_declarations(
            &dep_loaded.ast,
            &mut builder,
            &dep_loaded.named_source,
            names,
        )?;
    }

    // Merge type-system declarations from module-imported registries.
    for dep_registry in &ctx.extra_registry_builders {
        merge_registry_into_builder(&mut builder, dep_registry, &HashMap::new());
    }

    // Process deferred instantiated imports: compile dep to IR and merge.
    process_deferred_instantiated_imports(
        project,
        &ctx.deferred_instantiated,
        evaluated_files,
        &mut builder,
        &mut unfrozen,
    )?;

    let ir = unfrozen.freeze(builder.build());

    // Apply overrides routed to this file (using original param names).
    let mut ir = ir;
    let file_overrides: HashMap<DeclName, graphcal_compiler::syntax::ast::Expr> = override_targets
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
        imported_source_order: ctx.imported_source_order,
    })
}

/// Process deferred instantiated imports by compiling each dependency to IR
/// and merging it into the importer's IR.
fn process_deferred_instantiated_imports(
    project: &crate::loader::LoadedProject,
    deferred_imports: &[DeferredInstantiatedImport],
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::ir::UnfrozenIR,
) -> Result<(), CompileError> {
    for deferred in deferred_imports {
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
        merge_registry_into_builder(builder, &dep_registry, &deferred.index_bindings);

        // Validate range index dimension matching (Phase B — requires compiled registries).
        for (dep_idx_name, importer_idx_name) in &deferred.index_bindings {
            if let Some(dep_idx_def) = dep_registry.indexes.get_index(dep_idx_name)
                && let crate::registry::IndexKind::RequiredRange { dimension: dep_dim } =
                    &dep_idx_def.kind
                && let Some(imp_idx_def) = builder.get_index(importer_idx_name)
                && let crate::registry::IndexKind::Range {
                    dimension: imp_dim, ..
                }
                | crate::registry::IndexKind::RequiredRange { dimension: imp_dim } =
                    &imp_idx_def.kind
                && dep_dim != imp_dim
            {
                return Err(CompileError::Eval(
                    GraphcalError::IndexBindingDimensionMismatch {
                        dep_index: dep_idx_name.clone(),
                        expected_dim: dep_registry.dimensions.format_dimension(dep_dim),
                        bound_index: importer_idx_name.clone(),
                        found_dim: builder.format_dimension(imp_dim),
                        src: dep_src.clone(),
                        span: deferred.import_span.into(),
                    },
                ));
            }
        }

        // Collect all declaration names in the dependency (for prefix_expr_refs).
        // These are un-prefixed member names used for containment checks.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dep_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }
        // Also include function names.
        for fn_entry in &dep_unfrozen.functions {
            dep_names.insert(fn_entry.name.member().to_string());
        }

        // Merge the dependency's IR into the importer's IR.
        unfrozen.merge_dependency(
            dep_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.import_item_attributes,
        );

        // For selective instantiated imports, add alias nodes that reference
        // the prefixed declarations. E.g., `delta_v` → `@prefix::delta_v`.
        if let Some(selective) = &deferred.selective_names {
            add_selective_aliases(dep_loaded, selective, deferred, unfrozen);
        }
    }
    Ok(())
}

/// Add alias declarations for selective instantiated imports.
///
/// For each selected name, creates either a const or node alias in the importer's IR
/// that references the prefixed declaration from the merged dependency.
fn add_selective_aliases(
    dep_loaded: &crate::loader::LoadedFile,
    selective: &[(String, String)],
    deferred: &DeferredInstantiatedImport,
    unfrozen: &mut graphcal_compiler::ir::ir::UnfrozenIR,
) {
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
                DeclKind::Node(n) if n.name.value.as_str() == orig_name => Some(n.type_ann.clone()),
                _ => None,
            });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        // Substitute index names in the type annotation.
        graphcal_compiler::ir::ir::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );

        // Determine if this is a const or runtime declaration.
        let is_const =
            dep_loaded.ast.declarations.iter().any(
                |d| matches!(&d.kind, DeclKind::Const(c) if c.name.value.as_str() == orig_name),
            );

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
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        } else {
            unfrozen.add_node_alias(
                ScopedName::local(local_name.clone()),
                type_ann,
                alias_expr,
                deferred.import_span,
                ScopedName::local(prefixed_name),
            );
        }
    }
}

/// Result of looking up a single selective import item in an `EvaluatedFile`.
enum SelectiveImportResult {
    /// A const value was found and registered.
    Const,
    /// A runtime value (param/node) was found and registered.
    Runtime,
    /// A function was found and registered.
    Function,
    /// An assert was found (caller must handle assert-specific registration).
    Assert,
    /// The name was not found in the evaluated file's values or functions.
    NotFound,
}

/// Look up a single selective import item in an `EvaluatedFile` and register it.
///
/// Handles `const_values`, values (params/nodes), and functions.
/// Returns what was found so the caller can handle assert and type-system fallbacks.
fn import_selective_item(
    dep: &EvaluatedFile,
    orig_name: &str,
    local_name: &str,
    span: Span,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) -> SelectiveImportResult {
    if let Some(rv) = dep.const_values.get(orig_name) {
        let scoped = ScopedName::Local(local_name.to_string());
        imported_names.const_names.push((scoped.clone(), span));
        let dt = dep
            .declared_types
            .get(orig_name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        SelectiveImportResult::Const
    } else if let Some(rv) = dep.values.get(orig_name) {
        let scoped = ScopedName::Local(local_name.to_string());
        imported_names.param_names.push((scoped.clone(), span));
        let dt = dep
            .declared_types
            .get(orig_name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
        SelectiveImportResult::Runtime
    } else if let Some(fn_entry) = dep
        .functions
        .iter()
        .find(|entry| entry.name.member() == orig_name)
    {
        imported_names.functions.push(
            graphcal_compiler::registry::resolve_types::ResolvedFunctionEntry {
                name: local_name.to_string(),
                decl: fn_entry.decl.clone(),
                span: fn_entry.span,
            },
        );
        SelectiveImportResult::Function
    } else if dep.has_assert(orig_name) {
        SelectiveImportResult::Assert
    } else {
        SelectiveImportResult::NotFound
    }
}

/// Import all values from an `EvaluatedFile` under a module prefix.
///
/// Registers `const_values`, values (params/nodes), and functions with qualified
/// `ScopedName::Qualified` names.
fn import_module_values(
    dep: &EvaluatedFile,
    module_name: &str,
    import_span: Span,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
) {
    // Sort keys for deterministic ordering — HashMap iteration is arbitrary.
    let mut const_keys: Vec<&String> = dep.const_values.keys().collect();
    const_keys.sort();
    for name in const_keys {
        let rv = &dep.const_values[name];
        let scoped = ScopedName::Qualified {
            module: module_name.to_string(),
            member: name.clone(),
        };
        imported_names
            .const_names
            .push((scoped.clone(), import_span));
        let dt = dep
            .declared_types
            .get(name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Const));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }
    let mut value_keys: Vec<&String> = dep.values.keys().collect();
    value_keys.sort();
    for name in value_keys {
        let rv = &dep.values[name];
        let scoped = ScopedName::Qualified {
            module: module_name.to_string(),
            member: name.clone(),
        };
        imported_names
            .param_names
            .push((scoped.clone(), import_span));
        let dt = dep
            .declared_types
            .get(name)
            .cloned()
            .unwrap_or(DeclaredType::Scalar(
                graphcal_compiler::syntax::dimension::Dimension::dimensionless(),
            ));
        if let Some(ref mut source_order) = imported_source_order {
            source_order.push((scoped.clone(), DeclCategory::Param));
        }
        imported_values.insert(scoped, (rv.clone(), dt));
    }
    for fn_entry in &dep.functions {
        let flat = format!("{module_name}::{}", fn_entry.name);
        imported_names.functions.push(
            graphcal_compiler::registry::resolve_types::ResolvedFunctionEntry {
                name: flat,
                decl: fn_entry.decl.clone(),
                span: fn_entry.span,
            },
        );
    }
}

/// Build imported value names and values for a dependency file from its own transitive imports.
///
/// This mirrors the import-processing logic in `compile_single_file_in_project` but
/// only for non-instantiated imports (the dependency's own transitive deps are already
/// evaluated and stored in `evaluated_files`).
fn build_dep_imported_values(
    project: &crate::loader::LoadedProject,
    dep_path: &Path,
    evaluated_files: &HashMap<PathBuf, EvaluatedFile>,
) -> Result<DepImportedValues, CompileError> {
    let dep_loaded = &project.files[dep_path];
    let dep_src = &dep_loaded.named_source;

    let mut imported_names = ImportedValueNames::default();
    let mut imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)> = HashMap::new();

    for (_decl, import_decl, trans_canonical) in dep_loaded.imports_with_paths() {
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

        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
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
            graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
                for import_item in names {
                    let orig_name = &import_item.name.name;
                    let local_name = import_item.local_name().to_string();
                    // Type-system declarations are handled by the registry, not imported_values.
                    let _ = import_selective_item(
                        trans_dep,
                        orig_name,
                        &local_name,
                        import_item.name.span,
                        &mut imported_names,
                        &mut imported_values,
                        None,
                    );
                }
            }
            graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
                let module_name = alias.as_ref().map_or_else(
                    || {
                        derive_module_name_from_import_path(&import_decl.path, dep_src)
                            .unwrap_or_else(|_| "dep".to_string())
                    },
                    |alias_ident| alias_ident.name.clone(),
                );
                let import_span = import_decl.path.span();
                import_module_values(
                    trans_dep,
                    &module_name,
                    import_span,
                    &mut imported_names,
                    &mut imported_values,
                    None,
                );
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
            const_values: plan
                .const_values
                .into_iter()
                .map(|(k, v)| (k.into_inner(), v))
                .collect(),
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
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    allow_defaults: bool,
) -> Result<EvalResult, CompileError> {
    // Pre-compute override routing: map each override name to the file that owns
    // the param. Walk root file's imports to find the owning file for each override.
    let override_targets = route_overrides_to_files(project, overrides)?;

    // Strict param check: when overrides are provided and --allow-defaults is not set,
    // all overridable params (root file + selectively imported) must be explicitly provided.
    if !overrides.is_empty() && !allow_defaults {
        let root_file = &project.files[&project.root];
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
        for (_decl, import_decl, import_canonical) in root_file.imports_with_paths() {
            // Only check non-parameterized imports (parameterized have their own check)
            if !import_decl.param_bindings.is_empty() {
                continue;
            }
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = &import_decl.kind
            {
                let dep_file = &project.files[import_canonical];
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
                            let is_overridden = overrides.keys().any(|k| k.as_str() == local_name);
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

        // Files with required params (no default) or required indexes cannot be
        // evaluated standalone. They are only consumed via instantiated imports
        // where `merge_dependency` provides the bindings.
        let has_required_params = compiled
            .tir
            .params
            .iter()
            .any(|entry| entry.default_expr.is_none());
        let has_required_indexes = compiled
            .tir
            .registry
            .indexes
            .all_indexes()
            .any(graphcal_compiler::registry::registry::IndexDef::is_required);

        if !is_root && (has_required_params || has_required_indexes) {
            continue;
        }

        if is_root {
            // Reject standalone evaluation of files with required indexes.
            if has_required_indexes {
                let file_src = &project.files[file_path].named_source;
                for idx_def in compiled.tir.registry.indexes.all_indexes() {
                    if idx_def.is_required() {
                        let span = project.files[file_path]
                            .ast
                            .declarations
                            .iter()
                            .find_map(|d| {
                                if let DeclKind::Index(idx) = &d.kind
                                    && idx.name.value.as_str() == idx_def.name.as_str()
                                {
                                    Some(d.span.into())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| miette::SourceSpan::from((0, 0)));
                        return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
                            name: idx_def.name.to_string(),
                            src: file_src.clone(),
                            span,
                        }));
                    }
                }
            }
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
                        DeclCategory::Assert
                        | DeclCategory::Plot
                        | DeclCategory::Figure
                        | DeclCategory::Layer => {}
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
                layers: eval_result.layers,
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
/// that started the chain. When a transitive dependency is reachable from multiple
/// root imports, the first root import in source order wins.
fn build_dep_import_spans(project: &crate::loader::LoadedProject) -> HashMap<PathBuf, Span> {
    let root_file = &project.files[&project.root];
    let mut spans: HashMap<PathBuf, Span> = HashMap::new();

    // Process root's direct imports in source order (as returned by imports_with_paths).
    // For each, DFS into its transitive dependencies, propagating the root import span.
    // `entry().or_insert()` ensures the first root import (in source order) to reach
    // a transitive dep determines its attribution.
    for (decl, _import_decl, canonical) in root_file.imports_with_paths() {
        let root_span = decl.span;
        let mut stack = vec![canonical.to_path_buf()];
        while let Some(path) = stack.pop() {
            if path == project.root {
                continue;
            }
            // Only process if not already attributed.
            if let std::collections::hash_map::Entry::Vacant(entry) = spans.entry(path.clone()) {
                entry.insert(root_span);
                // Push this file's own imports for transitive propagation.
                if let Some(file) = project.files.get(&path) {
                    for (_decl, _imp, c) in file.imports_with_paths() {
                        if !spans.contains_key(c) {
                            stack.push(c.to_path_buf());
                        }
                    }
                }
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
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
) -> Result<HashMap<DeclName, (PathBuf, DeclName)>, CompileError> {
    if overrides.is_empty() {
        return Ok(HashMap::new());
    }

    let root_file = &project.files[&project.root];

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
        for (_decl, import_decl, import_canonical) in root_file.imports_with_paths() {
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = &import_decl.kind
            {
                for item in names {
                    let local_name = item.local_name().to_string();
                    if local_name == name_str {
                        let orig_name = &item.name.name;

                        // Verify it's actually a param in the source file.
                        let dep_file = &project.files[import_canonical];
                        let is_param = dep_file.ast.declarations.iter().any(|d| {
                            matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        });
                        if is_param {
                            result.insert(
                                override_name.clone(),
                                (
                                    import_canonical.to_path_buf(),
                                    DeclName::new(orig_name.clone()),
                                ),
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
    let local_runtime_names: HashSet<String> = tir
        .params
        .iter()
        .map(|e| e.name.to_string())
        .chain(tir.nodes.iter().map(|e| e.name.to_string()))
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
fn merge_registry_into_builder(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<String, String>,
) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        builder.register_base_dimension(
            graphcal_compiler::syntax::names::DimName::new(name),
            id.clone(),
        );
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
        builder.register_unit_dynamic((*name).clone(), dim.clone(), scale.clone());
    }

    // Import indexes — skip bound indexes (they are replaced by the importer's index).
    for idx_def in dep_registry.indexes.all_indexes() {
        if !index_bindings.contains_key(idx_def.name.as_str()) {
            builder.register_index(idx_def.clone());
        }
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
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
) -> Result<(), CompileError> {
    for (override_name, override_expr) in overrides {
        let name_str = override_name.as_str();
        if let Some((_, cat)) = ir.source_order.iter().find(|(n, _)| n.member() == name_str) {
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

        if let Some(entry) = ir.params.iter_mut().find(|e| e.name.member() == name_str) {
            entry.default_expr = Some(override_expr.clone());
        }

        let all_runtime: std::collections::HashSet<&str> = ir
            .params
            .iter()
            .map(|e| e.name.member())
            .chain(ir.nodes.iter().map(|e| e.name.member()))
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        ir.runtime_deps.insert(
            ScopedName::local(name_str),
            graph_refs.into_iter().map(ScopedName::local).collect(),
        );
    }
    Ok(())
}

/// Compile a [`LoadedProject`](crate::loader::LoadedProject) to TIR without evaluating.
///
/// Resolves imports from `use` declarations in the root file, lowers to IR,
/// type-resolves, and runs all checks (recursion, dimensions). The project may
/// have been loaded from disk, constructed from in-memory source, or a mix of
/// both (via [`graphcal_io::OverlayFileSystem`] + [`crate::loader::load_project`]).
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
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
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
/// All filesystem access goes through the provided [`graphcal_io::FileSystemReader`].
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or evaluation fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_project<F: graphcal_io::FileSystemReader>(
    root_path: &Path,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
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
/// All filesystem access goes through the provided [`graphcal_io::FileSystemReader`].
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or checking fails.
pub fn compile_to_tir_project<F: graphcal_io::FileSystemReader>(
    root_path: &Path,
    project_root: Option<&Path>,
    fs: &F,
) -> Result<(crate::tir::TIR, crate::loader::LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

/// Extract a `PascalCase` index name from a binding expression.
///
/// Index bindings use the form `DepIndex = ImporterIndex`, where both sides are
/// `PascalCase` identifiers. The parser produces `ExprKind::StructConstruction`
/// (with empty `fields`/`type_args`) for bare `PascalCase` identifiers in
/// expression position, because it cannot distinguish a bare type name from an
/// index name at parse time.
fn extract_index_name_from_binding_expr(
    expr: &Expr,
    dep_index_name: &str,
    file_src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match &expr.kind {
        ExprKind::ConstRef(name) => Ok(name.value.to_string()),
        ExprKind::StructConstruction {
            type_name,
            type_args,
            fields,
        } if type_args.is_empty() && fields.is_empty() => Ok(type_name.value.as_str().to_string()),
        _ => Err(CompileError::Eval(GraphcalError::BindingTargetsIndex {
            name: dep_index_name.to_string(),
            src: file_src.clone(),
            span: expr.span.into(),
        })),
    }
}

/// Check whether a file contains a declaration with the given name.
///
/// Returns `true` if the file has a type-system declaration (dimension, unit,
/// index, or struct type) with that name. This is used as a fallback when a
/// selective import name is not found among the dependency's evaluated values
/// or functions.
pub(super) fn file_has_declaration(
    file: &graphcal_compiler::syntax::ast::File,
    name: &str,
) -> bool {
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
        DeclKind::UnionType(u) => u.name.value.as_str() == name,
        DeclKind::Plot(p) => p.name.value.as_str() == name,
        DeclKind::Figure(f) => f.name.value.as_str() == name,
        DeclKind::Layer(l) => l.name.value.as_str() == name,
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
    if let graphcal_compiler::syntax::ast::TypeExprKind::DimExpr(dim_expr) = &field.type_ann.kind
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

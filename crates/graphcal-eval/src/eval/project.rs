//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::syntax::ast::{DeclKind, Expr, ExprKind, ImportPath};
use graphcal_compiler::syntax::names::{DeclName, Spanned};
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
        ImportPath::ParentScope { .. } => {
            // Parent scope imports don't derive a module name;
            // they bring items directly into scope.
            Ok("parent".to_string())
        }
        ImportPath::CrossFileDag { dag_name, .. } => {
            // Cross-file DAG paths use the DAG name as the module name.
            Ok(dag_name.name.clone())
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
}

/// Rewrite qualified references to flat names in-place.
///
/// Replaces `QualifiedGraphRef { module: "m", name: "x" }` with `GraphRef("m::x")`,
/// and `QualifiedConstRef` with `ConstRef`.
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
    /// Names of declarations marked `pub` in the source file.
    /// Used to enforce private-by-default visibility during imports.
    pub_names: HashSet<String>,
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
struct DepImportedValues {
    names: ImportedValueNames,
    values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
}

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

/// A deferred inline DAG include that needs IR merging.
struct DeferredInlineDagInclude {
    /// Virtual File AST constructed from the DAG body declarations.
    dag_body: graphcal_compiler::syntax::ast::File,
    /// Imported names collected from `import ..` inside the DAG body.
    dag_imported_names: ImportedValueNames,
    /// Type-system declarations imported from parent scope via `import ..`.
    dag_parent_type_decls: Vec<graphcal_compiler::syntax::ast::Declaration>,
    /// The prefix for all merged declarations (from alias or dag name).
    prefix: String,
    /// Param bindings: `param_name` → binding expression.
    bindings: HashMap<String, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    index_bindings: HashMap<String, String>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports.
    selective_names: Option<Vec<(String, String)>>,
    /// Span of the include declaration (for diagnostics).
    import_span: Span,
    /// Per-import-item attributes.
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
    /// Registry + `pub_names` for module-imported dependencies.
    extra_registry_builders: Vec<(&'a Registry, &'a HashSet<String>)>,
    deferred_instantiated: Vec<DeferredInstantiatedImport>,
    deferred_inline_dags: Vec<DeferredInlineDagInclude>,
}

/// Compile a single file within a project, using pre-evaluated values from dependencies.
///
/// Builds import bindings, lowers to IR, applies overrides, and type-resolves to TIR.
/// Both [`evaluate_project_perfile`] and [`compile_to_tir_project_perfile`] call this
/// for each file in the project.
#[expect(
    clippy::too_many_lines,
    reason = "import processing, inline DAG handling, and cross-file DAG handling form a cohesive pipeline"
)]
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
        deferred_inline_dags: Vec::new(),
    };

    // Collect inline DAG definitions from the file's AST.
    let dag_definitions: HashMap<String, &graphcal_compiler::syntax::ast::DagDecl> = loaded_file
        .ast
        .declarations
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Dag(dag) => Some((dag.name.value.to_string(), dag)),
            _ => None,
        })
        .collect();

    // Check for recursive DAG instantiation.
    check_dag_recursion(&dag_definitions, file_src)?;

    // Process all import declarations (non-instantiated, compile-time items only).
    for (_decl, import_decl, import_canonical) in loaded_file.imports_with_paths() {
        let import_canonical = import_canonical.to_path_buf();
        process_non_instantiated_import(
            project,
            &import_canonical,
            &import_decl.path,
            &import_decl.kind,
            file_src,
            evaluated_files,
            &mut ctx,
            true, // is_import: enforce const-only
        )?;
    }

    // Process all include declarations (file-based DAG instantiation).
    // Inline DAG includes (single-segment module paths matching a dag name),
    // cross-file DAG includes, and bare module path DAG includes are handled below.
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_paths() {
        // Skip cross-file DAG includes — handled in the next section.
        if include_decl.path.is_cross_file_dag() {
            continue;
        }
        // Skip bare module path DAG references — handled after inline DAGs.
        // These are multi-segment ModulePath includes where the last segment
        // matches a DAG in the resolved target file.
        if is_bare_module_dag_ref(
            &include_decl.path,
            &include_canonical.to_path_buf(),
            project,
        ) {
            continue;
        }
        let include_canonical = include_canonical.to_path_buf();
        if include_decl.param_bindings.is_empty() {
            process_non_instantiated_import(
                project,
                &include_canonical,
                &include_decl.path,
                &include_decl.kind,
                file_src,
                evaluated_files,
                &mut ctx,
                false, // is_import: include allows runtime items
            )?;
        } else {
            process_instantiated_include(
                project,
                file_path,
                &include_canonical,
                include_decl,
                decl,
                file_src,
                evaluated_files,
                &mut ctx,
            )?;
        }
    }

    // Process inline DAG includes (include dag_name(...) { ... }).
    // These are includes with single-segment module paths that match inline DAG definitions.
    for decl in &loaded_file.ast.declarations {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        // Only handle single-segment module paths that match a DAG name.
        let dag_name = match &include_decl.path {
            graphcal_compiler::syntax::ast::ImportPath::ModulePath { segments, .. }
                if segments.len() == 1 =>
            {
                &segments[0].name
            }
            _ => continue,
        };
        let dag_def = match dag_definitions.get(dag_name.as_str()) {
            Some(dag) => *dag,
            None => continue, // Not an inline DAG — already handled by file-based includes
        };

        process_inline_dag_include(
            dag_def,
            dag_name,
            include_decl,
            decl,
            &loaded_file.ast,
            file_src,
            &mut ctx,
            false, // same-file DAG
        )?;
    }

    // Process cross-file DAG includes (include "./file.gcl"/dag_name(...) { ... }).
    // These reference inline DAG definitions in other files.
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_paths() {
        let ImportPath::CrossFileDag { dag_name, .. } = &include_decl.path else {
            continue;
        };

        let include_canonical = include_canonical.to_path_buf();

        // Find the target file's AST from the project.
        let target_loaded = project.files.get(&include_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "cross-file DAG target file not found in project: {}",
                    include_canonical.display()
                ),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        // Find the named DAG definition in the target file's AST.
        let target_dag_def = target_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Dag(dag) if dag.name.value.as_str() == dag_name.name => Some(dag),
                _ => None,
            })
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!(
                        "DAG `{}` not found in file `{}`",
                        dag_name.name,
                        include_canonical.display()
                    ),
                    src: file_src.clone(),
                    span: dag_name.span.into(),
                })
            })?;

        // Reuse the same inline DAG processing, but with the target file's AST
        // for parent scope resolution.
        process_inline_dag_include(
            target_dag_def,
            &dag_name.name,
            include_decl,
            decl,
            &target_loaded.ast,
            file_src,
            &mut ctx,
            true, // cross-file DAG
        )?;
    }

    // Process bare module path DAG includes (include pkg/mod/dag_name(...) { ... }).
    // These are multi-segment ModulePath includes where the last segment is a DAG
    // defined in the resolved parent file (e.g. `pkg/mod.gcl` contains `dag dag_name`).
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_paths() {
        if !is_bare_module_dag_ref(
            &include_decl.path,
            &include_canonical.to_path_buf(),
            project,
        ) {
            continue;
        }

        let include_canonical = include_canonical.to_path_buf();
        let ImportPath::ModulePath { segments, .. } = &include_decl.path else {
            // is_bare_module_dag_ref only returns true for ModulePath
            continue;
        };
        // Safety: is_bare_module_dag_ref ensures segments.len() >= 2
        let Some(last_seg) = segments.last() else {
            continue;
        };
        let dag_name = &last_seg.name;

        // Find the target file's AST from the project.
        let target_loaded = project.files.get(&include_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "bare module DAG target file not found in project: {}",
                    include_canonical.display()
                ),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        // Find the named DAG definition in the target file's AST.
        let target_dag_def = target_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Dag(dag) if dag.name.value.as_str() == dag_name => Some(dag),
                _ => None,
            })
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!(
                        "DAG `{}` not found in file `{}`",
                        dag_name,
                        include_canonical.display()
                    ),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                })
            })?;

        // Reuse the same inline DAG processing, with the target file's AST
        // for parent scope resolution.
        process_inline_dag_include(
            target_dag_def,
            dag_name,
            include_decl,
            decl,
            &target_loaded.ast,
            file_src,
            &mut ctx,
            true, // cross-file DAG
        )?;
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

/// Process an instantiated include (one with param bindings), deferring it for
/// post-lowering IR merging.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and allow_defaults check form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to project, importer, dep, and context"
)]
fn process_instantiated_include<'a>(
    project: &'a crate::loader::LoadedProject,
    importer_path: &Path,
    import_canonical: &PathBuf,
    include_decl: &graphcal_compiler::syntax::ast::IncludeDecl,
    decl: &graphcal_compiler::syntax::ast::Declaration,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
) -> Result<(), CompileError> {
    let dep_loaded = &project.files[import_canonical];
    let importer_loaded = &project.files[importer_path];

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            if let Some(alias_ident) = alias {
                alias_ident.name.clone()
            } else {
                derive_module_name_from_import_path(&include_decl.path, file_src)?
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Selective(_) => {
            // For selective instantiated includes, we still need a prefix
            // for the merged declarations. Derive from filename.
            derive_module_name_from_import_path(&include_decl.path, file_src)?
        }
    };

    // Check for duplicate module names (instantiated includes occupy the same namespace).
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
            name: prefix,
            src: file_src.clone(),
            span: include_decl.path.span().into(),
            first: (*first_span).into(),
        }));
    }
    ctx.module_map.insert(
        prefix.clone(),
        (import_canonical.clone(), include_decl.path.span()),
    );

    // Classify and validate bindings against the dependency's AST.
    // Each binding is either a param binding (name targets a `param`) or an
    // index binding (name targets a `cat`/`range` index).
    let mut bindings = HashMap::new();
    let mut index_bindings = HashMap::new();
    for binding in &include_decl.param_bindings {
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
                    .find_map(|(reg, _)| reg.indexes.get_index(&rhs_name))
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
                        .map(graphcal_compiler::registry::types::IndexDef::is_named)
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
                DeclKind::ConstNode(c) if c.name.value.as_str() == binding_name => {
                    Some("const node")
                }
                DeclKind::Node(n) if n.name.value.as_str() == binding_name => Some("node"),
                DeclKind::Assert(a) if a.name.value.as_str() == binding_name => Some("assert"),
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
            file_path: include_decl.path.display_path(),
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
    let selective_names = match &include_decl.kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the dependency.
                if !file_has_declaration(&dep_loaded.ast, orig_name) {
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: include_decl.path.display_path(),
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
                let is_const = dep_loaded.ast.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name)
                });
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
                    DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
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
            let import_span = include_decl.path.span();
            for dep_decl in &dep_loaded.ast.declarations {
                let (dep_name, is_const) = match &dep_decl.kind {
                    DeclKind::Param(p) => (Some(p.name.value.to_string()), false),
                    DeclKind::ConstNode(c) => (Some(c.name.value.to_string()), true),
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
            // Import type-system declarations (pub items only).
            if let Some(dep_eval) = evaluated_files.get(import_canonical) {
                ctx.extra_registry_builders
                    .push((&dep_eval.registry, &dep_eval.pub_names));
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
                    span: include_decl.path.span().into(),
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
                    span: include_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <value>` in the include binding or add `#[allow_defaults]` to the include",
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
                    span: include_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <IndexName>` in the include binding or add `#[allow_defaults]` to the include",
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

/// Process an inline DAG include (`include dag_name(...) { ... }`).
///
/// Creates a virtual File from the DAG body, validates bindings against it,
/// and defers for IR merging.
///
/// When `is_cross_file` is `true`, const node declarations from the parent scope
/// (`import ..`) are included directly in the DAG body rather than being registered
/// as external imported names. This is necessary because the parent file's const
/// values are not available in the importing file's IR.
#[expect(
    clippy::too_many_lines,
    reason = "binding validation, scope registration, and deferred include setup form a single cohesive pipeline"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "needs access to DAG def, include decl, parent AST, and cross-file flag"
)]
fn process_inline_dag_include(
    dag_def: &graphcal_compiler::syntax::ast::DagDecl,
    dag_name: &str,
    include_decl: &graphcal_compiler::syntax::ast::IncludeDecl,
    decl: &graphcal_compiler::syntax::ast::Declaration,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    ctx: &mut ImportContext<'_>,
    is_cross_file: bool,
) -> Result<(), CompileError> {
    use graphcal_compiler::syntax::ast::ImportKind;

    // Determine the prefix (namespace) for the merged declarations.
    let prefix = match &include_decl.kind {
        ImportKind::Module { alias } => alias.as_ref().map_or_else(
            || dag_name.to_string(),
            |alias_ident| alias_ident.name.clone(),
        ),
        ImportKind::Selective(_) => dag_name.to_string(),
    };

    // Check for duplicate module names.
    // We use a sentinel path for inline DAGs in the module_map.
    let sentinel_path = PathBuf::from(format!("<dag:{dag_name}>"));
    if let Some((_, first_span)) = ctx.module_map.get(&prefix) {
        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
            name: prefix,
            src: file_src.clone(),
            span: include_decl.path.span().into(),
            first: (*first_span).into(),
        }));
    }
    ctx.module_map
        .insert(prefix.clone(), (sentinel_path, include_decl.path.span()));

    // Create a virtual File from the DAG body, filtering out `import ..` declarations.
    // Import .. declarations are processed separately to populate the DAG's imported names.
    let mut dag_body_decls = Vec::new();
    let mut dag_imported_names = ImportedValueNames::default();
    let mut dag_parent_type_decls = Vec::new();

    for body_decl in &dag_def.body {
        match &body_decl.kind {
            DeclKind::Import(import_decl) if import_decl.path.is_parent_scope() => {
                if is_cross_file {
                    // For cross-file DAGs, resolve parent scope items and include
                    // const declarations directly in the DAG body (since the parent
                    // file's values are not in the importing file's IR).
                    process_cross_file_parent_scope_import(
                        &import_decl.kind,
                        parent_ast,
                        file_src,
                        &mut dag_body_decls,
                        &mut dag_parent_type_decls,
                    )?;
                } else {
                    // For same-file DAGs, register names for resolution; the actual
                    // values are available in the parent file's IR.
                    process_parent_scope_import(
                        &import_decl.kind,
                        parent_ast,
                        file_src,
                        &mut dag_imported_names,
                        &mut dag_parent_type_decls,
                    )?;
                }
            }
            _ => {
                dag_body_decls.push(body_decl.clone());
            }
        }
    }

    let dag_body = graphcal_compiler::syntax::ast::File {
        declarations: dag_body_decls,
    };

    // Classify and validate bindings against the DAG body's declarations.
    let mut bindings = HashMap::new();
    let mut index_bindings = HashMap::new();
    for binding in &include_decl.param_bindings {
        let binding_name = &binding.name.name;

        // Check if the binding name is a param in the DAG body.
        let is_param = dag_body.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == binding_name),
        );
        if is_param {
            bindings.insert(binding_name.clone(), binding.value.clone());
            continue;
        }

        // Check if it's an index in the DAG body.
        let dep_index = dag_body.declarations.iter().find_map(|d| match &d.kind {
            DeclKind::Index(idx) if idx.name.value.as_str() == binding_name => Some(idx),
            _ => None,
        });
        if let Some(_dep_idx) = dep_index {
            let rhs_name =
                extract_index_name_from_binding_expr(&binding.value, binding_name, file_src)?;
            index_bindings.insert(binding_name.clone(), rhs_name);
            continue;
        }

        // Unknown binding target.
        let actual_kind = dag_body.declarations.iter().find_map(|d| match &d.kind {
            DeclKind::ConstNode(c) if c.name.value.as_str() == binding_name => Some("const node"),
            DeclKind::Node(n) if n.name.value.as_str() == binding_name => Some("node"),
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
            file_path: dag_name.to_string(),
            src: file_src.clone(),
            span: binding.name.span.into(),
        }));
    }

    // Register imported names in the importer's scope.
    let mut import_item_attributes: HashMap<
        String,
        Vec<graphcal_compiler::syntax::ast::Attribute>,
    > = HashMap::new();
    let selective_names = match &include_decl.kind {
        ImportKind::Selective(names) => {
            let mut selective = Vec::new();
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Verify the name exists in the DAG body.
                if !file_has_declaration(&dag_body, orig_name) {
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: dag_name.to_string(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                if !import_item.attributes.is_empty() {
                    import_item_attributes
                        .insert(orig_name.clone(), import_item.attributes.clone());
                }

                // Register the local name in scope.
                let is_const = dag_body.declarations.iter().any(|d| {
                    matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name)
                });
                let is_runtime = dag_body.declarations.iter().any(|d| {
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
                    // Type-system declarations — handled via registry merge.
                }

                selective.push((orig_name.clone(), local_name));
            }
            Some(selective)
        }
        ImportKind::Module { .. } => {
            // Register all DAG body names under the prefix.
            let import_span = include_decl.path.span();
            for dep_decl in &dag_body.declarations {
                let (dep_name, is_const) = match &dep_decl.kind {
                    DeclKind::Param(p) => (Some(p.name.value.to_string()), false),
                    DeclKind::ConstNode(c) => (Some(c.name.value.to_string()), true),
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
            None
        }
    };

    // Strict binding check: all required params/indexes must be bound.
    for dep_decl in &dag_body.declarations {
        if let DeclKind::Index(idx) = &dep_decl.kind
            && idx.kind.is_required()
            && !index_bindings.contains_key(idx.name.value.as_str())
        {
            return Err(CompileError::Eval(
                GraphcalError::RequiredParamNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                },
            ));
        }
    }

    let allow_defaults = decl
        .attributes
        .iter()
        .any(|a| a.name.name == "allow_defaults");
    if !allow_defaults {
        for dep_decl in &dag_body.declarations {
            if let DeclKind::Param(p) = &dep_decl.kind
                && p.value.is_some()
                && !bindings.contains_key(p.name.value.as_str())
            {
                return Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided {
                    name: p.name.value.to_string(),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <value>` in the include binding or add `#[allow_defaults]` to the include",
                        name = p.name.value,
                    ),
                }));
            }
            if let DeclKind::Index(idx) = &dep_decl.kind
                && !idx.kind.is_required()
                && !index_bindings.contains_key(idx.name.value.as_str())
            {
                return Err(CompileError::Eval(GraphcalError::DefaultIndexNotProvided {
                    name: idx.name.value.to_string(),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                    help: format!(
                        "provide `{name} = <IndexName>` in the include binding or add `#[allow_defaults]` to the include",
                        name = idx.name.value,
                    ),
                }));
            }
        }
    }

    ctx.deferred_inline_dags.push(DeferredInlineDagInclude {
        dag_body,
        dag_imported_names,
        dag_parent_type_decls,
        prefix,
        bindings,
        index_bindings,
        selective_names,
        import_span: decl.span,
        import_item_attributes,
    });
    Ok(())
}

/// Process `import .. { ... }` declarations inside a DAG body.
///
/// Resolves the imported items to compile-time declarations in the parent scope
/// and populates the DAG's imported names.
fn process_parent_scope_import(
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    dag_imported_names: &mut ImportedValueNames,
    dag_parent_type_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
) -> Result<(), CompileError> {
    let names = match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => names,
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
            // `import .. as alias;` or `import ..;` — not supported (semantics unclear).
            // Only selective parent scope imports are supported.
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "module-style `import ..` is not supported; use `import .. { name1, name2 }` to import specific items from the parent scope".to_string(),
                src: file_src.clone(),
                span: (0..0).into(),
            }));
        }
    };

    for import_item in names {
        let orig_name = &import_item.name.name;
        let local_name = import_item.local_name().to_string();

        // Find the declaration in the parent scope.
        let parent_decl = parent_ast.declarations.iter().find(|d| match &d.kind {
            DeclKind::ConstNode(c) => c.name.value.as_str() == orig_name,
            DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
            DeclKind::Type(t) => t.name.value.as_str() == orig_name,
            DeclKind::UnionType(t) => t.name.value.as_str() == orig_name,
            DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
            DeclKind::Dag(dag) => dag.name.value.as_str() == orig_name,
            // Runtime items and other declarations are NOT importable via `import ..`.
            _ => false,
        });

        let parent_decl = parent_decl.ok_or_else(|| {
            CompileError::Eval(GraphcalError::ImportNameNotFound {
                name: orig_name.clone(),
                file_path: "..".to_string(),
                src: file_src.clone(),
                span: import_item.name.span.into(),
            })
        })?;

        // Classify the imported item.
        match &parent_decl.kind {
            DeclKind::ConstNode(_) => {
                let scoped = ScopedName::Local(local_name);
                dag_imported_names
                    .const_names
                    .push((scoped, import_item.name.span));
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_) => {
                // Type-system declarations — need to be registered in the DAG's registry.
                dag_parent_type_decls.push(parent_decl.clone());
            }
            // DAG definitions and other items don't need registration in imported_names.
            _ => {}
        }
    }

    Ok(())
}

/// Process `import .. { ... }` declarations inside a cross-file DAG body.
///
/// Unlike same-file DAGs where parent scope const values are available in the
/// importing file's IR, cross-file DAGs must include the parent const declarations
/// directly in the DAG body. Type-system declarations are handled via the
/// `dag_parent_type_decls` mechanism as usual.
fn process_cross_file_parent_scope_import(
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    parent_ast: &graphcal_compiler::syntax::ast::File,
    file_src: &NamedSource<Arc<String>>,
    dag_body_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
    dag_parent_type_decls: &mut Vec<graphcal_compiler::syntax::ast::Declaration>,
) -> Result<(), CompileError> {
    let names = match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => names,
        graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "module-style `import ..` is not supported; use `import .. { name1, name2 }` to import specific items from the parent scope".to_string(),
                src: file_src.clone(),
                span: (0..0).into(),
            }));
        }
    };

    for import_item in names {
        let orig_name = &import_item.name.name;

        // Find the declaration in the parent scope.
        let parent_decl = parent_ast.declarations.iter().find(|d| match &d.kind {
            DeclKind::ConstNode(c) => c.name.value.as_str() == orig_name,
            DeclKind::BaseDimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Dimension(dim) => dim.name.value.as_str() == orig_name,
            DeclKind::Unit(u) => u.name.value.as_str() == orig_name,
            DeclKind::Type(t) => t.name.value.as_str() == orig_name,
            DeclKind::UnionType(t) => t.name.value.as_str() == orig_name,
            DeclKind::Index(idx) => idx.name.value.as_str() == orig_name,
            DeclKind::Dag(dag) => dag.name.value.as_str() == orig_name,
            _ => false,
        });

        let parent_decl = parent_decl.ok_or_else(|| {
            CompileError::Eval(GraphcalError::ImportNameNotFound {
                name: orig_name.clone(),
                file_path: "..".to_string(),
                src: file_src.clone(),
                span: import_item.name.span.into(),
            })
        })?;

        match &parent_decl.kind {
            DeclKind::ConstNode(_) => {
                // Include const declarations directly in the DAG body so they
                // become part of the DAG's IR and get merged with prefixing.
                dag_body_decls.push(parent_decl.clone());
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_) => {
                dag_parent_type_decls.push(parent_decl.clone());
            }
            _ => {}
        }
    }

    Ok(())
}

/// Process a non-instantiated import or include (no param bindings), importing values and
/// type-system declarations from the already-evaluated dependency.
///
/// When `is_import` is `true`, only compile-time items (consts, dims, units, types, indexes,
/// dags, assertions) are allowed. Runtime items (params, non-const nodes) trigger an error
/// advising the user to use `include` instead.
/// Check whether a multi-segment `ModulePath` include is a bare module path
/// DAG reference.  This is the case when:
/// 1. The path is a `ModulePath` with 2+ segments, AND
/// 2. The resolved target file's AST contains a `dag` definition whose name
///    matches the last segment of the module path.
///
/// For example, `include pkg/lib/double(...)` where `pkg/lib.gcl` defines
/// `dag double { ... }`.
fn is_bare_module_dag_ref(
    import_path: &ImportPath,
    resolved_canonical: &PathBuf,
    project: &crate::loader::LoadedProject,
) -> bool {
    let segments = match import_path {
        ImportPath::ModulePath { segments, .. } if segments.len() >= 2 => segments,
        _ => return false,
    };

    // Safety: the match guard above ensures segments.len() >= 2
    let Some(last_seg) = segments.last() else {
        return false;
    };
    let last_segment = &last_seg.name;

    // Check if the resolved file contains a DAG with the matching name.
    let Some(target_loaded) = project.files.get(resolved_canonical) else {
        return false;
    };

    target_loaded
        .ast
        .declarations
        .iter()
        .any(|d| matches!(&d.kind, DeclKind::Dag(dag) if dag.name.value.as_str() == last_segment))
}

#[expect(
    clippy::too_many_arguments,
    reason = "import processing needs all these context parameters"
)]
#[expect(
    clippy::too_many_lines,
    reason = "visibility check adds necessary logic to the import processing"
)]
fn process_non_instantiated_import<'a>(
    project: &crate::loader::LoadedProject,
    import_canonical: &PathBuf,
    import_path: &graphcal_compiler::syntax::ast::ImportPath,
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    file_src: &NamedSource<Arc<String>>,
    evaluated_files: &'a HashMap<PathBuf, EvaluatedFile>,
    ctx: &mut ImportContext<'a>,
    is_import: bool,
) -> Result<(), CompileError> {
    let dep = evaluated_files.get(import_canonical).ok_or_else(|| {
        CompileError::Eval(GraphcalError::EvalError {
            message: format!(
                "internal: dependency {} not yet evaluated",
                import_canonical.display()
            ),
            src: file_src.clone(),
            span: import_path.span().into(),
        })
    })?;

    match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();

                // Visibility check: the item must be declared `pub` in the source file.
                if !dep.pub_names.contains(orig_name.as_str()) {
                    // Check if the name exists at all (value or type-system) before
                    // reporting "private" vs "not found".
                    let dep_loaded = &project.files[import_canonical];
                    let exists = dep.const_values.contains_key(orig_name)
                        || dep.values.contains_key(orig_name)
                        || dep.has_assert(orig_name)
                        || file_has_declaration(&dep_loaded.ast, orig_name);
                    if exists {
                        return Err(CompileError::Eval(GraphcalError::ImportPrivateItem {
                            name: orig_name.clone(),
                            file_path: import_path.display_path(),
                            src: file_src.clone(),
                            span: import_item.name.span.into(),
                        }));
                    }
                    return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                        name: orig_name.clone(),
                        file_path: import_path.display_path(),
                        src: file_src.clone(),
                        span: import_item.name.span.into(),
                    }));
                }

                match import_selective_item(
                    dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    &mut ctx.imported_names,
                    &mut ctx.imported_values,
                    Some(&mut ctx.imported_source_order),
                ) {
                    SelectiveImportResult::Const => {}
                    SelectiveImportResult::Runtime => {
                        if is_import {
                            return Err(CompileError::Eval(GraphcalError::ImportRuntimeItem {
                                name: orig_name.clone(),
                                src: file_src.clone(),
                                span: import_item.name.span.into(),
                            }));
                        }
                    }
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
                                file_path: import_path.display_path(),
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
                derive_module_name_from_import_path(import_path, file_src)?
            };
            if let Some((_, first_span)) = ctx.module_map.get(&module_name) {
                return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                    name: module_name,
                    src: file_src.clone(),
                    span: import_path.span().into(),
                    first: (*first_span).into(),
                }));
            }
            ctx.module_map.insert(
                module_name.clone(),
                (import_canonical.clone(), import_path.span()),
            );

            // Import all values under module::name prefix.
            let import_span = import_path.span();
            import_module_values(
                dep,
                &module_name,
                import_span,
                &mut ctx.imported_names,
                &mut ctx.imported_values,
                Some(&mut ctx.imported_source_order),
                is_import,
            );
            // Import all public type-system declarations from dep's registry.
            ctx.extra_registry_builders
                .push((&dep.registry, &dep.pub_names));
        }
    }
    Ok(())
}

/// Rewrite qualified references in the AST when module imports are present.
///
/// If there are no module imports, returns a borrowed reference to the original AST.
/// Otherwise, clones the AST and rewrites `QualifiedGraphRef` and `QualifiedConstRef`
/// to their flat counterparts.
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
            DeclKind::Param(p) => {
                if let Some(ref mut value) = p.value {
                    rewrite_qualified_refs(value);
                }
            }
            DeclKind::Node(n) => rewrite_qualified_refs(&mut n.value),
            DeclKind::ConstNode(c) => rewrite_qualified_refs(&mut c.value),
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
            _ => {}
        }
    }
    // Also rewrite qualified refs in param binding expressions (in include declarations).
    for decl in &mut ast.declarations {
        if let DeclKind::Include(include_decl) = &mut decl.kind {
            for binding in &mut include_decl.param_bindings {
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

    // Merge type-system declarations from module-imported registries (pub items only).
    for (dep_registry, pub_names) in &ctx.extra_registry_builders {
        merge_registry_into_builder_filtered(
            &mut builder,
            dep_registry,
            &HashMap::new(),
            Some(pub_names),
        );
    }

    // Process deferred instantiated imports: compile dep to IR and merge.
    process_deferred_instantiated_imports(
        project,
        &ctx.deferred_instantiated,
        evaluated_files,
        &mut builder,
        &mut unfrozen,
    )?;

    // Process deferred inline DAG includes: compile DAG body to IR and merge.
    process_deferred_inline_dag_includes(
        &ctx.deferred_inline_dags,
        file_src,
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
    crate::dim_check::check_dimensions_tir(&tir, file_src)?;

    let declared_types = tir.build_declared_types(file_src)?;

    for (override_name, override_expr) in &file_overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
            &tir.registry,
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
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
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
            &dep_imported.names,
            dep_imported.values,
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
                && let crate::registry::IndexKind::Range(
                    crate::registry::RangeIndexData { dimension: imp_dim, .. },
                )
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

/// Process deferred inline DAG includes by compiling each DAG body to IR
/// and merging it into the importer's IR.
fn process_deferred_inline_dag_includes(
    deferred_dags: &[DeferredInlineDagInclude],
    file_src: &NamedSource<Arc<String>>,
    builder: &mut RegistryBuilder,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) -> Result<(), CompileError> {
    for deferred in deferred_dags {
        // Compile the DAG body to IR.
        // The DAG body is lowered as if it were a standalone file, with only
        // prelude + explicitly imported items in scope.
        let (dag_builder, dag_unfrozen) = crate::ir::lower_to_builder_with_imported_values(
            &deferred.dag_body,
            file_src,
            &deferred.dag_imported_names,
            HashMap::new(), // No pre-evaluated values for inline DAGs
        )?;

        // Register parent scope type-system declarations in the DAG's registry.
        // These come from `import .. { DimName, UnitName }` in the DAG body.
        let mut dag_builder = dag_builder;
        for parent_decl in &deferred.dag_parent_type_decls {
            let parent_ast = graphcal_compiler::syntax::ast::File {
                declarations: vec![parent_decl.clone()],
            };
            let all_names: HashSet<String> = match &parent_decl.kind {
                DeclKind::BaseDimension(d) => std::iter::once(d.name.value.to_string()).collect(),
                DeclKind::Dimension(d) => std::iter::once(d.name.value.to_string()).collect(),
                DeclKind::Unit(u) => std::iter::once(u.name.value.to_string()).collect(),
                DeclKind::Type(t) => std::iter::once(t.name.value.to_string()).collect(),
                DeclKind::UnionType(t) => std::iter::once(t.name.value.to_string()).collect(),
                DeclKind::Index(idx) => std::iter::once(idx.name.value.to_string()).collect(),
                _ => HashSet::new(),
            };
            if !all_names.is_empty() {
                crate::ir::register_selected_declarations(
                    &parent_ast,
                    &mut dag_builder,
                    file_src,
                    &all_names,
                )?;
            }
        }

        // Merge the DAG's type-system declarations into the importer's registry.
        let dag_registry = dag_builder.build();
        merge_registry_into_builder(builder, &dag_registry, &deferred.index_bindings);

        // Collect all declaration names in the DAG body.
        let mut dep_names: HashSet<String> = HashSet::new();
        for (name, _) in &dag_unfrozen.source_order {
            dep_names.insert(name.member().to_string());
        }

        // Merge the DAG's IR into the importer's IR.
        unfrozen.merge_dependency(
            dag_unfrozen,
            &deferred.prefix,
            &deferred.bindings,
            &dep_names,
            &deferred.index_bindings,
            &deferred.import_item_attributes,
        );

        // For selective imports, add alias nodes.
        if let Some(selective) = &deferred.selective_names {
            add_inline_dag_selective_aliases(&deferred.dag_body, selective, deferred, unfrozen);
        }
    }
    Ok(())
}

/// Add alias declarations for selective inline DAG includes.
fn add_inline_dag_selective_aliases(
    dag_body: &graphcal_compiler::syntax::ast::File,
    selective: &[(String, String)],
    deferred: &DeferredInlineDagInclude,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    for (orig_name, local_name) in selective {
        let prefixed_name = format!("{}::{}", deferred.prefix, orig_name);

        // Find the type annotation from the DAG body's declarations.
        let type_ann = dag_body.declarations.iter().find_map(|d| match &d.kind {
            DeclKind::Param(p) if p.name.value.as_str() == orig_name => Some(p.type_ann.clone()),
            DeclKind::Node(n) if n.name.value.as_str() == orig_name => Some(n.type_ann.clone()),
            DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name => {
                Some(c.type_ann.clone())
            }
            _ => None,
        });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        // Substitute index names in the type annotation.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );

        let is_const = dag_body.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
        );
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

/// Add alias declarations for selective instantiated imports.
///
/// For each selected name, creates either a const or node alias in the importer's IR
/// that references the prefixed declaration from the merged dependency.
fn add_selective_aliases(
    dep_loaded: &crate::loader::LoadedFile,
    selective: &[(String, String)],
    deferred: &DeferredInstantiatedImport,
    unfrozen: &mut graphcal_compiler::ir::lower::UnfrozenIR,
) {
    for (orig_name, local_name) in selective {
        let prefixed_name = format!("{}::{}", deferred.prefix, orig_name);

        // Find the type annotation from the dependency's AST.
        let type_ann = dep_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Param(p) if p.name.value.as_str() == orig_name => {
                    Some(p.type_ann.clone())
                }
                DeclKind::Node(n) if n.name.value.as_str() == orig_name => Some(n.type_ann.clone()),
                DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name => {
                    Some(c.type_ann.clone())
                }
                _ => None,
            });

        let Some(mut type_ann) = type_ann else {
            continue;
        };

        // Substitute index names in the type annotation.
        graphcal_compiler::ir::lower::substitute_type_expr_index_names(
            &mut type_ann,
            &deferred.index_bindings,
        );

        // Determine if this is a const or runtime declaration.
        let is_const = dep_loaded.ast.declarations.iter().any(
            |d| matches!(&d.kind, DeclKind::ConstNode(c) if c.name.value.as_str() == orig_name),
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
    /// An assert was found (caller must handle assert-specific registration).
    Assert,
    /// The name was not found in the evaluated file's values.
    NotFound,
}

/// Look up a single selective import item in an `EvaluatedFile` and register it.
///
/// Handles `const_values` and values (params/nodes).
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
    } else if dep.has_assert(orig_name) {
        SelectiveImportResult::Assert
    } else {
        SelectiveImportResult::NotFound
    }
}

/// Import all values from an `EvaluatedFile` under a module prefix.
///
/// Registers `const_values` and values (params/nodes) with qualified
/// `ScopedName::Qualified` names.
///
/// When `const_only` is `true`, only `const_values` are imported; runtime values
/// (params/nodes) are silently skipped. This is used for `import` statements which
/// only allow compile-time items.
fn import_module_values(
    dep: &EvaluatedFile,
    module_name: &str,
    import_span: Span,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    mut imported_source_order: Option<&mut Vec<(ScopedName, DeclCategory)>>,
    const_only: bool,
) {
    // Sort keys for deterministic ordering — HashMap iteration is arbitrary.
    let mut const_keys: Vec<&String> = dep.const_values.keys().collect();
    const_keys.sort();
    for name in const_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
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

    // Skip runtime values when const_only is true (import semantics).
    if const_only {
        return;
    }

    let mut value_keys: Vec<&String> = dep.values.keys().collect();
    value_keys.sort();
    for name in value_keys {
        // Only import pub items.
        if !dep.pub_names.contains(name.as_str()) {
            continue;
        }
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

    // Process import declarations (non-instantiated).
    for (_decl, import_decl, trans_canonical) in dep_loaded.imports_with_paths() {
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

        build_dep_import_values_for_kind(
            &import_decl.path,
            &import_decl.kind,
            trans_dep,
            dep_src,
            &mut imported_names,
            &mut imported_values,
            true, // is_import: skip runtime items
        );
    }

    // Process include declarations.
    for (_decl, include_decl, trans_canonical) in dep_loaded.includes_with_paths() {
        if !include_decl.param_bindings.is_empty() {
            // Nested instantiated includes are not supported in this initial implementation.
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: "nested instantiated includes are not yet supported".to_string(),
                src: dep_src.clone(),
                span: include_decl.path.span().into(),
            }));
        }

        let trans_dep = evaluated_files.get(trans_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "internal: transitive dependency {} not yet evaluated",
                    trans_canonical.display()
                ),
                src: dep_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        build_dep_import_values_for_kind(
            &include_decl.path,
            &include_decl.kind,
            trans_dep,
            dep_src,
            &mut imported_names,
            &mut imported_values,
            false, // is_import: include allows runtime items
        );
    }

    Ok(DepImportedValues {
        names: imported_names,
        values: imported_values,
    })
}

/// Helper: import values from a dependency according to the import kind.
///
/// When `is_import` is `true`, runtime values are skipped (import semantics).
fn build_dep_import_values_for_kind(
    import_path: &ImportPath,
    import_kind: &graphcal_compiler::syntax::ast::ImportKind,
    trans_dep: &EvaluatedFile,
    dep_src: &NamedSource<Arc<String>>,
    imported_names: &mut ImportedValueNames,
    imported_values: &mut HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    is_import: bool,
) {
    match import_kind {
        graphcal_compiler::syntax::ast::ImportKind::Selective(names) => {
            for import_item in names {
                let orig_name = &import_item.name.name;
                let local_name = import_item.local_name().to_string();
                let result = import_selective_item(
                    trans_dep,
                    orig_name,
                    &local_name,
                    import_item.name.span,
                    imported_names,
                    imported_values,
                    None,
                );
                // For transitive import dependencies, skip runtime items silently
                // (the dep file was already validated; we just don't propagate runtime values
                // through import chains).
                if is_import && matches!(result, SelectiveImportResult::Runtime) {
                    // Runtime item was registered by import_selective_item;
                    // remove it since import doesn't allow runtime items.
                    let scoped = ScopedName::Local(local_name);
                    imported_values.remove(&scoped);
                    imported_names.param_names.retain(|(s, _)| *s != scoped);
                }
            }
        }
        graphcal_compiler::syntax::ast::ImportKind::Module { alias } => {
            let module_name = alias.as_ref().map_or_else(
                || {
                    derive_module_name_from_import_path(import_path, dep_src)
                        .unwrap_or_else(|_| "dep".to_string())
                },
                |alias_ident| alias_ident.name.clone(),
            );
            let import_span = import_path.span();
            import_module_values(
                trans_dep,
                &module_name,
                import_span,
                imported_names,
                imported_values,
                None,
                is_import,
            );
        }
    }
}

/// Evaluate and store a non-root file, producing an [`EvaluatedFile`] for downstream imports.
fn evaluate_and_store_file(
    compiled: CompiledFile,
    file_path: &Path,
    file_src: &NamedSource<Arc<String>>,
    pub_names: HashSet<String>,
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
            pub_names,
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

        // Check params from non-parameterized selective imports and includes
        let selective_imports: Vec<_> = root_file
            .imports_with_paths()
            .map(|(_, d, c)| (&d.kind, c))
            .chain(root_file.includes_with_paths().filter_map(|(_, d, c)| {
                if d.param_bindings.is_empty() {
                    Some((&d.kind, c))
                } else {
                    None
                }
            }))
            .collect();
        for (import_kind, import_canonical) in selective_imports {
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = import_kind {
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
            .any(graphcal_compiler::registry::types::IndexDef::is_required);

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
        let pub_names = extract_pub_names(&project.files[file_path].ast);
        evaluate_and_store_file(
            compiled,
            file_path,
            file_src,
            pub_names,
            &mut evaluated_files,
        )?;
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

    // Process root's direct imports/includes in source order.
    // For each, DFS into its transitive dependencies, propagating the root span.
    // `entry().or_insert()` ensures the first root import/include (in source order) to reach
    // a transitive dep determines its attribution.
    let root_decl_paths: Vec<(Span, PathBuf)> = root_file
        .imports_with_paths()
        .map(|(d, _, c)| (d.span, c.to_path_buf()))
        .chain(
            root_file
                .includes_with_paths()
                .map(|(d, _, c)| (d.span, c.to_path_buf())),
        )
        .collect();
    for (root_span, canonical) in root_decl_paths {
        let mut stack = vec![canonical];
        while let Some(path) = stack.pop() {
            if path == project.root {
                continue;
            }
            // Only process if not already attributed.
            if let std::collections::hash_map::Entry::Vacant(entry) = spans.entry(path.clone()) {
                entry.insert(root_span);
                // Push this file's own imports/includes for transitive propagation.
                if let Some(file) = project.files.get(&path) {
                    for (_decl, _imp, c) in file.imports_with_paths() {
                        if !spans.contains_key(c) {
                            stack.push(c.to_path_buf());
                        }
                    }
                    for (_decl, _inc, c) in file.includes_with_paths() {
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
        let pub_names = extract_pub_names(&project.files[file_path].ast);
        evaluate_and_store_file(
            compiled,
            file_path,
            file_src,
            pub_names,
            &mut evaluated_files,
        )?;
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

        // Check if the root file imports/includes this param from a dependency.
        let mut found = false;
        let selective_decls: Vec<_> = root_file
            .imports_with_paths()
            .map(|(_, d, c)| (&d.kind, c))
            .chain(
                root_file
                    .includes_with_paths()
                    .map(|(_, d, c)| (&d.kind, c)),
            )
            .collect();
        for (import_kind, import_canonical) in selective_decls {
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = import_kind {
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
                    DeclKind::ConstNode(c) if c.name.value.as_str() == name_str => {
                        Some(DeclCategory::Const)
                    }
                    DeclKind::Node(n) if n.name.value.as_str() == name_str => {
                        Some(DeclCategory::Node)
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
    let builtin_consts = crate::builtins::builtin_constants();
    let builtin_fns = crate::builtins::builtin_functions();
    let result =
        super::runtime::run_eval_loop(plan, tir, declared_types, src, builtin_consts, builtin_fns);

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
    merge_registry_into_builder_filtered(builder, dep_registry, index_bindings, None);
}

fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &HashMap<String, String>,
    pub_names: Option<&HashSet<String>>,
) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        if pub_names.is_some_and(|visible| !visible.contains(name)) {
            continue;
        }
        builder.register_base_dimension(
            graphcal_compiler::syntax::names::DimName::new(name),
            id.clone(),
        );
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        if pub_names.is_some_and(|visible| !visible.contains(name.as_str())) {
            continue;
        }
        builder.register_dimension(name.clone(), dim.clone());
    }

    // Import base dimension symbols (for SI unit string display).
    for (id, symbol) in dep_registry.dimensions.base_dim_symbols() {
        builder.set_base_dim_symbol(id.clone(), symbol.clone());
    }

    // Import units.
    for (name, dim, scale) in dep_registry.units.all_units() {
        if pub_names.is_some_and(|visible| !visible.contains(name.as_str())) {
            continue;
        }
        builder.register_unit_dynamic((*name).clone(), dim.clone(), scale.clone());
    }

    // Import indexes — skip bound indexes (they are replaced by the importer's index).
    for idx_def in dep_registry.indexes.all_indexes() {
        if !index_bindings.contains_key(idx_def.name.as_str()) {
            if pub_names.is_some_and(|visible| !visible.contains(idx_def.name.as_str())) {
                continue;
            }
            builder.register_index(idx_def.clone());
        }
    }

    // Import struct types.
    for type_def in dep_registry.types.all_types() {
        if pub_names.is_some_and(|visible| !visible.contains(type_def.name.as_str())) {
            continue;
        }
        builder.register_type(type_def.clone());
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
/// Check for recursive DAG instantiation.
///
/// Builds a dependency graph of inline DAGs and detects cycles.
/// Returns an error if a DAG directly or indirectly includes itself.
fn check_dag_recursion(
    dag_definitions: &HashMap<String, &graphcal_compiler::syntax::ast::DagDecl>,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), CompileError> {
    fn dfs<'a>(
        node: &'a str,
        deps: &HashMap<&str, Vec<&'a str>>,
        visited: &mut HashSet<&'a str>,
        in_stack: &mut HashSet<&'a str>,
        path: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        if in_stack.contains(node) {
            let cycle_start = path.iter().position(|n| *n == node).unwrap_or(0);
            let mut cycle: Vec<String> = path[cycle_start..]
                .iter()
                .map(ToString::to_string)
                .collect();
            cycle.push(node.to_string());
            return Some(cycle);
        }
        if visited.contains(node) {
            return None;
        }
        visited.insert(node);
        in_stack.insert(node);
        path.push(node);

        if let Some(neighbors) = deps.get(node) {
            for &neighbor in neighbors {
                if let Some(cycle) = dfs(neighbor, deps, visited, in_stack, path) {
                    return Some(cycle);
                }
            }
        }

        in_stack.remove(node);
        path.pop();
        None
    }

    // Build adjacency list: dag_name -> set of dag names it includes.
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, dag) in dag_definitions {
        let mut includes = Vec::new();
        for decl in &dag.body {
            if let DeclKind::Include(inc) = &decl.kind
                && let graphcal_compiler::syntax::ast::ImportPath::ModulePath { segments, .. } =
                    &inc.path
                && segments.len() == 1
            {
                let target = segments[0].name.as_str();
                if dag_definitions.contains_key(target) {
                    includes.push(target);
                }
            }
        }
        deps.insert(name.as_str(), includes);
    }

    let mut visited: HashSet<&str> = HashSet::new();
    let mut in_stack: HashSet<&str> = HashSet::new();
    for name in dag_definitions.keys() {
        if let Some(cycle) = dfs(name, &deps, &mut visited, &mut in_stack, &mut Vec::new()) {
            let cycle_str = cycle.join(" -> ");
            return Err(CompileError::Eval(GraphcalError::EvalError {
                message: format!("recursive DAG instantiation: {cycle_str}"),
                src: file_src.clone(),
                span: dag_definitions[name.as_str()].span.into(),
            }));
        }
    }
    Ok(())
}

/// Extract the set of `pub`-declared names from a file's AST.
fn extract_pub_names(file: &graphcal_compiler::syntax::ast::File) -> HashSet<String> {
    let mut pub_names = HashSet::new();
    for decl in &file.declarations {
        if !decl.is_pub {
            continue;
        }
        let name = match &decl.kind {
            DeclKind::Param(p) => p.name.value.to_string(),
            DeclKind::Node(n) => n.name.value.to_string(),
            DeclKind::ConstNode(c) => c.name.value.to_string(),
            DeclKind::Assert(a) => a.name.value.to_string(),
            DeclKind::BaseDimension(d) => d.name.value.to_string(),
            DeclKind::Dimension(d) => d.name.value.to_string(),
            DeclKind::Unit(u) => u.name.value.to_string(),
            DeclKind::Index(idx) => idx.name.value.to_string(),
            DeclKind::Type(t) => t.name.value.to_string(),
            DeclKind::UnionType(u) => u.name.value.to_string(),
            DeclKind::Plot(p) => p.name.value.to_string(),
            DeclKind::Figure(f) => f.name.value.to_string(),
            DeclKind::Layer(l) => l.name.value.to_string(),
            DeclKind::Dag(d) => d.name.value.to_string(),
            DeclKind::Import(_) | DeclKind::Include(_) => continue,
        };
        pub_names.insert(name);
    }
    pub_names
}

/// selective import name is not found among the dependency's evaluated values.
pub(super) fn file_has_declaration(
    file: &graphcal_compiler::syntax::ast::File,
    name: &str,
) -> bool {
    file.declarations.iter().any(|decl| match &decl.kind {
        DeclKind::Param(p) => p.name.value.as_str() == name,
        DeclKind::Node(n) => n.name.value.as_str() == name,
        DeclKind::ConstNode(c) => c.name.value.as_str() == name,
        DeclKind::Assert(a) => a.name.value.as_str() == name,
        DeclKind::BaseDimension(d) => d.name.value.as_str() == name,
        DeclKind::Dimension(d) => d.name.value.as_str() == name,
        DeclKind::Unit(u) => u.name.value.as_str() == name,
        DeclKind::Index(idx) => idx.name.value.as_str() == name,
        DeclKind::Type(t) => t.name.value.as_str() == name,
        DeclKind::UnionType(u) => u.name.value.as_str() == name,
        DeclKind::Plot(p) => p.name.value.as_str() == name,
        DeclKind::Figure(f) => f.name.value.as_str() == name,
        DeclKind::Layer(l) => l.name.value.as_str() == name,
        DeclKind::Dag(d) => d.name.value.as_str() == name,
        DeclKind::Import(_) | DeclKind::Include(_) => false,
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

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

use graphcal_compiler::gcl_err;

use crate::declared_type::DeclaredType;
use crate::error::GraphcalError;
use crate::registry::{Registry, RegistryBuilder};
use crate::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use crate::runtime_value::RuntimeValue;

use super::runtime::evaluate_plan;
use super::types::{AssertResult, CompileError, EvalResult};

mod imports;
mod lowering;
mod pipeline;

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// Helper function to derive a module name from an `ImportPath`.
///
/// For `FilePath`, uses the filename stem.
/// For `ModulePath`, uses the last segment as the module name.
pub(super) fn derive_module_name_from_import_path(
    import_path: &ImportPath,
    src: &NamedSource<Arc<String>>,
) -> Result<String, CompileError> {
    match import_path {
        ImportPath::FilePath { path, span } => {
            crate::loader::derive_module_name(path).map_err(|stem| {
                CompileError::Eval(gcl_err!(InvalidModuleName {
                    stem: stem,
                } @ src, *span))
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
pub(super) struct EvaluatedFile {
    /// Evaluated runtime values (params + nodes): name → `RuntimeValue`.
    pub(super) values: HashMap<String, RuntimeValue>,
    /// Evaluated const values: name → `RuntimeValue`.
    pub(super) const_values: HashMap<String, RuntimeValue>,
    /// Declared types for all consts/params/nodes in this file.
    pub(super) declared_types: HashMap<String, DeclaredType>,
    /// Assertion results from this file: name → (result, span).
    pub(super) assertions: HashMap<DeclName, (AssertResult, Span)>,
    /// The file's frozen registry (for type-system import by downstream files).
    pub(super) registry: Registry,
    /// Names of declarations marked `pub` in the source file.
    /// Used to enforce private-by-default visibility during imports.
    pub(super) pub_names: HashSet<String>,
}

impl EvaluatedFile {
    /// Check whether this file declares an assertion with the given name.
    pub(super) fn has_assert(&self, name: &str) -> bool {
        self.assertions.keys().any(|n| n.as_str() == name)
    }
}

/// The result of compiling a single file within a project context.
///
/// Produced by [`compile_single_file_in_project`] and consumed by the
/// per-file evaluation and TIR compilation pipelines.
pub(super) struct CompiledFile {
    pub(super) tir: crate::tir::TIR,
    pub(super) declared_types: HashMap<String, DeclaredType>,
    /// Imported values for this file (cloned before being consumed by IR).
    /// Used by the root file to enrich output with imported value names.
    pub(super) imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    /// Imported value categories in source order (for root output).
    pub(super) imported_source_order: Vec<(ScopedName, DeclCategory)>,
}

/// Return type for [`build_dep_imported_values`].
pub(super) struct DepImportedValues {
    pub(super) names: ImportedValueNames,
    pub(super) values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
}

/// An instantiated import that needs IR merging (deferred until after lowering).
pub(super) struct DeferredInstantiatedImport {
    /// Canonical path of the dependency file.
    pub(super) dep_path: PathBuf,
    /// The prefix for all merged declarations (from alias or filename).
    pub(super) prefix: String,
    /// Param bindings: `param_name` → binding expression.
    pub(super) bindings: HashMap<String, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    pub(super) index_bindings: HashMap<String, String>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports (all names are accessible via `prefix::`).
    pub(super) selective_names: Option<Vec<(String, String)>>, // (orig_name, local_name)
    /// Span of the import declaration (for diagnostics).
    pub(super) import_span: Span,
    /// Per-import-item attributes (e.g., `#[expected_fail(...)]` on imported assertions).
    /// Key = original name in dep, Value = list of attributes from the import item.
    pub(super) import_item_attributes:
        HashMap<String, Vec<graphcal_compiler::syntax::ast::Attribute>>,
}

/// A deferred inline DAG include that needs IR merging.
pub(super) struct DeferredInlineDagInclude {
    /// Virtual File AST constructed from the DAG body declarations.
    pub(super) dag_body: graphcal_compiler::syntax::ast::File,
    /// Imported names collected from `import ..` inside the DAG body.
    pub(super) dag_imported_names: ImportedValueNames,
    /// Type-system declarations imported from parent scope via `import ..`.
    pub(super) dag_parent_type_decls: Vec<graphcal_compiler::syntax::ast::Declaration>,
    /// The prefix for all merged declarations (from alias or dag name).
    pub(super) prefix: String,
    /// Param bindings: `param_name` → binding expression.
    pub(super) bindings: HashMap<String, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    pub(super) index_bindings: HashMap<String, String>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports.
    pub(super) selective_names: Option<Vec<(String, String)>>,
    /// Span of the include declaration (for diagnostics).
    pub(super) import_span: Span,
    /// Per-import-item attributes.
    pub(super) import_item_attributes:
        HashMap<String, Vec<graphcal_compiler::syntax::ast::Attribute>>,
}

/// Mutable state accumulated while processing import declarations.
///
/// Bundles the various collections that [`compile_single_file_in_project`] builds
/// during its import-processing loop, avoiding excessive parameter counts in the
/// extracted helper functions.
pub(super) struct ImportContext<'a> {
    pub(super) imported_names: ImportedValueNames,
    pub(super) imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    pub(super) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(super) imported_type_system_names: HashMap<PathBuf, HashSet<String>>,
    pub(super) module_map: HashMap<String, (PathBuf, Span)>,
    /// Registry + `pub_names` for module-imported dependencies.
    pub(super) extra_registry_builders: Vec<(&'a Registry, &'a HashSet<String>)>,
    pub(super) deferred_instantiated: Vec<DeferredInstantiatedImport>,
    pub(super) deferred_inline_dags: Vec<DeferredInlineDagInclude>,
}

/// Result of looking up a single selective import item in an `EvaluatedFile`.
pub(super) enum SelectiveImportResult {
    /// A const value was found and registered.
    Const,
    /// A runtime value (param/node) was found and registered.
    Runtime,
    /// An assert was found (caller must handle assert-specific registration).
    Assert,
    /// The name was not found in the evaluated file's values.
    NotFound,
}

/// Rewrite qualified references in the AST when module imports are present.
///
/// If there are no module imports, returns a borrowed reference to the original AST.
/// Otherwise, clones the AST and rewrites `QualifiedGraphRef` and `QualifiedConstRef`
/// to their flat counterparts.
pub(super) fn rewrite_qualified_refs_in_ast<'a>(
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

/// Extract the set of `pub`-declared names from a file's AST.
pub(super) fn extract_pub_names(file: &graphcal_compiler::syntax::ast::File) -> HashSet<String> {
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

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

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
    pipeline::compile_to_tir_project_perfile(project)
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
    pipeline::evaluate_project_perfile(project, overrides, allow_defaults)
}

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

//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::syntax::ast::{DeclKind, Expr, ExprKind, ModulePath};
use graphcal_compiler::syntax::names::{DeclName, DimName, IndexName, Spanned, StructTypeName};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::visitor::ExprVisitorMut;

use graphcal_compiler::ir::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{Registry, RegistryBuilder};

use super::runtime::evaluate_plan;
use super::types::{AssertResult, CompileError, EvalResult};

mod imports;
mod lowering;
mod pipeline;

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// Derive a module name (the leaf segment) from a `ModulePath`.
///
/// Used as the include-instance alias for the bare `include path(args);`
/// form and as the module-qualifier name for `import path;`.
pub(super) fn derive_module_name_from_import_path(import_path: &ModulePath) -> String {
    import_path
        .leaf()
        .map_or_else(|| "module".to_string(), |seg| seg.name.clone())
}

/// Visitor that rewrites qualified const references to flat names.
struct QualifiedRefRewriter;

impl ExprVisitorMut for QualifiedRefRewriter {
    type Error = std::convert::Infallible;

    fn visit_qualified_const_ref_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        let old_kind = std::mem::replace(&mut expr.kind, ExprKind::Number(0.0));
        expr.kind = match old_kind {
            ExprKind::QualifiedConstRef { module, name } => {
                // The internal HashMap key encoding remains `m::x` to avoid
                // collisions with user-visible `.`-separated names.
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

/// Rewrite qualified const references in-place.
///
/// Replaces `QualifiedConstRef { module: "m", name: "x" }` with
/// `ConstRef("m::x")`.
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
    /// Compiled dag TIRs for each `dag { ... }` declared in this file.
    ///
    /// Keyed by bare dag name. Cloned into downstream importers' `TIR::dags`
    /// under `"alias::dag_name"` keys so qualified inline calls
    /// (`@alias::dag(args)::out`) resolve through the same machinery as
    /// same-file inline calls.
    pub(super) dag_tirs: HashMap<String, graphcal_compiler::tir::typed::TIR>,
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
    pub(super) tir: graphcal_compiler::tir::typed::TIR,
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
    /// DAG identifier of the dependency.
    pub(super) dep_dag_id: graphcal_compiler::syntax::dag_id::DagId,
    /// The prefix for all merged declarations (from alias or filename).
    pub(super) prefix: String,
    /// Param bindings: `param_name` → binding expression.
    pub(super) bindings: HashMap<String, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    pub(super) index_bindings: HashMap<IndexName, IndexName>,
    /// Type bindings: `dep_type_name` → `importer_type_name`.
    pub(super) type_bindings: HashMap<StructTypeName, StructTypeName>,
    /// Dimension bindings: `dep_dim_name` → `importer_dim_name`.
    pub(super) dim_bindings: HashMap<DimName, DimName>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports (all names are accessible via `prefix::`).
    pub(super) selective_names: Option<Vec<(String, String)>>, // (orig_name, local_name)
    /// Span of the import declaration (for diagnostics).
    pub(super) import_span: Span,
    /// Per-import-item attributes (e.g., `#[expected_fail(...)]` on imported assertions).
    /// Key = original name in dep, Value = list of attributes from the import item.
    pub(super) import_item_attributes:
        HashMap<String, Vec<graphcal_compiler::syntax::ast::Attribute>>,
    /// Whether this include carries a leading `pub` (whole-module re-export, issue #452).
    pub(super) pub_reexport_whole: bool,
    /// Original names (in the dep) of selective items marked `pub` in the
    /// importer's brace list (issue #452). Empty for whole-module form.
    pub(super) pub_reexport_items: HashSet<String>,
}

/// A deferred inline DAG include that needs IR merging.
pub(super) struct DeferredInlineDagInclude {
    /// Virtual File AST constructed from the DAG body declarations.
    pub(super) dag_body: graphcal_compiler::syntax::ast::File,
    /// Imported names collected from `import ..` inside the DAG body.
    pub(super) dag_imported_names: ImportedValueNames,
    /// The prefix for all merged declarations (from alias or dag name).
    pub(super) prefix: String,
    /// Param bindings: `param_name` → binding expression.
    pub(super) bindings: HashMap<String, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    pub(super) index_bindings: HashMap<IndexName, IndexName>,
    /// Type bindings: `dep_type_name` → `importer_type_name`.
    pub(super) type_bindings: HashMap<StructTypeName, StructTypeName>,
    /// Dimension bindings: `dep_dim_name` → `importer_dim_name`.
    pub(super) dim_bindings: HashMap<DimName, DimName>,
    /// For selective imports: the selected names and their local aliases.
    /// `None` for module imports.
    pub(super) selective_names: Option<Vec<(String, String)>>,
    /// Span of the include declaration (for diagnostics).
    pub(super) import_span: Span,
    /// Per-import-item attributes.
    pub(super) import_item_attributes:
        HashMap<String, Vec<graphcal_compiler::syntax::ast::Attribute>>,
    /// Whether this include carries a leading `pub` (whole-module re-export, issue #452).
    pub(super) pub_reexport_whole: bool,
    /// Original names (in the DAG body) of selective items marked `pub` in
    /// the importer's brace list (issue #452). Empty for whole-module form.
    pub(super) pub_reexport_items: HashSet<String>,
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
    pub(super) imported_type_system_names:
        HashMap<graphcal_compiler::syntax::dag_id::DagId, HashSet<String>>,
    pub(super) module_map: HashMap<String, (graphcal_compiler::syntax::dag_id::DagId, Span)>,
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
    module_map: &HashMap<String, (graphcal_compiler::syntax::dag_id::DagId, Span)>,
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

/// Extract the set of names visible to importers of a file.
///
/// Explicitly `pub`/`pub(bind)` declarations contribute. Params are
/// implicitly visible under the A5 rule ("params are always visible
/// and bindable") and therefore always contribute regardless of
/// annotation.
///
/// Selective `import "X" { pub name }` re-exports `name` at this file
/// per issue #452 — those names also contribute. Whole-module
/// `pub import "X";` / `pub include "X";` re-exports every `pub` item
/// from X; that form is resolved transitively during import processing
/// (the enumeration requires X's own `pub_names`, which this pure AST
/// walk does not have), so it is not expanded here.
pub(super) fn extract_pub_names(file: &graphcal_compiler::syntax::ast::File) -> HashSet<String> {
    let mut pub_names = HashSet::new();
    for decl in &file.declarations {
        let implicitly_visible = matches!(decl.kind, DeclKind::Param(_));
        if !decl.is_pub() && !implicitly_visible {
            match &decl.kind {
                DeclKind::Import(d) => {
                    if let graphcal_compiler::syntax::ast::ImportKind::Selective(items) = &d.kind {
                        for item in items {
                            if item.is_pub {
                                pub_names.insert(item.local_name().to_string());
                            }
                        }
                    }
                }
                DeclKind::Include(d) => {
                    if let graphcal_compiler::syntax::ast::ImportKind::Selective(items) = &d.kind {
                        for item in items {
                            if item.is_pub {
                                pub_names.insert(item.local_name().to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
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
            DeclKind::Multi(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
        };
        pub_names.insert(name);
    }
    pub_names
}

/// selective import name is not found among the dependency's evaluated values.
///
/// Also matches re-exported names introduced by `import "X" { pub name }` or
/// `include "X" { pub name }`. Issue #452 — a re-exported name stands in for a
/// local declaration when a downstream importer asks "does this file have
/// `name`?".
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
        DeclKind::Import(d) => matches!(
            &d.kind,
            graphcal_compiler::syntax::ast::ImportKind::Selective(items)
                if items.iter().any(|it| it.is_pub && it.local_name() == name)
        ),
        DeclKind::Include(d) => matches!(
            &d.kind,
            graphcal_compiler::syntax::ast::ImportKind::Selective(items)
                if items.iter().any(|it| it.is_pub && it.local_name() == name)
        ),
        DeclKind::Multi(_) => graphcal_compiler::syntax::desugar::unreachable_post_desugar(),
    })
}

/// Resolve a struct field's declared type, handling generic type parameter substitution.
///
/// If the field's type annotation references a generic type parameter (e.g., `D` in
/// `Vec3<D: Dim, F: Type>`), the substitution map provides the concrete type.
/// Otherwise, falls back to direct registry resolution.
pub(super) fn resolve_field_declared_type(
    field: &graphcal_compiler::registry::types::StructField,
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
    ir: &mut graphcal_compiler::ir::lower::IR,
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
        graphcal_compiler::ir::resolve::collect_graph_refs(
            override_expr,
            &all_runtime,
            &mut graph_refs,
        );
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
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
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
) -> Result<EvalResult, CompileError> {
    pipeline::evaluate_project_perfile(project, overrides)
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
    fs: &F,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    compile_and_eval_from_project(&project, overrides)
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
pub fn compile_to_tir(
    source: &str,
    name: &str,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
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
) -> Result<
    (
        graphcal_compiler::tir::typed::TIR,
        crate::loader::LoadedProject,
    ),
    CompileError,
> {
    let project = crate::loader::load_project(root_path, project_root, fs)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

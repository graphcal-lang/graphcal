//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::{DeclKind, Expr, ExprKind, ModulePath};
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::dimension::{DimName, UnitRef};
use graphcal_compiler::syntax::index_name::IndexName;
use graphcal_compiler::syntax::module_name::ModuleAliasName;
use graphcal_compiler::syntax::phase::Desugared;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::span::Spanned;
use graphcal_compiler::syntax::type_name::StructTypeName;
use graphcal_compiler::syntax::visitor::ExprVisitorMut;

pub(in crate::eval::project) use crate::import_surface::{
    ImportItemPresence, decl_is_public, extract_pub_names, file_exports_import_item,
    file_has_import_item, file_import_item_presence,
};
use graphcal_compiler::ir::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{PositiveFiniteScale, Registry, RegistryBuilder};

use super::runtime::evaluate_plan;
use super::types::{AssertResult, CompileError, EvalResult};

mod imports;
mod lowering;
mod pipeline;

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// A binding map whose **key** is the dependency-side name and whose **value**
/// is the importer-side name the dep name resolves to. Used for index, type,
/// and dim bindings on instantiated includes. The aliased name keeps the
/// directional convention discoverable everywhere the map shape appears in a
/// signature.
pub(in crate::eval::project) type DepToImporter<T> = HashMap<T, T>;

/// A selective import/include alias.
///
/// `original` is the dependency-side declaration name; `local` is the name
/// introduced into the importer. Keeping the two roles named prevents call
/// sites from swapping a raw `(String, String)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::eval::project) struct ImportAlias {
    pub(in crate::eval::project) original: DeclName,
    pub(in crate::eval::project) local: DeclName,
}

/// Derive a module name (the leaf segment) from a `ModulePath`.
///
/// Used as the include-instance alias for the bare `include path(args);`
/// form and as the module-qualifier name for `import path;`.
pub(in crate::eval::project) fn derive_module_name_from_import_path(
    import_path: &ModulePath,
) -> ModuleAliasName {
    ModuleAliasName::from_atom(import_path.leaf().name.clone())
}

/// Visitor that recognizes `FieldAccess(GraphRef(alias), field)` and rewrites
/// it to a qualified `GraphRef` when `(alias, field)` matches an imported
/// module-namespace member.
///
/// `@bar.field` parses as `FieldAccess(GraphRef(bar), field)`. For the
/// `include foo() as bar;` and `import foo as bar;` namespace forms, the
/// dependency's items are registered as qualified `ScopedName`s. The rewriter
/// promotes the access to a typed qualified `GraphRef` directly — no
/// `Qualified*Ref` variant or flat-string boundary involved.
struct AliasFieldAccessRewriter<'a> {
    qualified_pairs: &'a HashSet<QualifiedMember>,
}

impl ExprVisitorMut<Desugared> for AliasFieldAccessRewriter<'_> {
    type Error = std::convert::Infallible;

    fn visit_expr_mut(&mut self, expr: &mut Expr) -> Result<(), Self::Error> {
        // Recurse first; chained `@bar.x.y` becomes
        // `FieldAccess(FieldAccess(GraphRef(bar), x), y)`. We promote the
        // inner `FieldAccess(GraphRef(bar), x)` to a qualified `GraphRef`,
        // leaving the outer `.y` as a struct-field access on the
        // resulting qualified node value.
        self.dispatch_mut(expr)?;

        let promote = if let ExprKind::FieldAccess { expr: inner, field } = &expr.kind
            && let ExprKind::GraphRef(qualifier_name) = &inner.kind
            && !qualifier_name.value.is_qualified()
            && self.qualified_pairs.contains(&QualifiedMember {
                module: ModuleAliasName::expect_valid(qualifier_name.value.member()),
                member: DeclName::from_atom(field.value.atom().clone()),
            }) {
            let merged_span = qualifier_name.span.merge(field.span);
            Some(ExprKind::GraphRef(Spanned {
                value: ScopedName::qualified(qualifier_name.value.member(), field.value.as_str()),
                span: merged_span,
            }))
        } else {
            None
        };
        if let Some(kind) = promote {
            expr.kind = kind;
        }
        Ok(())
    }
}

/// `(module, member)` pair identifying a qualified import alias for the
/// field-access rewriter — distinct from a flat `(String, String)` tuple so
/// the two halves cannot be swapped at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct QualifiedMember {
    module: ModuleAliasName,
    member: DeclName,
}

/// Promote `FieldAccess(GraphRef(Local(alias)), field)` to a qualified
/// `GraphRef` in-place. This is the only producer of qualified graph
/// references in the project pipeline — qualified const references come
/// out of reference resolution directly.
fn rewrite_alias_field_access(expr: &mut Expr, qualified_pairs: &HashSet<QualifiedMember>) {
    if qualified_pairs.is_empty() {
        return;
    }
    let mut rewriter = AliasFieldAccessRewriter { qualified_pairs };
    let _ = rewriter.visit_expr_mut(expr);
}

// ---------------------------------------------------------------------------
// Per-file evaluation types and pipeline
// ---------------------------------------------------------------------------

/// The compiled dependency artifact for a single file in the per-file pipeline.
///
/// Files with required params or indexes cannot be evaluated standalone, but
/// downstream compile-time imports still need their registry, public names,
/// declared types, const values, and DAG metadata. For those files,
/// `runtime_available` is `false` and `values` / `assertions` are empty.
pub(in crate::eval::project) struct EvaluatedFile {
    /// Whether runtime values and assertion results were evaluated.
    pub(in crate::eval::project) runtime_available: bool,
    /// Evaluated runtime values (params + nodes): name → `RuntimeValue`.
    pub(in crate::eval::project) values: HashMap<DeclName, RuntimeValue>,
    /// Evaluated const values: name → `RuntimeValue`.
    pub(in crate::eval::project) const_values: HashMap<DeclName, RuntimeValue>,
    /// Declared types for all consts/params/nodes in this file.
    pub(in crate::eval::project) declared_types: HashMap<ScopedName, DeclaredType>,
    /// Assertion results from this file: name → (result, span).
    /// Names keep alias qualification for include-instantiated asserts (#813).
    pub(in crate::eval::project) assertions: HashMap<ScopedName, (AssertResult, Span)>,
    /// Evaluated plot specs of this file, keyed by their local (leaf) name.
    /// Consumers request them through include brace lists (#847).
    pub(in crate::eval::project) plots: HashMap<DeclName, super::types::PlotSpec>,
    /// The file's frozen registry (for type-system import by downstream files).
    pub(in crate::eval::project) registry: Registry,
    /// Names of declarations marked `pub` in the source file.
    /// Used to enforce private-by-default visibility during imports.
    pub(in crate::eval::project) pub_names: HashSet<DeclName>,
    /// Concrete scale factors for this file's own dynamic units, resolved
    /// against the file's evaluated runtime values. Module importers convert
    /// the dynamic units to static scales at registry-merge time — the scale
    /// expression references this file's params and cannot be re-evaluated in
    /// the importer's context. Empty when `runtime_available` is `false`.
    pub(in crate::eval::project) resolved_dynamic_unit_scales:
        HashMap<UnitRef, PositiveFiniteScale>,
    /// Compiled dag TIRs for each `dag { ... }` declared in this file.
    ///
    /// Keyed by bare dag name. Cloned into downstream importers' `TIR::dags`
    /// under `"alias::dag_name"` keys so qualified inline calls
    /// (`@alias.dag(args).out`) resolve through the same machinery as
    /// same-file inline calls. The internal `::` separator avoids collisions
    /// with user-visible `.`-separated names.
    pub(in crate::eval::project) dag_tirs: graphcal_compiler::tir::typed::DagRegistry,
}

impl EvaluatedFile {
    /// Check whether this file has an evaluated top-level assertion with the
    /// given (bare local) name.
    pub(in crate::eval::project) fn has_assert(&self, name: &str) -> bool {
        self.assertions.contains_key(&ScopedName::local(name))
    }
}

/// The result of compiling a single file within a project context.
///
/// Produced by [`compile_single_file_in_project`] and consumed by the
/// per-file evaluation and TIR compilation pipelines.
pub(in crate::eval::project) struct CompiledFile {
    pub(in crate::eval::project) tir: graphcal_compiler::tir::typed::TIR,
    pub(in crate::eval::project) declared_types: HashMap<ScopedName, DeclaredType>,
    /// Imported values for this file (cloned before being consumed by IR).
    /// Used by the root file to enrich output with imported value names.
    pub(in crate::eval::project) imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    /// Imported value categories in source order (for root output).
    pub(in crate::eval::project) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    /// Plot specs requested from standalone-evaluated dependencies via
    /// include brace lists, renamed to their local aliases (#847).
    pub(in crate::eval::project) included_plots: Vec<super::types::PlotSpec>,
}

/// Return type for [`build_dep_imported_values`].
pub(in crate::eval::project) struct DepImportedValues {
    pub(in crate::eval::project) names: ImportedValueNames,
    pub(in crate::eval::project) values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
}

/// A deferred include of a DAG (file-level or inline) — compile its body
/// and merge into the importer's IR after the importer's own decls are
/// lowered.
///
/// A file include (`include lib(args).{...}`) is a DAG whose source is
/// the file root; an inline DAG include (`include dag(args)` /
/// `include lib.dag(args)`) is a DAG inside some file. After the flat
/// TIR registry, both are uniformly addressed by canonical [`DagId`].
pub(in crate::eval::project) struct DeferredDagInclude {
    /// Identifies the source kind and any kind-specific data (file's AST
    /// vs inline dag's body + parent context).
    pub(in crate::eval::project) source: DeferredDagSource,
    /// The prefix for all merged declarations (from alias or dag name or
    /// filename).
    pub(in crate::eval::project) prefix: ModuleAliasName,
    /// Param bindings: `param_name` → binding expression.
    pub(in crate::eval::project) bindings: HashMap<DeclName, Expr>,
    /// Index bindings: `dep_index_name` → `importer_index_name`.
    pub(in crate::eval::project) index_bindings: DepToImporter<IndexName>,
    /// Type bindings: `dep_type_name` → `importer_type_name`.
    pub(in crate::eval::project) type_bindings: DepToImporter<StructTypeName>,
    /// Dimension bindings: `dep_dim_name` → `importer_dim_name`.
    pub(in crate::eval::project) dim_bindings: DepToImporter<DimName>,
    /// For selective includes: the selected names and their local aliases.
    /// `None` for module-form includes (all names accessible via `prefix::`).
    pub(in crate::eval::project) selective_names: Option<Vec<ImportAlias>>,
    /// Plots requested by the include brace list, keyed by the dep-side
    /// plot name (#847).
    pub(in crate::eval::project) requested_plots:
        HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot>,
    /// Span of the include declaration (for diagnostics).
    pub(in crate::eval::project) import_span: Span,
    /// Per-import-item attributes (e.g., `#[expected_fail(...)]` on
    /// included assertions). Key = original name in dep.
    pub(in crate::eval::project) import_item_attributes:
        HashMap<DeclName, Vec<graphcal_compiler::desugar::desugared_ast::Attribute>>,
    /// Whether this include carries a leading `pub` (whole-module re-export).
    pub(in crate::eval::project) pub_reexport_whole: bool,
    /// Original names of selective items marked `pub` in the importer's
    /// brace list. Empty for whole-module form.
    pub(in crate::eval::project) pub_reexport_items: HashSet<DeclName>,
}

/// What is being included — distinguishes file roots from inline DAGs and
/// carries the kind-specific data the deferred processor needs.
pub(in crate::eval::project) enum DeferredDagSource {
    /// File include — body is the dep file's full AST, with its own
    /// transitive imports' values supplied via
    /// [`build_dep_imported_values`].
    File {
        /// Canonical [`DagId`](graphcal_compiler::dag_id::DagId)
        /// of the dep file (equal to the file's root id).
        dep_dag_id: graphcal_compiler::dag_id::DagId,
    },
    /// Inline DAG include — body is the dag block's declarations, with
    /// `import <self>.{...}` items resolved against `parent_dag_id`
    /// (Concept 9: the file that *defined* the DAG, not the file
    /// performing the include).
    InlineDag {
        /// Virtual File AST constructed from the DAG body declarations.
        dag_body: graphcal_compiler::desugar::desugared_ast::File,
        /// Imported names collected from `import ..` inside the DAG body.
        dag_imported_names: ImportedValueNames,
        /// Canonical identity of the included DAG module.
        dag_id: graphcal_compiler::dag_id::DagId,
        /// [`DagId`](graphcal_compiler::dag_id::DagId) of the file
        /// where this DAG was *defined*. For same-file includes this is
        /// the importer; for cross-file qualified includes it's the target
        /// file.
        parent_dag_id: graphcal_compiler::dag_id::DagId,
    },
}

/// Mutable state accumulated while processing import declarations.
///
/// Bundles the various collections that [`compile_single_file_in_project`] builds
/// during its import-processing loop, avoiding excessive parameter counts in the
/// extracted helper functions.
pub(in crate::eval::project) struct ImportContext<'a> {
    pub(in crate::eval::project) imported_names: ImportedValueNames,
    pub(in crate::eval::project) imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    pub(in crate::eval::project) imported_source_order: Vec<(ScopedName, DeclCategory)>,
    pub(in crate::eval::project) imported_type_system_names: HashMap<
        graphcal_compiler::dag_id::DagId,
        graphcal_compiler::ir::lower::SelectedDeclarations,
    >,
    pub(in crate::eval::project) module_map:
        HashMap<ModuleAliasName, (graphcal_compiler::dag_id::DagId, Span)>,
    /// Registry surfaces of module-imported dependencies, merged into the
    /// importer's registry builder before its own declarations register.
    pub(in crate::eval::project) extra_registry_builders: Vec<ModuleRegistryImport<'a>>,
    pub(in crate::eval::project) deferred_dag_includes: Vec<DeferredDagInclude>,
    /// Plot specs requested from standalone-evaluated dependencies via
    /// include brace lists, renamed to their local aliases (#847).
    pub(in crate::eval::project) included_plot_specs: Vec<super::types::PlotSpec>,
}

/// A module-imported dependency's registry surface, queued for merging into
/// the importer's registry builder.
pub(in crate::eval::project) struct ModuleRegistryImport<'a> {
    /// The dependency's frozen registry.
    pub(in crate::eval::project) registry: &'a Registry,
    /// Names declared `pub` in the dependency — only these cross the boundary.
    pub(in crate::eval::project) pub_names: &'a HashSet<DeclName>,
    /// The import alias that keys the dependency's `pub` units in the
    /// importer's unit scope (`alias.unit`).
    pub(in crate::eval::project) unit_alias: ModuleAliasName,
    /// Concrete scales for the dependency's dynamic units, resolved against
    /// its evaluated runtime values. Dynamic units merge as static scales —
    /// their scale expressions reference the dependency's own params and
    /// cannot be re-evaluated in the importer's context.
    pub(in crate::eval::project) resolved_dynamic_scales: &'a HashMap<UnitRef, PositiveFiniteScale>,
    /// The import-statement span, localizing merge-conflict diagnostics.
    pub(in crate::eval::project) import_span: Span,
}

/// Result of looking up a single selective import item in an `EvaluatedFile`.
#[derive(Debug)]
pub(in crate::eval::project) enum SelectiveImportResult {
    /// A const value was found and registered.
    Const,
    /// A runtime value (param/node) was found and registered.
    Runtime,
    /// An assert was found (caller must handle assert-specific registration).
    Assert,
    /// The name was not found in the evaluated file's values.
    NotFound,
}

/// Resolve namespace-alias graph references in the AST: rewrite
/// `FieldAccess(GraphRef(Local(alias)), field)` to a qualified
/// `GraphRef(Qualified { module: alias, member: field })` when
/// `(alias, field)` matches an imported namespace member. The
/// qualification is preserved structurally throughout the IR / eval
/// pipeline — there is no flat-string boundary.
///
/// If there are no module imports and no qualified members, returns a
/// borrowed reference to the original AST.
pub(in crate::eval::project) fn rewrite_qualified_refs_in_ast<'a>(
    ast: &'a graphcal_compiler::desugar::desugared_ast::File,
    module_map: &HashMap<ModuleAliasName, (graphcal_compiler::dag_id::DagId, Span)>,
    imported_names: &ImportedValueNames,
) -> std::borrow::Cow<'a, graphcal_compiler::desugar::desugared_ast::File> {
    let alias_pairs = collect_qualified_pairs(imported_names);
    if module_map.is_empty() && alias_pairs.is_empty() {
        return std::borrow::Cow::Borrowed(ast);
    }

    let mut ast = ast.clone();
    for decl in &mut ast.declarations {
        rewrite_decl_exprs(decl, &alias_pairs);
    }
    std::borrow::Cow::Owned(ast)
}

/// Collect `(module, member)` pairs from imported namespace registrations.
///
/// Module-form `import`/`include` registers each dep declaration as qualified
/// `ScopedName`s; selective imports register local `ScopedName`s. The pairs
/// returned here drive the `@alias.member` rewrite — bare locals do not
/// participate.
fn collect_qualified_pairs(imported: &ImportedValueNames) -> HashSet<QualifiedMember> {
    let mut pairs = HashSet::new();
    let entries = imported
        .const_names
        .iter()
        .chain(imported.param_names.iter())
        .chain(imported.node_names.iter());
    for (scoped, _) in entries {
        if let [module] = scoped.qualifier() {
            pairs.insert(QualifiedMember {
                module: ModuleAliasName::expect_valid(module.as_ref()),
                member: DeclName::expect_valid(scoped.member()),
            });
        }
    }
    pairs
}

/// Apply the alias-field-access rewrite to a single declaration's expressions.
fn rewrite_decl_exprs(
    decl: &mut graphcal_compiler::desugar::desugared_ast::Declaration,
    alias_pairs: &HashSet<QualifiedMember>,
) {
    let rewrite = |e: &mut Expr| {
        rewrite_alias_field_access(e, alias_pairs);
    };
    match &mut decl.kind {
        DeclKind::Param(p) => {
            if let Some(ref mut value) = p.value {
                rewrite(value);
            }
        }
        DeclKind::Node(n) => rewrite(&mut n.value),
        DeclKind::ConstNode(c) => rewrite(&mut c.value),
        DeclKind::Assert(a) => match &mut a.body {
            graphcal_compiler::desugar::desugared_ast::AssertBody::Expr(e) => rewrite(e),
            graphcal_compiler::desugar::desugared_ast::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                rewrite(actual);
                rewrite(expected);
                rewrite(tolerance);
            }
        },
        DeclKind::Include(include_decl) => {
            for binding in &mut include_decl.param_bindings {
                rewrite(&mut binding.value);
            }
        }
        _ => {}
    }
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
    if let graphcal_compiler::desugar::desugared_ast::TypeExprKind::DimExpr(dim_expr) =
        &field.type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
        && let Some(name) = dim_expr.terms[0]
            .term
            .name
            .value
            .as_bare()
            .map(graphcal_compiler::syntax::names::NameAtom::as_str)
        && let Some(concrete) = generic_sub.get(name)
    {
        return Some((*concrete).clone());
    }
    // Non-generic: resolve directly from the registry. Overflow in dimension
    // arithmetic is treated as "no declared type info" here — the value will
    // render as a raw scalar, and dim_check would have already flagged the
    // overflow as a real error during compilation.
    registry
        .dimensions
        .resolve_type_expr(&field.type_ann)
        .ok()
        .flatten()
        .map(DeclaredType::Scalar)
}

/// Validate and apply parameter overrides to an IR.
pub(in crate::eval::project) fn apply_overrides(
    ir: &mut graphcal_compiler::ir::lower::UnfrozenIR,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
) -> Result<(), CompileError> {
    for (override_name, override_expr) in overrides {
        let name_str = override_name.as_str();
        let matches = ir
            .source_order
            .iter()
            .filter(|(name, _)| name.member() == name_str)
            .collect::<Vec<_>>();
        let param_matches = matches
            .iter()
            .filter_map(|(name, cat)| matches!(cat, DeclCategory::Param).then_some((*name).clone()))
            .collect::<Vec<_>>();
        let target_name = match param_matches.as_slice() {
            [name] => name,
            [] => {
                if let Some((_, non_param_cat)) = matches.first() {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: *non_param_cat,
                    }));
                }
                return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                    name: override_name.clone(),
                }));
            }
            candidates => {
                return Err(CompileError::Eval(GraphcalError::OverrideAmbiguousParam {
                    name: override_name.clone(),
                    candidates: candidates.to_vec(),
                }));
            }
        };

        // Runtime dependencies are recomputed from the lowered HIR at the
        // freeze boundary, so the replaced default needs no dep bookkeeping.
        ir.override_param_default(target_name, override_expr.clone());
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
/// Uses per-file evaluation: each file is compiled in topological order, and
/// files that can run standalone are evaluated independently. Library files
/// with required runtime inputs keep compile-time artifacts only. Import
/// declarations bind evaluated dependency values when available. All evaluated
/// assertions are aggregated.
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
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
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
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
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
/// Returns a [`CompileError`] if parsing, lowering, or checking fails, or if
/// `name` is not a valid `.gcl` source path.
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

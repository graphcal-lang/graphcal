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
use graphcal_compiler::syntax::module_name::{IncludeInstanceScope, ModuleAliasName};
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::phase::Desugared;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::span::Spanned;
use graphcal_compiler::syntax::type_name::StructTypeName;
use graphcal_compiler::syntax::visitor::ExprVisitorMut;

pub(in crate::eval::project) use crate::import_surface::{
    ImportItemPresence, decl_has_external_role, extract_external_decl_surface,
    file_exposes_import_item, file_has_import_item, file_import_item_presence,
};
use graphcal_compiler::ir::imported_binding::ImportedBinding;
use graphcal_compiler::ir::resolve::{DeclCategory, ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::registry::runtime_value::RuntimeValue;
use graphcal_compiler::registry::types::{
    IndexBindingTarget, PositiveFiniteScale, Registry, RegistryBuilder,
};

use super::types::{AssertResult, CompileError, DeclType, EvalResult, NodeError, Value};

mod imports;
mod lowering;
mod pipeline;
mod prepared;

pub use prepared::{
    InclusiveBounds, ModelConstructorSchema, ModelDefinitionError, ModelExecutionError,
    ModelFieldSchema, ModelIndexKind, ModelIndexSchema, ModelOutputPort, ModelQuantitySchema,
    ModelRowFailure, ModelRowOutcome, ModelUnitSchema, ModelValueSchema, ParameterBindingBuilder,
    ParameterBindingRow, ParameterDomain, ParameterPort, ParameterPosition, ParameterValue,
    PreparedModel, PreparedProject, TenaxV2Input, TenaxV2InputKind, TenaxV2Model, TenaxV2Output,
    TenaxV2RowOutcome,
};

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// A binding map whose **key** is the dependency-side name and whose **value**
/// is the importer-side name the dep name resolves to. Used for type and
/// dimension bindings on instantiated includes. The aliased name keeps the
/// directional convention discoverable everywhere the map shape appears in a
/// signature.
type DepToImporter<T> = HashMap<T, T>;

/// Dependency index port to a resolved importer-side declared or structural axis.
type IndexBindings = HashMap<IndexName, IndexBindingTarget>;

/// One evaluated value retained for output assembly across project-file
/// boundaries.
type EvaluatedOutputValue = (ScopedName, Result<Value, NodeError>, DeclType);

/// Presentation aliases for private selective-include scopes.
///
/// Keys are collision-free synthetic IR qualifiers; values are human-readable
/// target leaves retained only when that leaf is unambiguous in the importer.
type IncludeDebugNameMap = HashMap<ModuleAliasName, ModuleAliasName>;

/// A selective import/include alias.
///
/// `original` is the dependency-side declaration name; `local` is the name
/// introduced into the importer. Keeping the two roles named prevents call
/// sites from swapping a raw `(String, String)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::eval::project) struct ImportAlias {
    original: DeclName,
    local: DeclName,
}

/// Derive a module name (the leaf segment) from a `ModulePath`.
///
/// Used as the include-instance alias for the bare `include path(args);`
/// form and as the module-qualifier name for `import path;`.
fn derive_module_name_from_import_path(import_path: &ModulePath) -> ModuleAliasName {
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
                module: ModuleAliasName::from_atom(qualifier_name.value.member().atom().clone()),
                member: DeclName::from_atom(field.value.atom().clone()),
            }) {
            let merged_span = qualifier_name.span.merge(field.span);
            Some(ExprKind::GraphRef(Spanned {
                value: ScopedName::qualified(
                    ModuleAliasName::from_atom(qualifier_name.value.member().atom().clone()),
                    DeclName::from_atom(field.value.atom().clone()),
                ),
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
struct EvaluatedFile {
    /// Whether runtime values and assertion results were evaluated.
    runtime_available: bool,
    /// Evaluated runtime values (params + nodes): name → `RuntimeValue`.
    values: HashMap<DeclName, RuntimeValue>,
    /// Evaluated const values: name → `RuntimeValue`.
    const_values: HashMap<DeclName, RuntimeValue>,
    /// Complete evaluated values in source order, retained so an empty-argument
    /// file include can expose its instance internals in the all-values view
    /// without changing the include's pre-evaluated execution path.
    evaluated_values: Vec<EvaluatedOutputValue>,
    /// Declared types for all consts/params/nodes in this file.
    declared_types: HashMap<ScopedName, DeclaredType>,
    /// Assertion results from this file: name → (result, span).
    /// Names keep alias qualification for include-instantiated asserts (#813).
    assertions: HashMap<ScopedName, (AssertResult, Span)>,
    /// Evaluated plot specs of this file, keyed by their local (leaf) name.
    /// Consumers request them through include brace lists (#847).
    plots: HashMap<DeclName, super::types::PlotSpec>,
    /// The file's frozen registry (for type-system import by downstream files).
    registry: Registry,
    /// Explicit exports and annotation-free `param` input ports, classified
    /// separately for import/include boundary checks.
    external_surface: ExternalDeclSurface,
    /// Concrete scale factors for this file's own dynamic units, resolved
    /// against the file's evaluated runtime values. Module importers convert
    /// the dynamic units to static scales at registry-merge time — the scale
    /// expression references this file's params and cannot be re-evaluated in
    /// the importer's context. Empty when `runtime_available` is `false`.
    resolved_dynamic_unit_scales: HashMap<UnitRef, PositiveFiniteScale>,
    /// Canonical nominal dependencies of producer parameter defaults, reusable
    /// at configured include boundaries before body substitution.
    override_dependencies: graphcal_compiler::tir::dim_check::OverrideDependencySummary,
    /// Compiled TIRs for this file-root DAG and every inline DAG it contains.
    ///
    /// Entries retain canonical `DagId` keys when cloned into downstream
    /// importers. Imported aliases resolve to those identities before TIR, so
    /// file-root calls (`@alias(args).out`) and child calls
    /// (`@alias.dag(args).out`) share the same lookup.
    dag_tirs: graphcal_compiler::tir::typed::DagRegistry,
    /// Resolved extern function signatures declared by this file's
    /// `import plugin` blocks. Merged into importers' TIRs alongside
    /// `dag_tirs` so cross-file qualified inline calls into dag bodies that
    /// use extern functions can resolve their signatures at eval time.
    extern_functions: HashMap<
        graphcal_compiler::syntax::plugin::ExternFnKey,
        graphcal_compiler::ir::lower::ExternFunctionEntry,
    >,
}

impl EvaluatedFile {
    /// Check whether this file has an evaluated top-level assertion with the
    /// given (bare local) name.
    fn has_assert(&self, name: &NameAtom) -> bool {
        self.assertions
            .contains_key(&ScopedName::from(name.clone()))
    }
}

/// The result of compiling a single file within a project context.
///
/// Produced by [`compile_single_file_in_project`] and consumed by the
/// per-file evaluation and TIR compilation pipelines.
struct CompiledFile {
    tir: graphcal_compiler::tir::typed::TIR,
    declared_types: HashMap<ScopedName, DeclaredType>,
    /// Imported values for this file (cloned before being consumed by IR).
    /// Used by the root file to enrich output with imported value names.
    imported_values: HashMap<ScopedName, (RuntimeValue, DeclaredType)>,
    /// Imported value categories in source order (for root output).
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
    /// Values belonging to this file's consumer-facing output surface.
    ///
    /// The set contains the file's own declarations, imported names, selected
    /// include aliases, and projectable outputs of whole-instance includes.
    output_surface: HashSet<ScopedName>,
    /// Complete values from pre-evaluated empty-argument file includes. These
    /// are hidden from the surface view but retained for `EvalOutputView::All`.
    included_debug_values: Vec<EvaluatedOutputValue>,
    /// Human-readable aliases for unambiguous private include scopes.
    include_debug_names: IncludeDebugNameMap,
    /// Plot specs requested from standalone-evaluated dependencies via
    /// include brace lists, renamed to their local aliases (#847).
    included_plots: Vec<super::types::PlotSpec>,
}

/// A deferred include of a DAG (file-level or inline) — compile its body
/// and merge into the importer's IR after the importer's own decls are
/// lowered.
///
/// A file include (`include lib(args).{...}`) is a DAG whose source is
/// the file root; an inline DAG include (`include dag(args)` /
/// `include lib.dag(args)`) is a DAG inside some file. After the flat
/// TIR registry, both are uniformly addressed by canonical [`DagId`].
struct DeferredDagInclude {
    /// Identifies the source kind and any kind-specific data (file's AST
    /// vs inline dag's body + parent context).
    source: DeferredDagSource,
    /// Private merge namespace for this configured instance. Only the named
    /// variant is source-visible; selective includes use an opaque identity.
    instance_scope: IncludeInstanceScope,
    /// Target leaf retained for human-readable debug output when unambiguous.
    debug_scope: ModuleAliasName,
    /// Param bindings: `param_name` → binding expression.
    bindings: HashMap<DeclName, Expr>,
    /// Index bindings: dependency port → resolved importer-side axis.
    index_bindings: IndexBindings,
    /// Importer-source value span for each index binding, keyed by dep-side name.
    index_binding_spans: HashMap<IndexName, Span>,
    /// Type bindings: `dep_type_name` → `importer_type_name`.
    type_bindings: DepToImporter<StructTypeName>,
    /// Dimension bindings: `dep_dim_name` → `importer_dim_name`.
    dim_bindings: DepToImporter<DimName>,
    /// For selective includes: the selected names and their local aliases.
    /// `None` for module-form includes (all names accessible via `prefix::`).
    selective_names: Option<Vec<ImportAlias>>,
    /// Values this include intentionally exposes on the importer's output
    /// surface. Merged implementation values not listed here remain available
    /// to the all-values debug view.
    surface_outputs: Vec<ScopedName>,
    /// Plots requested by the include brace list, keyed by the dep-side
    /// plot name (#847).
    requested_plots: HashMap<DeclName, graphcal_compiler::ir::lower::RequestedPlot>,
    /// Span of the include declaration (for diagnostics).
    import_span: Span,
    /// Per-import-item attributes (e.g., `#[expected_fail(...)]` on
    /// included assertions). Key = original name in dep.
    import_item_attributes:
        HashMap<DeclName, Vec<graphcal_compiler::desugar::desugared_ast::Attribute>>,
    /// Original names of selective items marked `pub` in the importer's
    /// brace list. Module-form includes never export outputs.
    pub_reexport_items: HashSet<DeclName>,
}

/// What is being included — distinguishes file roots from inline DAGs and
/// carries the kind-specific data the deferred processor needs.
enum DeferredDagSource {
    /// File include — body is the dependency's full AST. Its own import and
    /// include declarations are processed recursively for each instance.
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

/// Canonical routing metadata for one module alias in the project pipeline.
///
/// `target` identifies the exact file-root or inline DAG module bound to the
/// alias; it is never collapsed to the containing source file.
#[derive(Debug, Clone)]
struct ProjectModuleBinding {
    target: graphcal_compiler::dag_id::DagId,
    span: Span,
    role: graphcal_compiler::syntax::module_resolve::ModuleAliasRole,
}

impl ProjectModuleBinding {
    const fn span(&self) -> Span {
        self.span
    }
}

/// Mutable state accumulated while processing import declarations.
///
/// Bundles the various collections that [`compile_single_file_in_project`] builds
/// during its import-processing loop, avoiding excessive parameter counts in the
/// extracted helper functions.
struct ImportContext<'a> {
    imported_names: ImportedValueNames,
    imported_bindings: HashMap<ScopedName, ImportedBinding>,
    imported_source_order: Vec<(ScopedName, DeclCategory)>,
    imported_type_system_names: HashMap<
        graphcal_compiler::dag_id::DagId,
        graphcal_compiler::ir::lower::SelectedDeclarations,
    >,
    module_map: HashMap<ModuleAliasName, ProjectModuleBinding>,
    /// Registry surfaces of module-imported dependencies, merged into the
    /// importer's registry builder before its own declarations register.
    extra_registry_builders: Vec<ModuleRegistryImport<'a>>,
    deferred_dag_includes: Vec<DeferredDagInclude>,
    /// Complete values from empty-argument file includes, qualified by their
    /// logical instance prefix for the all-values output view.
    included_debug_values: Vec<EvaluatedOutputValue>,
    /// Plot specs requested from standalone-evaluated dependencies via
    /// include brace lists, renamed to their local aliases (#847).
    included_plot_specs: Vec<super::types::PlotSpec>,
}

/// A module-imported dependency's registry surface, queued for merging into
/// the importer's registry builder.
struct ModuleRegistryImport<'a> {
    /// The dependency's frozen registry.
    registry: &'a Registry,
    /// The dependency's external declaration surface. Registry items cross
    /// only when explicitly exported; input ports remain a distinct role.
    external_surface: &'a ExternalDeclSurface,
    /// The import alias that keys the dependency's `pub` units in the
    /// importer's unit scope (`alias.unit`).
    unit_alias: ModuleAliasName,
    /// Concrete scales for the dependency's dynamic units, resolved against
    /// its evaluated runtime values. Dynamic units merge as static scales —
    /// their scale expressions reference the dependency's own params and
    /// cannot be re-evaluated in the importer's context.
    resolved_dynamic_scales: &'a HashMap<UnitRef, PositiveFiniteScale>,
    /// The import-statement span, localizing merge-conflict diagnostics.
    import_span: Span,
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

/// Resolve namespace-alias graph references in one compilation body's AST and
/// deferred include bindings: rewrite
/// `FieldAccess(GraphRef(Local(alias)), field)` to a qualified
/// `GraphRef(Qualified { module: alias, member: field })` when
/// `(alias, field)` matches an imported namespace member. The qualification is
/// preserved structurally throughout the IR / eval pipeline — there is no
/// flat-string boundary.
///
/// Deferred bindings are extracted from the canonical loaded AST before the
/// body AST is cloned and rewritten. Rewriting both representations together
/// ensures a nested instantiation can prefix sibling-instance references using
/// their full qualified identity.
///
/// If there are no qualified members, returns a borrowed reference to the
/// original AST.
fn rewrite_qualified_refs_in_compilation_body<'a>(
    ast: &'a graphcal_compiler::desugar::desugared_ast::File,
    imported_names: &ImportedValueNames,
    deferred_dag_includes: &mut [DeferredDagInclude],
) -> std::borrow::Cow<'a, graphcal_compiler::desugar::desugared_ast::File> {
    let alias_pairs = collect_qualified_pairs(imported_names);
    if alias_pairs.is_empty() {
        return std::borrow::Cow::Borrowed(ast);
    }

    for binding in deferred_dag_includes
        .iter_mut()
        .flat_map(|include| include.bindings.values_mut())
    {
        rewrite_alias_field_access(binding, &alias_pairs);
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
                module: module.clone(),
                member: scoped.member().clone(),
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
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<DeclaredType> {
    // Check if the field type is a bare generic param reference (e.g., `D`)
    if let graphcal_compiler::desugar::desugared_ast::TypeExprKind::DimExpr(dim_expr) =
        &field.type_ann().kind
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
        return Some(concrete.clone());
    }
    if let graphcal_compiler::desugar::desugared_ast::TypeExprKind::ComplexApplication {
        generic_args,
    } = &field.type_ann().kind
        && let [dimension_arg] = generic_args.as_slice()
    {
        let dimension = resolve_field_dimension_arg(dimension_arg, generic_sub, registry);
        if let Some(dimension) = dimension {
            return Some(DeclaredType::Complex(dimension));
        }
    }
    // Non-generic: resolve directly from the registry. Overflow in dimension
    // arithmetic is treated as "no declared type info" here — the value will
    // render as a raw quantity, and dim_check would have already flagged the
    // overflow as a real error during compilation.
    registry
        .dimensions
        .resolve_type_expr(field.type_ann())
        .ok()
        .flatten()
        .map(DeclaredType::Quantity)
}

fn resolve_field_dimension_arg(
    arg: &graphcal_compiler::syntax::ast::GenericArg<graphcal_compiler::syntax::phase::Desugared>,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<graphcal_compiler::dimension::Dimension> {
    use graphcal_compiler::syntax::ast::GenericArg;
    match arg {
        GenericArg::Type(type_expr) => resolve_field_dimension(type_expr, generic_sub, registry),
        GenericArg::Ambiguous(ambiguous) => {
            resolve_ambiguous_field_dimension(ambiguous, generic_sub, registry)
        }
        GenericArg::Index(_) | GenericArg::Nat(_) => None,
    }
}

fn resolve_ambiguous_field_dimension(
    arg: &graphcal_compiler::syntax::ast::AmbiguousGenericArg,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<graphcal_compiler::dimension::Dimension> {
    use graphcal_compiler::syntax::ast::AmbiguousGenericArg;
    match arg {
        AmbiguousGenericArg::Name(name) => {
            field_dimension_name(name.name.as_str(), generic_sub, registry)
        }
        AmbiguousGenericArg::Mul(operands, _) => {
            let mut operands = operands.iter();
            let first = resolve_ambiguous_field_dimension(operands.next()?, generic_sub, registry)?;
            operands.try_fold(first, |product, operand| {
                product
                    .checked_mul(&resolve_ambiguous_field_dimension(
                        operand,
                        generic_sub,
                        registry,
                    )?)
                    .ok()
            })
        }
    }
}

fn resolve_field_dimension(
    type_expr: &graphcal_compiler::desugar::desugared_ast::TypeExpr,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<graphcal_compiler::dimension::Dimension> {
    use graphcal_compiler::desugar::desugared_ast::{MulDivOp, TypeExprKind};
    match &type_expr.kind {
        TypeExprKind::Dimensionless => {
            Some(graphcal_compiler::dimension::Dimension::dimensionless())
        }
        TypeExprKind::DimExpr(dim_expr) => dim_expr.terms.iter().try_fold(
            graphcal_compiler::dimension::Dimension::dimensionless(),
            |acc, item| {
                let name = item.term.name.value.leaf().as_str();
                let dimension = field_dimension_name(name, generic_sub, registry)?;
                let powered = dimension
                    .pow(
                        item.term
                            .power
                            .unwrap_or(graphcal_compiler::dimension::Rational::ONE),
                    )
                    .ok()?;
                match item.op {
                    MulDivOp::Mul => acc.checked_mul(&powered).ok(),
                    MulDivOp::Div => acc.checked_div(&powered).ok(),
                }
            },
        ),
        TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DatetimeApplication { .. }
        | TypeExprKind::ComplexApplication { .. }
        | TypeExprKind::KeyApplication { .. }
        | TypeExprKind::Indexed { .. }
        | TypeExprKind::TypeApplication { .. } => None,
    }
}

fn field_dimension_name(
    name: &str,
    generic_sub: &HashMap<&str, DeclaredType>,
    registry: &Registry,
) -> Option<graphcal_compiler::dimension::Dimension> {
    generic_sub
        .get(name)
        .and_then(|declared| match declared {
            DeclaredType::Quantity(dimension) => Some(dimension.clone()),
            _ => None,
        })
        .or_else(|| registry.dimensions.get_dimension(name).cloned())
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
    compile_to_tir_from_project_with_cancellation(
        project,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile a loaded project to TIR with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`CompileError`] for invalid source or cancellation.
pub fn compile_to_tir_from_project_with_cancellation(
    project: &crate::loader::LoadedProject,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    // Phase A default: dependency-file extern calls evaluate through the
    // built-in demo registry (see `crate::host_fns`).
    compile_to_tir_from_project_with_host_fns_and_cancellation(
        project,
        &crate::host_fns::demo_registry(),
        cancellation,
    )
}

/// Like [`compile_to_tir_from_project`], with an embedder-supplied host
/// function registry backing extern (plugin) calls in dependency files.
///
/// Compile-only consumers still evaluate standalone dependency files so
/// importers can bind their values; a dependency node calling an extern
/// function needs the same registry evaluation uses, or its value (and
/// everything importing it) degrades to a per-node failure.
///
/// # Errors
///
/// Returns a [`CompileError`] if lowering, resolution, or checking fails.
pub fn compile_to_tir_from_project_with_host_fns(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    compile_to_tir_from_project_with_host_fns_and_cancellation(
        project,
        host_fns,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile with embedder host functions and cooperative cancellation.
///
/// # Errors
///
/// Returns a [`CompileError`] for invalid source or cancellation.
pub fn compile_to_tir_from_project_with_host_fns_and_cancellation(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    pipeline::compile_to_tir_project_perfile(project, host_fns, cancellation)
}

/// Prepare a loaded project once for repeated typed evaluation.
///
/// # Errors
///
/// Returns a [`CompileError`] for loading-independent compilation, type, plan,
/// plugin, or cancellation failures.
pub fn prepare_from_project(
    project: &crate::loader::LoadedProject,
) -> Result<PreparedProject, CompileError> {
    prepare_from_project_with_host_fns(project, &crate::host_fns::demo_registry())
}

/// Prepare with an embedder-supplied host-function registry.
pub fn prepare_from_project_with_host_fns(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<PreparedProject, CompileError> {
    prepare_from_project_with_host_fns_and_cancellation(
        project,
        host_fns,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Prepare with host functions and cooperative cancellation.
pub fn prepare_from_project_with_host_fns_and_cancellation(
    project: &crate::loader::LoadedProject,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<PreparedProject, CompileError> {
    pipeline::prepare_project_perfile(project, host_fns, cancellation)
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
    compile_and_eval_from_project_with_cancellation(
        project,
        overrides,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile and evaluate a project with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`CompileError`] for invalid source, evaluation failure, or
/// cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_from_project_with_cancellation(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalResult, CompileError> {
    // Phase A default: no real plugins exist yet, so the built-in demo
    // registry is the standard embedder injection (see `crate::host_fns`).
    compile_and_eval_from_project_with_host_fns_and_cancellation(
        project,
        overrides,
        &crate::host_fns::demo_registry(),
        cancellation,
    )
}

/// Like [`compile_and_eval_from_project`], with an embedder-supplied host
/// function registry backing extern (plugin) calls.
///
/// Every extern function declared by an `import plugin` block must have a
/// registry entry; a missing entry is a load-time
/// [`MissingHostFunction`](graphcal_compiler::registry::error::GraphcalError::MissingHostFunction)
/// diagnostic.
///
/// # Errors
///
/// Returns a [`CompileError`] if any pipeline stage fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_from_project_with_host_fns(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
) -> Result<EvalResult, CompileError> {
    compile_and_eval_from_project_with_host_fns_and_cancellation(
        project,
        overrides,
        host_fns,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile and evaluate with embedder host functions and cooperative
/// cancellation.
///
/// # Errors
///
/// Returns a [`CompileError`] for invalid source, evaluation failure, or
/// cancellation.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_from_project_with_host_fns_and_cancellation(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_compiler::desugar::desugared_ast::Expr>,
    host_fns: &crate::host_fns::HostFunctionRegistry,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<EvalResult, CompileError> {
    let prepared = pipeline::prepare_project_perfile(project, host_fns, cancellation)?;
    let mut bindings = prepared.binding_builder();
    for (name, expression) in overrides {
        cancellation.checkpoint()?;
        bindings.bind_expression(name, expression)?;
    }
    let row = bindings.finish()?;
    prepared.evaluate_with_cancellation(&row, cancellation)
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

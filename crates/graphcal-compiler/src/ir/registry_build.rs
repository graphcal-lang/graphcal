//! Registry construction from desugared declarations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;

use crate::desugar::desugared_ast::{DeclKind, Expr, ExprKind, File, IndexDeclKind, TypeExpr};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::Dimension;
use crate::registry::dimension_registry::DimensionResolveError;
use crate::registry::error::GraphcalError;
use crate::registry::format::format_unit_expr_with_config;
use crate::registry::types::{
    self, PositiveFiniteScale, PositiveFiniteScaleError, RegistryBuilder, UnitScale,
};
use crate::syntax::decl_name::DeclName;
use crate::syntax::dimension::{DimName, UnitRef};
use crate::syntax::index_name::IndexName;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::{NameAtom, NamePath};
use crate::syntax::span::{Span, Spanned};
use crate::syntax::type_name::{ConstructorName, GenericParamName};
use crate::syntax::visitor::ExprVisitor;

use super::lower::{BodySource, UnfrozenDynamicUnitScaleEntry};

/// Register dimensions, units, indexes, and struct types from a file's declarations
/// into the registry.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub(super) fn register_file_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
) -> Result<Vec<UnfrozenDynamicUnitScaleEntry>, GraphcalError> {
    let mut dynamic_unit_scales = Vec::new();
    register_declarations_impl(file, registry, src, None, dag_id, &mut dynamic_unit_scales)?;
    Ok(dynamic_unit_scales)
}

/// Categorized declarations selected from a dependency's registry.
///
/// Each source import marker maps to exactly one typed set. Keeping the
/// categories separate prevents a declaration from drifting between semantic
/// namespaces without changing the importing source.
#[derive(Debug, Default, Clone)]
pub struct SelectedDeclarations {
    /// Bare items are unresolved between declaration and constructor namespaces.
    terms: HashSet<crate::syntax::names::NameAtom>,
    dimensions: HashSet<crate::syntax::dimension::DimName>,
    units: HashSet<crate::syntax::dimension::UnitName>,
    indexes: HashSet<crate::syntax::index_name::IndexName>,
    types: HashSet<crate::syntax::type_name::StructTypeName>,
}

impl SelectedDeclarations {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.dimensions.is_empty()
            && self.units.is_empty()
            && self.indexes.is_empty()
            && self.types.is_empty()
    }

    /// Record one selective import in its declared namespace.
    pub fn insert(
        &mut self,
        namespace: crate::syntax::ast::ImportItemNamespace,
        name: crate::syntax::names::NameAtom,
    ) {
        match namespace {
            crate::syntax::ast::ImportItemNamespace::Term => {
                self.terms.insert(name);
            }
            crate::syntax::ast::ImportItemNamespace::Type => {
                self.types
                    .insert(crate::syntax::type_name::StructTypeName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Dimension => {
                self.dimensions
                    .insert(crate::syntax::dimension::DimName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Unit => {
                self.units
                    .insert(crate::syntax::dimension::UnitName::from_atom(name));
            }
            crate::syntax::ast::ImportItemNamespace::Index => {
                self.indexes
                    .insert(crate::syntax::index_name::IndexName::from_atom(name));
            }
        }
    }

    /// Dimension names selected with the explicit `dim` marker.
    pub fn dimensions(&self) -> impl Iterator<Item = &crate::syntax::dimension::DimName> {
        self.dimensions.iter()
    }

    /// Clone this selection without declarations that are closed semantic values.
    ///
    /// Project lowering imports selected dimensions and units from the
    /// dependency's already-resolved semantic registry, while the remaining
    /// declaration categories still use source registration.
    #[must_use]
    pub fn without_resolved_dimensions_and_units(&self) -> Self {
        let mut selected = self.clone();
        selected.dimensions.clear();
        selected.units.clear();
        selected
    }

    /// Unit names selected with the explicit `unit` marker.
    pub fn units(&self) -> impl Iterator<Item = &crate::syntax::dimension::UnitName> {
        self.units.iter()
    }
}

/// Register only categorized declarations selected from a file.
///
/// This is the selective counterpart to `register_file_declarations`: each
/// declaration kind is filtered through the import category written by the
/// consumer.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_selected_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    names: &SelectedDeclarations,
    dag_id: &crate::dag_id::DagId,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, Some(names), dag_id, &mut Vec::new())
}

/// Shared implementation for registering type-system declarations.
///
/// Registration is split into phases to allow forward references between
/// declarations of the same kind (e.g., a derived dimension referencing another
/// derived dimension declared later in the file). The phases are:
///
/// 1. Base dimensions, types, union types, named/required-named indexes
/// 2. Derived dimensions (topologically sorted by inter-dependency)
/// 3. Required-coordinate indexes (depend only on dimensions)
/// 4. Units (topologically sorted by inter-dependency)
/// 5. Coordinate indexes (depend on dimensions and units)
///
/// When `filter` is `None`, all declarations are registered.
/// When `filter` is `Some(names)`, every declaration kind is filtered through
/// its own typed import category.
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&SelectedDeclarations>,
    dag_id: &crate::dag_id::DagId,
    dynamic_unit_scales: &mut Vec<UnfrozenDynamicUnitScaleEntry>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{DimDecl, IndexDecl, UnitDecl};

    let should_register_term = |name: &str| filter.is_none_or(|names| names.terms.contains(name));
    let should_register_dimension =
        |name: &str| filter.is_none_or(|names| names.dimensions.contains(name));
    let should_register_unit = |name: &str| filter.is_none_or(|names| names.units.contains(name));
    let should_register_index =
        |name: &str| filter.is_none_or(|names| names.indexes.contains(name));
    let should_register_type = |name: &str| filter.is_none_or(|names| names.types.contains(name));

    // Collect declarations by kind for phased registration.
    let mut derived_dims: Vec<&DimDecl> = Vec::new();
    let mut units: Vec<&UnitDecl> = Vec::new();
    let mut required_coordinate_indexes: Vec<(&IndexDecl, Span)> = Vec::new();
    let mut coordinate_indexes: Vec<(&IndexDecl, Span)> = Vec::new();

    // Phase 1: Register base dimensions, types, union types, named/required-named indexes.
    // Also collect derived dims, units, and dependent indexes for later phases.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(d) if should_register_dimension(d.name.value.as_str()) => {
                register_base_dimension_decl(d, registry, dag_id);
            }
            DeclKind::Dimension(d) if should_register_dimension(d.name.value.as_str()) => {
                if d.definition.is_some() {
                    derived_dims.push(d);
                } else {
                    // Required dim (`dim D;`) — no body. Compile as an opaque
                    // base dimension so the library checks out in isolation;
                    // substitution via include-time dim bindings happens in a
                    // later phase (see visibility/bindability axioms plan §C2).
                    register_required_dimension_decl(d, registry, dag_id);
                }
            }
            DeclKind::Unit(u) if should_register_unit(u.name.value.as_str()) => {
                units.push(u);
            }
            DeclKind::Index(idx) if should_register_index(idx.name.value.as_str()) => {
                match &idx.kind {
                    IndexDeclKind::RequiredCoordinate { .. } => {
                        required_coordinate_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Range { .. } | IndexDeclKind::Linspace { .. } => {
                        coordinate_indexes.push((idx, decl.span));
                    }
                    IndexDeclKind::Named { .. } | IndexDeclKind::RequiredNamed => {
                        register_index_decl(idx, registry, src, decl.span)?;
                    }
                }
            }
            DeclKind::Type(t) if should_register_type(t.name.value.as_str()) => {
                register_type_decl(t, registry, src)?;
            }
            DeclKind::Dag(d) if should_register_term(d.name.value.as_str()) => {
                registry.register_dag(d.name.value.clone(), d.clone());
            }
            _ => {}
        }
    }

    // Phase 2: Topologically sort and register derived dimensions.
    if !derived_dims.is_empty() {
        let sorted = topo_sort_derived_dims(&derived_dims, src)?;
        for d in sorted {
            register_dimension_decl(d, registry, src)?;
        }
    }

    // Phase 3: Register required-coordinate indexes (depend only on dimensions).
    for (idx, span) in &required_coordinate_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    // Phase 4: Topologically sort and register units.
    if !units.is_empty() {
        let sorted = topo_sort_units(&units, src)?;
        for u in sorted {
            if let Some(dynamic_scale) = register_unit_decl(u, registry, src, dag_id)? {
                dynamic_unit_scales.push(dynamic_scale);
            }
        }
    }

    // Phase 5: Register coordinate indexes (depend on dimensions and units).
    for (idx, span) in &coordinate_indexes {
        register_index_decl(idx, registry, src, *span)?;
    }

    register_structural_finite_indexes(file, registry, src)?;

    Ok(())
}

/// Register concrete Fin definitions under typed structural identities appearing
/// in type positions or finite comprehensions.
fn register_structural_finite_indexes(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Param(d) => {
                collect_finite_indexes_from_type_expr(&d.type_ann, registry, src)?;
                if let Some(ref value) = d.value {
                    collect_finite_indexes_from_expr(value, registry, src)?;
                }
            }
            DeclKind::Node(d) | DeclKind::ConstNode(d) => {
                collect_finite_indexes_from_type_expr(&d.type_ann, registry, src)?;
                collect_finite_indexes_from_expr(&d.value, registry, src)?;
            }
            DeclKind::Type(d) => {
                for default in d
                    .generic_params
                    .iter()
                    .filter_map(|param| param.default.as_ref())
                {
                    collect_finite_indexes_from_generic_arg(default, registry, src)?;
                }
                if let crate::desugar::desugared_ast::TypeDeclBody::Constructors(members) = &d.body
                {
                    for field in members
                        .iter()
                        .filter_map(|member| member.payload.as_ref())
                        .flatten()
                    {
                        collect_finite_indexes_from_type_expr(&field.type_ann, registry, src)?;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Topologically sort derived dimension declarations by their inter-dependencies.
///
/// Dependencies on dimensions already in the registry (e.g., from preludes or imports)
/// are considered satisfied and do not create graph edges. Only dependencies between
/// the file-local derived dimensions are edges.
fn topo_sort_derived_dims<'a>(
    dims: &[&'a crate::desugar::desugared_ast::DimDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::desugared_ast::DimDecl>, GraphcalError> {
    let mut graph = DiGraph::<&str, ()>::new();
    let mut name_to_idx: HashMap<&str, petgraph::graph::NodeIndex> = HashMap::new();
    let mut idx_to_pos: HashMap<petgraph::graph::NodeIndex, usize> = HashMap::new();

    // Add a node for each derived dimension.
    for (pos, d) in dims.iter().enumerate() {
        let name = d.name.value.as_str();
        let idx = graph.add_node(name);
        name_to_idx.insert(name, idx);
        idx_to_pos.insert(idx, pos);
    }

    // Add edges: if dim A references dim B (and B is a *different* file-local dim), add A → B.
    // Self-references (e.g., `dimension Mass = Mass;` aliasing a prelude dimension) are
    // excluded — they resolve against the existing registry during registration.
    for d in dims {
        let self_name = d.name.value.as_str();
        let from = name_to_idx[self_name];
        // Only derived dims reach this sort; required dims are routed
        // directly to the base-dim registry in Phase 1.
        let Some(definition) = &d.definition else {
            continue;
        };
        for item in &definition.terms {
            let Some(dep_name) = item
                .term
                .name
                .value
                .as_bare()
                .map(super::super::syntax::names::NameAtom::as_str)
            else {
                continue;
            };
            if dep_name != self_name
                && let Some(&to) = name_to_idx.get(dep_name)
            {
                graph.add_edge(from, to, ());
            }
        }
    }

    // Topologically sort (reversed, since edges point from dependent → dependency).
    let sorted_indices = toposort(&graph, None).map_err(|cycle| {
        let cycle_name = graph[cycle.node_id()];
        let pos = idx_to_pos[&cycle.node_id()];
        GraphcalError::CyclicDimension {
            name: DimName::expect_valid(cycle_name),
            src: src.clone(),
            span: dims[pos].name.span.into(),
        }
    })?;

    // toposort returns dependencies-last order; reverse for dependencies-first.
    Ok(sorted_indices
        .into_iter()
        .rev()
        .map(|idx| dims[idx_to_pos[&idx]])
        .collect())
}

/// Topologically sort unit declarations by their inter-dependencies.
///
/// A unit depends on other units through its `definition.unit_expr` (e.g., `const unit km: Length = 1000 m;`
/// depends on `m`). Dependencies on units already in the registry are satisfied and
/// do not create graph edges.
fn topo_sort_units<'a>(
    units: &[&'a crate::desugar::desugared_ast::UnitDecl],
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<&'a crate::desugar::desugared_ast::UnitDecl>, GraphcalError> {
    let mut graph = DiGraph::<&str, ()>::new();
    let mut name_to_idx: HashMap<&str, petgraph::graph::NodeIndex> = HashMap::new();
    let mut idx_to_pos: HashMap<petgraph::graph::NodeIndex, usize> = HashMap::new();

    // Add a node for each unit.
    for (pos, u) in units.iter().enumerate() {
        let name = u.name.value.as_str();
        let idx = graph.add_node(name);
        name_to_idx.insert(name, idx);
        idx_to_pos.insert(idx, pos);
    }

    // Add edges: if unit A's definition references unit B (a *different* file-local unit), add A → B.
    for u in units {
        let self_name = u.name.value.as_str();
        let from = name_to_idx[self_name];
        if let Some(def) = &u.definition {
            for item in &def.unit_expr.terms {
                // Module-qualified references can never name a file-local
                // unit, so only bare references create graph edges.
                if item.name.value.is_qualified() {
                    continue;
                }
                let dep_name = item.name.value.name().as_str();
                if dep_name != self_name
                    && let Some(&to) = name_to_idx.get(dep_name)
                {
                    graph.add_edge(from, to, ());
                }
            }
        }
    }

    let sorted_indices = toposort(&graph, None).map_err(|cycle| {
        let pos = idx_to_pos[&cycle.node_id()];
        GraphcalError::CyclicUnit {
            name: units[pos].name.value.clone(),
            src: src.clone(),
            span: units[pos].name.span.into(),
        }
    })?;

    // toposort returns dependencies-last order; reverse for dependencies-first.
    Ok(sorted_indices
        .into_iter()
        .rev()
        .map(|idx| units[idx_to_pos[&idx]])
        .collect())
}

fn register_base_dimension_decl(
    d: &crate::desugar::desugared_ast::BaseDimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::dag_id::DagId,
) {
    let dim_id = crate::dimension::BaseDimId::UserDefined(
        crate::syntax::dimension::ResolvedDimName::from_def(dag_id.clone(), d.name.value.clone()),
    );
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

fn register_dimension_decl(
    d: &crate::desugar::desugared_ast::DimDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Only derived dims reach this function; required dims (`dim D;`)
    // are routed to `register_required_dimension_decl` in Phase 1 and
    // never end up in the topo-sorted derived-dim list.
    let Some(definition) = d.definition.as_ref() else {
        return Ok(());
    };
    let dim = registry
        .resolve_dim_expr_detailed(definition)
        .map_err(|err| dimension_resolve_error(err, src, definition.span))?;
    registry.register_dimension(d.name.value.clone(), dim);
    Ok(())
}

/// Register a required dim (`dim D;`) as an opaque base dimension.
///
/// The library treats the required dim like a base SI dimension while
/// compiling standalone. Later include-time substitution rewires
/// references through the importer's dim bindings.
fn register_required_dimension_decl(
    d: &crate::desugar::desugared_ast::DimDecl,
    registry: &mut RegistryBuilder,
    dag_id: &crate::dag_id::DagId,
) {
    let dim_id = crate::dimension::BaseDimId::UserDefined(
        crate::syntax::dimension::ResolvedDimName::from_def(dag_id.clone(), d.name.value.clone()),
    );
    registry.register_base_dimension(d.name.value.clone(), dim_id);
}

pub(super) fn registry_build_error(
    err: &types::RegistryBuildError,
    src: &NamedSource<Arc<String>>,
) -> GraphcalError {
    GraphcalError::internal_error(
        format!("registry build failed: {err}"),
        src,
        DiagnosticAnchor::WholeFile,
    )
}

fn dimension_resolve_error(
    err: DimensionResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    match err {
        DimensionResolveError::UnknownDimension { name } => GraphcalError::UnknownDimension {
            name: NamePath::from(name.into_atom()),
            src: src.clone(),
            span: span.into(),
        },
        DimensionResolveError::Overflow(_) => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: span.into(),
        },
    }
}

fn eval_error(
    message: impl Into<String>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: message.into(),
        src: src.clone(),
        span: span.into(),
    }
}

fn validate_positive_finite_scale(
    value: f64,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<PositiveFiniteScale, GraphcalError> {
    PositiveFiniteScale::new(value).map_err(|err| {
        let reason = match err {
            PositiveFiniteScaleError::NonFinite => "must be finite",
            PositiveFiniteScaleError::NonPositive => "must be greater than zero",
        };
        eval_error(format!("{context} {reason}, got {value}"), src, span)
    })
}

fn multiply_positive_scales(
    lhs: PositiveFiniteScale,
    rhs: PositiveFiniteScale,
    context: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<PositiveFiniteScale, GraphcalError> {
    validate_positive_finite_scale(lhs.get() * rhs.get(), context, src, span)
}

fn register_unit_decl(
    u: &crate::desugar::desugared_ast::UnitDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    dag_id: &crate::dag_id::DagId,
) -> Result<Option<UnfrozenDynamicUnitScaleEntry>, GraphcalError> {
    let dim = registry
        .resolve_dim_expr_detailed(&u.dim_type)
        .map_err(|err| dimension_resolve_error(err, src, u.dim_type.span))?;
    let Some(def) = &u.definition else {
        registry
            .register_base_unit(u.name.value.clone(), dim.clone())
            .map_err(|error| {
                let dim = registry.format_dimension(&dim);
                let (reason, help) = match error {
                    types::BaseUnitRegistrationError::NotBaseDimension => (
                        format!("`{dim}` is not a base dimension"),
                        format!(
                            "`base unit` can only define the canonical unit of a bare base dimension; define `{}` with an explicit `const unit` or `unit` scale instead",
                            u.name.value
                        ),
                    ),
                    types::BaseUnitRegistrationError::AlreadyRegistered { existing } => (
                        format!("`{dim}` already has canonical base unit `{existing}`"),
                        "a dimension can have only one canonical base unit; if another multiplicative unit is valid for this dimension, define its scale explicitly with `const unit` or `unit`"
                            .to_string(),
                    ),
                };
                GraphcalError::InvalidBaseUnitDeclaration {
                    name: u.name.value.clone(),
                    dim,
                    reason,
                    help,
                    src: src.clone(),
                    span: u.name.span.into(),
                }
            })?;
        return Ok(None);
    };

    if registry.is_affine_prone(&dim) {
        return Err(GraphcalError::AffineProneUnitDefinition {
            dim: registry.format_dimension(&dim),
            src: src.clone(),
            span: u.name.span.into(),
        });
    }
    if u.constness.is_const() {
        if let Some(graph_ref) = first_graph_ref(&def.scale_expr) {
            return Err(GraphcalError::GraphRefInConstUnit {
                name: graph_ref.value,
                src: src.clone(),
                span: graph_ref.span.into(),
            });
        }
        if let Some(unit_name) = first_non_const_unit_ref(registry, &def.unit_expr) {
            return Err(GraphcalError::NonConstUnitInConst {
                name: unit_name.value.clone(),
                src: src.clone(),
                span: unit_name.span.into(),
            });
        }
    }
    let resolved_definition = resolve_unit_definition(registry, &def.unit_expr, src)?;
    if resolved_definition.dimension != dim {
        return Err(GraphcalError::UnitDefinitionDimensionMismatch {
            name: u.name.value.clone(),
            declared: registry.format_dimension(&dim),
            definition: registry.format_dimension(&resolved_definition.dimension),
            src: src.clone(),
            span: def.unit_expr.span.into(),
        });
    }
    let mut dynamic_unit_scale = None;
    let scale = if contains_graph_ref(&def.scale_expr) {
        // Preserve the validated definition in IR so strict HIR lowering,
        // policy checking, dependency collection, and type checking all
        // consume the same source-qualified semantic entry.
        dynamic_unit_scale = Some(UnfrozenDynamicUnitScaleEntry {
            spelling: UnitRef::local(u.name.value.clone()),
            expr: def.scale_expr.clone(),
            unit_owner: dag_id.clone(),
            body_resolution_owner: dag_id.clone(),
            declared_dimension: dim.clone(),
            base_unit_dimension: resolved_definition.dimension.clone(),
            span: def.scale_expr.span,
            src: BodySource::own(),
        });
        UnitScale::Dynamic {
            base_unit_scale: resolved_definition.base_scale,
        }
    } else {
        // Static scale value. A plain `unit` with no `@` still remains a
        // runtime unit for const-context policy; `const unit` is the
        // surface marker that makes it available to `const node`.
        let scale_expr = validate_positive_finite_scale(
            eval_scale_expr(&def.scale_expr, src)?,
            "unit scale expression",
            src,
            def.scale_expr.span,
        )?;
        let scale = multiply_positive_scales(
            scale_expr,
            resolved_definition.base_scale,
            "unit scale",
            src,
            def.span,
        )?;
        UnitScale::Static(scale)
    };
    registry.register_unit_with_scale(u.name.value.clone(), dim, scale, u.constness);
    Ok(dynamic_unit_scale)
}

fn first_graph_ref(expr: &Expr) -> Option<Spanned<ScopedName>> {
    struct FirstGraphRef(Option<Spanned<ScopedName>>);

    impl ExprVisitor<crate::syntax::phase::Desugared> for FirstGraphRef {
        type Error = std::convert::Infallible;

        fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
            if self.0.is_none()
                && let ExprKind::GraphRef(name) = &expr.kind
            {
                self.0 = Some(name.clone());
            }
            Ok(())
        }
    }

    let mut visitor = FirstGraphRef(None);
    let _ = visitor.visit_expr(expr);
    visitor.0
}

fn first_non_const_unit_ref<'a>(
    registry: &RegistryBuilder,
    unit_expr: &'a crate::desugar::desugared_ast::UnitExpr,
) -> Option<&'a Spanned<crate::syntax::dimension::UnitRef>> {
    unit_expr.terms.iter().find_map(|term| {
        registry
            .get_unit(&term.name.value)
            .is_some_and(|info| !info.constness.is_const())
            .then_some(&term.name)
    })
}

/// Dimension and static scale proved for a unit definition's RHS unit expression.
///
/// Keeping these facts together prevents registration from pairing a scale from
/// one physical dimension with an unrelated declared dimension.
struct ResolvedUnitDefinition {
    dimension: Dimension,
    base_scale: PositiveFiniteScale,
}

/// Resolve the unit-expression half of a unit definition before mutating the registry.
///
/// For `unit EUR: Money = (@rate) USD;`, this proves both that `USD` measures
/// `Money` and that its static base scale is 1.0. The base unit itself must be
/// static; only the scalar expression may depend on runtime values.
fn resolve_unit_definition(
    registry: &RegistryBuilder,
    unit_expr: &crate::desugar::desugared_ast::UnitExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedUnitDefinition, GraphcalError> {
    let (dimension, base_scale) = registry
        .resolve_unit_expr(unit_expr)
        .map_err(|err| unit_resolve_to_graphcal(err, src, unit_expr.span))?;
    let base_scale =
        validate_positive_finite_scale(base_scale, "base unit scale", src, unit_expr.span)?;
    Ok(ResolvedUnitDefinition {
        dimension,
        base_scale,
    })
}

/// Convert a typed unit-resolution failure into a spanned diagnostic.
fn unit_resolve_to_graphcal(
    err: crate::registry::types::UnitResolveError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    use crate::registry::types::UnitResolveError;
    match err {
        UnitResolveError::UnknownUnit(name) => GraphcalError::UnknownUnit {
            name,
            src: src.clone(),
            span: span.into(),
        },
        UnitResolveError::DynamicScale(name) => GraphcalError::EvalError {
            message: format!("unit `{name}` has a dynamic scale and cannot be used here"),
            src: src.clone(),
            span: span.into(),
        },
        UnitResolveError::InvalidScale { value, reason } => {
            let reason = match reason {
                PositiveFiniteScaleError::NonFinite => "must be finite",
                PositiveFiniteScaleError::NonPositive => "must be greater than zero",
            };
            GraphcalError::EvalError {
                message: format!("compound unit scale {reason}, got {value}"),
                src: src.clone(),
                span: span.into(),
            }
        }
        UnitResolveError::Overflow(_) => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: span.into(),
        },
    }
}

/// Check if an expression contains any `@`-references (graph refs).
fn contains_graph_ref(expr: &Expr) -> bool {
    crate::ir::resolve::contains_graph_ref(expr)
}

fn concrete_nat_value(
    expr: &crate::desugar::desugared_ast::NatExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<u64>, GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(value, _) => Ok(Some(*value)),
        NatExpr::Var(_) => Ok(None),
        NatExpr::Add(operands, span) => {
            operands.iter().try_fold(Some(0_u64), |sum, operand| {
                match (sum, concrete_nat_value(operand, src)?) {
                    (Some(sum), Some(value)) => sum
                        .checked_add(value)
                        .map(Some)
                        .ok_or_else(|| eval_error("Fin cardinality addition overflow", src, *span)),
                    _ => Ok(None),
                }
            })
        }
        NatExpr::Mul(operands, span) => {
            operands.iter().try_fold(Some(1_u64), |product, operand| {
                match (product, concrete_nat_value(operand, src)?) {
                    (Some(product), Some(value)) => {
                        product.checked_mul(value).map(Some).ok_or_else(|| {
                            eval_error("Fin cardinality multiplication overflow", src, *span)
                        })
                    }
                    _ => Ok(None),
                }
            })
        }
    }
}

fn ensure_concrete_finite_index(
    cardinality: u64,
    span: Span,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let index = types::FiniteIndex::try_from_u64(cardinality)
        .map_err(|error| eval_error(error.to_string(), src, span))?;
    registry.ensure_finite_index(index.cardinality());
    Ok(())
}

fn collect_finite_index_expr(
    index: &crate::desugar::desugared_ast::IndexExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match index {
        crate::desugar::desugared_ast::IndexExpr::Finite { cardinality, span } => {
            if let Some(value) = concrete_nat_value(cardinality, src)? {
                ensure_concrete_finite_index(value, *span, registry, src)?;
            }
        }
        crate::desugar::desugared_ast::IndexExpr::Name(_)
        | crate::desugar::desugared_ast::IndexExpr::BareNat(_) => {}
    }
    Ok(())
}

fn collect_finite_indexes_from_generic_arg(
    arg: &crate::desugar::desugared_ast::GenericArg,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match arg {
        crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
            collect_finite_indexes_from_type_expr(type_expr, registry, src)
        }
        crate::desugar::desugared_ast::GenericArg::Index(index) => {
            collect_finite_index_expr(index, registry, src)
        }
        crate::desugar::desugared_ast::GenericArg::Nat(_)
        | crate::desugar::desugared_ast::GenericArg::Ambiguous(_) => Ok(()),
    }
}

/// Register every concrete `Fin(N)` identity used by a type expression.
fn collect_finite_indexes_from_type_expr(
    type_expr: &crate::desugar::desugared_ast::TypeExpr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if let crate::desugar::desugared_ast::TypeExprKind::Indexed { base, indexes } = &type_expr.kind
    {
        collect_finite_indexes_from_type_expr(base, registry, src)?;
        for index in indexes {
            collect_finite_index_expr(index, registry, src)?;
        }
    }
    match &type_expr.kind {
        crate::desugar::desugared_ast::TypeExprKind::TypeApplication { generic_args, .. } => {
            for arg in generic_args {
                collect_finite_indexes_from_generic_arg(arg, registry, src)?;
            }
        }
        crate::desugar::desugared_ast::TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                collect_finite_indexes_from_type_expr(arg, registry, src)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Register concrete `Fin(N)` identities used by comprehensions and tables.
fn collect_finite_indexes_from_expr(
    expr: &crate::desugar::desugared_ast::Expr,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::{ExprKind, ForBindingIndex};

    struct FiniteCollector<'a> {
        registry: &'a mut RegistryBuilder,
        src: &'a NamedSource<Arc<String>>,
    }

    impl crate::syntax::visitor::ExprVisitor<crate::syntax::phase::Desugared> for FiniteCollector<'_> {
        type Error = GraphcalError;

        fn visit_expr(
            &mut self,
            expr: &crate::desugar::desugared_ast::Expr,
        ) -> Result<(), GraphcalError> {
            match &expr.kind {
                ExprKind::ForComp { bindings, .. } => {
                    for binding in bindings {
                        if let ForBindingIndex::Finite { cardinality, span } = &binding.index
                            && let Some(value) = concrete_nat_value(cardinality, self.src)?
                        {
                            ensure_concrete_finite_index(value, *span, self.registry, self.src)?;
                        }
                    }
                }
                ExprKind::MapLiteral { entries } => {
                    for entry in entries {
                        for key in &entry.keys {
                            if let crate::syntax::ast::MapEntryIndex::Finite(cardinality) =
                                &key.index.value
                            {
                                ensure_concrete_finite_index(
                                    *cardinality,
                                    key.index.span,
                                    self.registry,
                                    self.src,
                                )?;
                            }
                        }
                    }
                }
                _ => {}
            }
            self.dispatch(expr)
        }
    }

    let mut collector = FiniteCollector { registry, src };
    collector.visit_expr(expr)
}

fn register_index_decl(
    idx: &crate::desugar::desugared_ast::IndexDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<(), GraphcalError> {
    let kind = match &idx.kind {
        crate::desugar::desugared_ast::IndexDeclKind::Named { variants } => {
            types::IndexKind::Named {
                variants: variants.iter().map(|v| v.value.clone()).collect(),
            }
        }
        crate::desugar::desugared_ast::IndexDeclKind::Range {
            start: start_expr,
            end: end_expr,
            step: step_expr,
        } => lower_range_index(
            &idx.name.value,
            start_expr,
            end_expr,
            step_expr,
            registry,
            src,
            decl_span,
        )?,
        crate::desugar::desugared_ast::IndexDeclKind::Linspace {
            start: start_expr,
            end: end_expr,
            points,
        } => lower_linspace_index(
            &idx.name.value,
            start_expr,
            end_expr,
            points,
            registry,
            src,
            decl_span,
        )?,
        crate::desugar::desugared_ast::IndexDeclKind::RequiredNamed => {
            types::IndexKind::RequiredNamed
        }
        crate::desugar::desugared_ast::IndexDeclKind::RequiredCoordinate { dimension } => {
            let dim = registry
                .resolve_dim_expr_detailed(dimension)
                .map_err(|err| dimension_resolve_error(err, src, dimension.span))?;
            types::IndexKind::RequiredCoordinate { dimension: dim }
        }
    };
    registry.register_index(types::IndexDef {
        name: idx.name.value.clone(),
        kind,
    });
    Ok(())
}

fn register_type_decl(
    t: &crate::desugar::desugared_ast::TypeDecl,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    validate_type_generic_params(t, src)?;
    let generic_params: Vec<types::TypeGenericParam> = t
        .generic_params
        .iter()
        .map(|g| types::TypeGenericParam {
            name: g.name.value.clone(),
            constraint: g.constraint.into(),
            default: g.default.clone(),
            span: g.name.span,
        })
        .collect();

    let type_def = match &t.body {
        crate::desugar::desugared_ast::TypeDeclBody::Required => {
            types::TypeDef::required(t.name.value.clone(), generic_params)
        }
        crate::desugar::desugared_ast::TypeDeclBody::Constructors(type_members) => {
            // Every constructor carries its payload inline; no per-constructor
            // TypeDef is synthesized. Checked construction keeps duplicate
            // payload fields out of the registry even for non-parser AST callers.
            let members = type_members
                .iter()
                .map(|member| {
                    let payload = match &member.payload {
                        Some(fields) => fields.as_slice(),
                        None => &[],
                    };
                    let fields = payload
                        .iter()
                        .map(|field| {
                            types::StructField::new(
                                field.name.value.clone(),
                                field.type_ann.clone(),
                            )
                        })
                        .collect();
                    types::UnionMemberDef::try_new(
                        ConstructorName::expect_valid(member.name.value.as_str()),
                        fields,
                    )
                    .map_err(|error| match error {
                        types::TypeDefError::DuplicateConstructorField {
                            constructor,
                            field,
                            first_index,
                            duplicate_index,
                        } => GraphcalError::DuplicateConstructorField {
                            type_name: t.name.value.clone(),
                            constructor,
                            field,
                            src: src.clone(),
                            duplicate: payload[duplicate_index].name.span.into(),
                            first: payload[first_index].name.span.into(),
                        },
                        types::TypeDefError::DuplicateConstructor { constructor } => {
                            GraphcalError::InternalError {
                                message: format!(
                                    "checked union member unexpectedly reported duplicate constructor `{constructor}`"
                                ),
                                src: src.clone(),
                                span: member.name.span.into(),
                            }
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            types::TypeDef::try_union(t.name.value.clone(), generic_params, members).map_err(
                |error| match error {
                    types::TypeDefError::DuplicateConstructor { constructor } => {
                        let mut declarations = type_members
                            .iter()
                            .filter(|member| member.name.value.as_str() == constructor.as_str());
                        let first = declarations
                            .next()
                            .map_or(t.name.span, |member| member.name.span);
                        let duplicate =
                            declarations.next().map_or(first, |member| member.name.span);
                        GraphcalError::DuplicateName {
                            name: constructor.to_string(),
                            src: src.clone(),
                            duplicate: duplicate.into(),
                            first: first.into(),
                        }
                    }
                    types::TypeDefError::DuplicateConstructorField { .. } => {
                        GraphcalError::InternalError {
                            message: "validated union member lost its field-uniqueness invariant"
                                .to_string(),
                            src: src.clone(),
                            span: t.name.span.into(),
                        }
                    }
                },
            )?
        }
    };

    registry.register_type(type_def);
    Ok(())
}

fn validate_type_generic_params(
    t: &crate::desugar::desugared_ast::TypeDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let positions = t.generic_params.iter().enumerate().try_fold(
        HashMap::new(),
        |mut positions, (index, param)| match positions.insert(
            param.name.value.atom().clone(),
            (param.name.value.clone(), index, param.name.span),
        ) {
            Some((name, _, _)) => Err(GraphcalError::EvalError {
                message: format!("duplicate generic parameter `{name}`"),
                src: src.clone(),
                span: param.name.span.into(),
            }),
            None => Ok(positions),
        },
    )?;

    let mut first_defaulted: Option<&GenericParamName> = None;
    for (index, param) in t.generic_params.iter().enumerate() {
        match &param.default {
            Some(default) => {
                first_defaulted.get_or_insert(&param.name.value);
                if let Some((referenced, span)) =
                    find_non_earlier_generic_reference(default, index, &positions)
                {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "default for generic parameter `{}` may reference only earlier generic parameters; `{referenced}` is not earlier",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: span.into(),
                    });
                }
            }
            None => {
                if let Some(first_defaulted) = first_defaulted {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "generic parameter `{}` without a default cannot follow defaulted parameter `{first_defaulted}`",
                            param.name.value
                        ),
                        src: src.clone(),
                        span: param.name.span.into(),
                    });
                }
            }
        }
    }
    Ok(())
}

type GenericParamPositions = HashMap<NameAtom, (GenericParamName, usize, Span)>;

fn find_non_earlier_generic_reference(
    arg: &crate::desugar::desugared_ast::GenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::GenericArg::Type(type_expr) => {
            find_non_earlier_type_reference(type_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Index(index) => {
            find_non_earlier_index_reference(index, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Nat(nat_expr) => {
            find_non_earlier_nat_reference(nat_expr, current_index, positions)
        }
        crate::syntax::ast::GenericArg::Ambiguous(ambiguous) => {
            find_non_earlier_ambiguous_reference(ambiguous, current_index, positions)
        }
    }
}

fn non_earlier_generic_reference(
    name: &NameAtom,
    span: Span,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    positions
        .get(name)
        .filter(|(_, index, _)| *index >= current_index)
        .map(|(name, _, _)| (name.clone(), span))
}

fn find_non_earlier_path_reference(
    path: &Spanned<NamePath>,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    path.value.is_bare().then_some(())?;
    non_earlier_generic_reference(path.value.leaf(), path.span, current_index, positions)
}

fn find_non_earlier_ambiguous_reference(
    arg: &crate::syntax::ast::AmbiguousGenericArg,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match arg {
        crate::syntax::ast::AmbiguousGenericArg::Name(ident) => {
            non_earlier_generic_reference(&ident.name, ident.span, current_index, positions)
        }
        crate::syntax::ast::AmbiguousGenericArg::Mul(operands, _) => {
            operands.iter().find_map(|operand| {
                find_non_earlier_ambiguous_reference(operand, current_index, positions)
            })
        }
    }
}

fn find_non_earlier_nat_reference(
    expr: &crate::syntax::ast::NatExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match expr {
        crate::syntax::ast::NatExpr::Literal(..) => None,
        crate::syntax::ast::NatExpr::Var(ident) => {
            non_earlier_generic_reference(&ident.name, ident.span, current_index, positions)
        }
        crate::syntax::ast::NatExpr::Add(operands, _)
        | crate::syntax::ast::NatExpr::Mul(operands, _) => operands
            .iter()
            .find_map(|operand| find_non_earlier_nat_reference(operand, current_index, positions)),
    }
}

fn find_non_earlier_index_reference(
    index: &crate::syntax::ast::IndexExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    match index {
        crate::syntax::ast::IndexExpr::Name(path) => {
            find_non_earlier_path_reference(path, current_index, positions)
        }
        crate::syntax::ast::IndexExpr::Finite { cardinality, .. }
        | crate::syntax::ast::IndexExpr::BareNat(cardinality) => {
            find_non_earlier_nat_reference(cardinality, current_index, positions)
        }
    }
}

fn find_non_earlier_type_reference(
    type_expr: &TypeExpr,
    current_index: usize,
    positions: &GenericParamPositions,
) -> Option<(GenericParamName, Span)> {
    use crate::desugar::desugared_ast::TypeExprKind;

    match &type_expr.kind {
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime => None,
        TypeExprKind::DimExpr(dim_expr) => dim_expr.terms.iter().find_map(|item| {
            find_non_earlier_path_reference(&item.term.name, current_index, positions)
        }),
        TypeExprKind::Indexed { base, indexes } => {
            find_non_earlier_type_reference(base, current_index, positions).or_else(|| {
                indexes.iter().find_map(|index| {
                    find_non_earlier_index_reference(index, current_index, positions)
                })
            })
        }
        TypeExprKind::TypeApplication { generic_args, .. } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => generic_args
            .iter()
            .find_map(|arg| find_non_earlier_generic_reference(arg, current_index, positions)),
        TypeExprKind::DatetimeApplication { type_args } => type_args.iter().find_map(|type_arg| {
            find_non_earlier_type_reference(type_arg, current_index, positions)
        }),
    }
}

/// Evaluate a constant scale expression (e.g. `1000`, `PI / 180`) to `f64`.
///
/// Scale expressions appear in unit definitions and are restricted to numeric
/// literals, built-in constants (`PI`, `E`), and basic arithmetic.
fn eval_scale_expr(expr: &Expr, src: &NamedSource<Arc<String>>) -> Result<f64, GraphcalError> {
    match &expr.kind {
        ExprKind::Number(n) => Ok(*n),
        #[expect(clippy::cast_precision_loss, reason = "unit scale constant expression")]
        ExprKind::Integer(n) => Ok(*n as f64),
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            // Route through the typed builtin-constant table instead of
            // string-matching a hand-picked subset: all built-in constants
            // (PI, E, TAU, SQRT2, LN2, LN10) are legal in scale expressions.
            let builtin = path
                .as_bare()
                .and_then(|ident| crate::builtin::BuiltinConst::parse(ident.name.as_str()));
            builtin
                .map(crate::builtin::BuiltinConst::value)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "unknown constant `{}` in scale expression; only built-in \
                         constants (PI, E, TAU, SQRT2, LN2, LN10) are supported",
                        path.display_path()
                    ),
                    src: src.clone(),
                    span: path.span().into(),
                })
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            use crate::desugar::desugared_ast::BinOp;
            use crate::syntax::ast::PowerExponent;
            let lhs_value = eval_scale_expr(lhs, src)?;
            if let BinOp::Pow(PowerExponent::Exact(exponent)) = op {
                return exponent
                    .pow_f64(lhs_value)
                    .map_err(|error| GraphcalError::EvalError {
                        message: error.to_string(),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
            }
            let rhs_value = eval_scale_expr(rhs, src)?;
            match op {
                BinOp::Add => Ok(lhs_value + rhs_value),
                BinOp::Sub => Ok(lhs_value - rhs_value),
                BinOp::Mul => Ok(lhs_value * rhs_value),
                BinOp::Div => Ok(lhs_value / rhs_value),
                BinOp::Pow(_) => Ok(lhs_value.powf(rhs_value)),
                _ => Err(GraphcalError::EvalError {
                    message: format!(
                        "unsupported operator `{op:?}` in scale expression; \
                         only `+`, `-`, `*`, `/`, `^` are allowed"
                    ),
                    src: src.clone(),
                    span: expr.span.into(),
                }),
            }
        }
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => Ok(-eval_scale_expr(operand, src)?),
        _ => Err(GraphcalError::EvalError {
            message: "scale expression must be a constant expression \
                      (numbers, PI, E, and arithmetic)"
                .to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Evaluate a statically known coordinate expression to its SI value and dimension.
///
/// This is a pure compile-time evaluator: graph references, calls, integer
/// values, and dynamic units are rejected. Arithmetic over quantity literals is
/// accepted so coordinate declarations are not limited to a single literal.
fn eval_coordinate_expr(
    expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(f64, crate::dimension::Dimension), GraphcalError> {
    use crate::desugar::desugared_ast::BinOp;
    use crate::dimension::Dimension;

    let ensure_finite = |value: f64, span: Span| {
        value.is_finite().then_some(value).ok_or_else(|| {
            eval_error(
                format!("coordinate expression must evaluate to a finite quantity, got {value}"),
                src,
                span,
            )
        })
    };

    match &expr.kind {
        ExprKind::Number(value) => Ok((
            ensure_finite(*value, expr.span)?,
            Dimension::dimensionless(),
        )),
        ExprKind::QuantityLiteral { value, unit } => {
            let (dimension, scale) = registry
                .resolve_unit_expr(unit)
                .map_err(|error| unit_resolve_to_graphcal(error, src, unit.span))?;
            let scale =
                validate_positive_finite_scale(scale, "coordinate unit scale", src, unit.span)?;
            Ok((ensure_finite(*value * scale.get(), expr.span)?, dimension))
        }
        ExprKind::UnresolvedRef(crate::syntax::ast::UnresolvedRef::Path(path)) => {
            let value = path
                .as_bare()
                .and_then(|ident| crate::builtin::BuiltinConst::parse(ident.name.as_str()))
                .map(crate::builtin::BuiltinConst::value)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!(
                        "coordinate expression must be statically evaluable; `{}` is not a built-in constant",
                        path.display_path()
                    ),
                    src: src.clone(),
                    span: path.span().into(),
                })?;
            Ok((value, Dimension::dimensionless()))
        }
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => {
            let (value, dimension) = eval_coordinate_expr(operand, registry, src)?;
            Ok((ensure_finite(-value, expr.span)?, dimension))
        }
        ExprKind::BinOp { op, lhs, rhs } => {
            let (lhs_value, lhs_dimension) = eval_coordinate_expr(lhs, registry, src)?;
            let (rhs_value, rhs_dimension) = eval_coordinate_expr(rhs, registry, src)?;
            let overflow = || GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: expr.span.into(),
            };
            let (value, dimension) = match op {
                BinOp::Add | BinOp::Sub => {
                    if lhs_dimension != rhs_dimension {
                        return Err(GraphcalError::EvalError {
                            message: "addition or subtraction in a coordinate expression requires matching dimensions".to_string(),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                    let value = if *op == BinOp::Add {
                        lhs_value + rhs_value
                    } else {
                        lhs_value - rhs_value
                    };
                    (value, lhs_dimension)
                }
                BinOp::Mul => (
                    lhs_value * rhs_value,
                    lhs_dimension
                        .checked_mul(&rhs_dimension)
                        .map_err(|_| overflow())?,
                ),
                BinOp::Div => (
                    lhs_value / rhs_value,
                    lhs_dimension
                        .checked_div(&rhs_dimension)
                        .map_err(|_| overflow())?,
                ),
                _ => {
                    return Err(eval_error(
                        "coordinate arguments support only static quantity arithmetic",
                        src,
                        expr.span,
                    ));
                }
            };
            Ok((ensure_finite(value, expr.span)?, dimension))
        }
        _ => Err(eval_error(
            "coordinate arguments must be statically evaluable quantities; Int values and runtime expressions are not supported",
            src,
            expr.span,
        )),
    }
}

fn coordinate_invalid(
    name: &IndexName,
    message: impl Into<String>,
    help: impl Into<String>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::CoordinateIndexInvalid {
        name: name.clone(),
        message: message.into(),
        help: help.into(),
        src: src.clone(),
        span: span.into(),
    }
}

/// One centralized binary64 endpoint comparison for coordinate construction.
fn coordinate_values_equal(actual: f64, expected: f64, scale_hint: f64) -> bool {
    let scale = actual.abs().max(expected.abs()).max(scale_hint.abs());
    // Keep the tolerance relative even for coordinates near zero. A unit-scale
    // floor would accept steps that miss tiny endpoints by a large fraction.
    // The subnormal floor covers a small number of binary64 ULPs at zero.
    let tolerance = (scale * (32.0 * f64::EPSILON)).max(f64::from_bits(32));
    (actual - expected).abs() <= tolerance
}

/// Exact numeric endpoint equality after finite-value validation.
fn coordinate_endpoints_equal(start: f64, end: f64) -> bool {
    matches!(start.partial_cmp(&end), Some(std::cmp::Ordering::Equal))
}

fn checked_range_cardinality(
    name: &IndexName,
    start: f64,
    end: f64,
    step: f64,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<types::IndexCardinality, GraphcalError> {
    if step == 0.0 {
        return Err(coordinate_invalid(
            name,
            "step must be nonzero",
            "use an explicitly positive or negative quantity step",
            src,
            span,
        ));
    }
    if (start < end && step < 0.0) || (start > end && step > 0.0) {
        return Err(coordinate_invalid(
            name,
            format!("step {step} moves away from endpoint {end}"),
            "use a positive step for an ascending range and a negative step for a descending range",
            src,
            span,
        ));
    }
    if coordinate_endpoints_equal(start, end) {
        return types::IndexCardinality::try_from_u64(1).map_err(|error| {
            coordinate_invalid(name, error.to_string(), "reduce the index size", src, span)
        });
    }

    let raw_intervals = (end - start) / step;
    if !raw_intervals.is_finite() || raw_intervals < 0.0 {
        return Err(coordinate_invalid(
            name,
            "range interval count is not a finite non-negative value",
            "choose finite bounds and a finite nonzero step with matching direction",
            src,
            span,
        ));
    }
    let intervals = raw_intervals.round();
    let reconstructed_end = intervals.mul_add(step, start);
    if intervals < 1.0 || !coordinate_values_equal(reconstructed_end, end, intervals * step) {
        return Err(coordinate_invalid(
            name,
            format!("step {step} does not land on endpoint {end}"),
            "change the endpoint or use `linspace(start, end, points: N)` when the point count is authoritative",
            src,
            span,
        ));
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "the practical limit is far below f64's exact integer range"
    )]
    if intervals >= types::MAX_INDEX_CARDINALITY as f64 {
        return Err(coordinate_invalid(
            name,
            format!(
                "range requires {} coordinate points, which exceeds the practical limit of {}",
                intervals + 1.0,
                types::MAX_INDEX_CARDINALITY
            ),
            format!(
                "reduce the index to at most {} points",
                types::MAX_INDEX_CARDINALITY
            ),
            src,
            span,
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the rounded interval count is finite, non-negative, and practically bounded"
    )]
    let count = intervals as u64 + 1;
    types::IndexCardinality::try_from_u64(count).map_err(|error| {
        coordinate_invalid(name, error.to_string(), "reduce the index size", src, span)
    })
}

fn eval_static_nat_expr(
    expr: &crate::desugar::desugared_ast::NatExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<u64, GraphcalError> {
    use crate::desugar::desugared_ast::NatExpr;
    match expr {
        NatExpr::Literal(value, _) => Ok(*value),
        NatExpr::Var(ident) => Err(GraphcalError::EvalError {
            message: format!(
                "linspace point count must be statically known; `{}` is not a constant Nat",
                ident.name
            ),
            src: src.clone(),
            span: ident.span.into(),
        }),
        NatExpr::Add(operands, span) => operands.iter().try_fold(0_u64, |sum, operand| {
            sum.checked_add(eval_static_nat_expr(operand, src)?)
                .ok_or_else(|| eval_error("linspace point-count addition overflow", src, *span))
        }),
        NatExpr::Mul(operands, span) => operands.iter().try_fold(1_u64, |product, operand| {
            product
                .checked_mul(eval_static_nat_expr(operand, src)?)
                .ok_or_else(|| {
                    eval_error("linspace point-count multiplication overflow", src, *span)
                })
        }),
    }
}

fn coordinate_display_unit(
    start_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(Option<String>, f64), GraphcalError> {
    let unit = match &start_expr.kind {
        ExprKind::QuantityLiteral { unit, .. } => unit,
        ExprKind::UnaryOp {
            op: crate::desugar::desugared_ast::UnaryOp::Neg,
            operand,
        } => return coordinate_display_unit(operand, registry, src),
        _ => return Ok((None, 1.0)),
    };
    match registry.resolve_unit_expr(unit) {
        Ok((_dimension, scale)) => {
            let scale = validate_positive_finite_scale(
                scale,
                "coordinate display unit scale",
                src,
                unit.span,
            )?;
            Ok((Some(format_unit_expr_with_config(unit, true)), scale.get()))
        }
        Err(crate::registry::types::UnitResolveError::Overflow(_)) => {
            Err(GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: unit.span.into(),
            })
        }
        Err(crate::registry::types::UnitResolveError::InvalidScale { value, reason }) => {
            let reason = match reason {
                PositiveFiniteScaleError::NonFinite => "must be finite",
                PositiveFiniteScaleError::NonPositive => "must be greater than zero",
            };
            Err(eval_error(
                format!("coordinate display unit scale {reason}, got {value}"),
                src,
                unit.span,
            ))
        }
        Err(_) => Ok((None, 1.0)),
    }
}

fn validate_coordinate_values(
    name: &IndexName,
    data: &types::CoordinateIndexData,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    let direction = data.end.total_cmp(&data.start);
    let count = data.cardinality.get();
    let first = data.coordinate_value(0);
    (1..count).try_fold(first, |previous, position| {
        let current = data.coordinate_value(position);
        if !current.is_finite() {
            return Err(coordinate_invalid(
                name,
                format!("coordinate at position {position} is not finite"),
                "reduce the cardinality or use bounds and spacing with stable binary64 coordinates",
                src,
                span,
            ));
        }
        let monotonic = match direction {
            std::cmp::Ordering::Less => current < previous,
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Greater => current > previous,
        };
        if !monotonic {
            return Err(coordinate_invalid(
                name,
                format!(
                    "coordinates at positions {} and {position} are duplicate or non-monotonic in binary64",
                    position - 1
                ),
                "reduce the cardinality or increase the spacing between adjacent coordinates",
                src,
                span,
            ));
        }
        Ok(current)
    })?;
    Ok(())
}

/// Lower `range(start, end, step: delta)` with exact endpoint semantics.
fn lower_range_index(
    name: &IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    step_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<types::IndexKind, GraphcalError> {
    let (start, start_dimension) = eval_coordinate_expr(start_expr, registry, src)?;
    let (end, end_dimension) = eval_coordinate_expr(end_expr, registry, src)?;
    let (step, step_dimension) = eval_coordinate_expr(step_expr, registry, src)?;
    if start_dimension != end_dimension || start_dimension != step_dimension {
        return Err(GraphcalError::CoordinateIndexDimensionMismatch {
            name: name.clone(),
            message: format!(
                "range start, end, and step have dimensions {}, {}, and {}",
                registry.format_dimension(&start_dimension),
                registry.format_dimension(&end_dimension),
                registry.format_dimension(&step_dimension)
            ),
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    let cardinality = checked_range_cardinality(name, start, end, step, src, decl_span)?;
    let (display_label, display_scale) = coordinate_display_unit(start_expr, registry, src)?;
    let data = types::CoordinateIndexData {
        start,
        end,
        spacing: types::CoordinateSpacing::Step { step },
        cardinality,
        dimension: start_dimension,
        display_label,
        display_scale,
    };
    if cardinality.get() > 1 {
        validate_coordinate_values(name, &data, src, decl_span)?;
    }
    Ok(types::IndexKind::Coordinate(data))
}

/// Lower `linspace(start, end, points: N)` with exact endpoint semantics.
fn lower_linspace_index(
    name: &IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    points_expr: &crate::desugar::desugared_ast::NatExpr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: Span,
) -> Result<types::IndexKind, GraphcalError> {
    let (start, start_dimension) = eval_coordinate_expr(start_expr, registry, src)?;
    let (end, end_dimension) = eval_coordinate_expr(end_expr, registry, src)?;
    if start_dimension != end_dimension {
        return Err(GraphcalError::CoordinateIndexDimensionMismatch {
            name: name.clone(),
            message: format!(
                "linspace start and end have dimensions {} and {}",
                registry.format_dimension(&start_dimension),
                registry.format_dimension(&end_dimension)
            ),
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    let points = eval_static_nat_expr(points_expr, src)?;
    if points == 0 {
        return Err(coordinate_invalid(
            name,
            "linspace requires at least one point",
            "use `points: 1` or greater",
            src,
            points_expr.span(),
        ));
    }
    let cardinality = types::IndexCardinality::try_from_u64(points).map_err(|error| {
        coordinate_invalid(
            name,
            error.to_string(),
            format!(
                "use a point count from 1 through {}",
                types::MAX_INDEX_CARDINALITY
            ),
            src,
            points_expr.span(),
        )
    })?;
    match (cardinality.get(), coordinate_endpoints_equal(start, end)) {
        (1, false) => {
            return Err(coordinate_invalid(
                name,
                "linspace with one point requires identical start and end values",
                "set end equal to start or request at least two points",
                src,
                decl_span,
            ));
        }
        (2.., true) => {
            return Err(coordinate_invalid(
                name,
                "linspace with two or more points requires distinct endpoints",
                "use `points: 1` for a singleton or choose distinct endpoints",
                src,
                decl_span,
            ));
        }
        _ => {}
    }
    let (display_label, display_scale) = coordinate_display_unit(start_expr, registry, src)?;
    let data = types::CoordinateIndexData {
        start,
        end,
        spacing: types::CoordinateSpacing::Linspace,
        cardinality,
        dimension: start_dimension,
        display_label,
        display_scale,
    };
    if cardinality.get() > 1 {
        validate_coordinate_values(name, &data, src, decl_span)?;
    }
    Ok(types::IndexKind::Coordinate(data))
}

/// Extract a map of type annotations from const/param/node declarations,
/// keyed by their typed declaration names.
pub(super) fn extract_type_annotations(ast: &File) -> HashMap<DeclName, TypeExpr> {
    let mut type_anns = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.clone(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.clone(), n.type_ann.clone());
            }
            DeclKind::ConstNode(c) => {
                type_anns.insert(c.name.value.clone(), c.type_ann.clone());
            }
            _ => {}
        }
    }
    type_anns
}

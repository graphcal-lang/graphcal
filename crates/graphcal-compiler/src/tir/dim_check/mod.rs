use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart};
use crate::syntax::index_name::{IndexEntryKey, IndexName};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Span;
use crate::syntax::type_name::StructTypeName;

use crate::registry::builtins::builtin_functions;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::SemanticRegistry;
use crate::tir::typed::{FiniteIndexIdentity, NatPolyForm};

pub(crate) use helpers::{expect_quantity, format_inferred_type};

use helpers::{format_declared_type, is_bool_type, resolved_type_matches_inferred, types_match};

mod builtins;
mod helpers;
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "inference functions pass compilation context through many parameters; \
              large match on ExprKind variants is inherently long"
)]
mod infer;
mod model_schema;
mod plot;
mod presentation;

pub use presentation::checked_expression_presentation;

pub use model_schema::{
    ConcreteModelConstructor, ConcreteModelField, ConcreteModelType, ConcreteModelTypeError,
    ValidatedModelType,
};
#[cfg(test)]
mod tests;

pub use crate::registry::declared_type::DeclaredType;

/// Index identity carried by inferred collection/label types.
///
/// Declared indexes compare by owner-qualified [`IndexTypeRef`]. Structural
/// finite indexes additionally carry their normalized Nat form so generic axes
/// such as `Fin(N + 1)` are not encoded in or compared through synthetic strings.
#[derive(Debug, Clone, Eq)]
pub(crate) struct InferredIndex {
    reference: IndexTypeRef,
}

impl InferredIndex {
    #[must_use]
    pub(crate) fn from_resolved(resolved: ResolvedIndexName) -> Self {
        Self {
            reference: IndexTypeRef::from_resolved(resolved),
        }
    }

    #[must_use]
    pub(crate) const fn from_ref(reference: IndexTypeRef) -> Self {
        Self { reference }
    }

    /// Create an inferred finite structural index from a validated finite-index identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity cannot be converted to an index type reference.
    fn from_finite_index_identity(
        identity: &FiniteIndexIdentity,
    ) -> Result<Self, crate::registry::types::FiniteIndexError> {
        Ok(Self {
            reference: identity.to_index_type_ref()?,
        })
    }

    /// Create an inferred finite structural index from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid finite structural size.
    pub(crate) fn from_finite_index_form(
        form: NatPolyForm,
    ) -> Result<Self, crate::registry::types::FiniteIndexError> {
        Self::from_finite_index_identity(&FiniteIndexIdentity::try_from_form(form)?)
    }

    #[must_use]
    pub(crate) const fn type_ref(&self) -> &IndexTypeRef {
        &self.reference
    }

    #[must_use]
    pub(crate) fn name(&self) -> IndexName {
        self.reference.display_name()
    }

    #[must_use]
    pub(crate) const fn declared_resolved(&self) -> Option<&ResolvedIndexName> {
        self.reference.declared_resolved()
    }

    #[must_use]
    const fn concrete_finite_index(&self) -> Option<crate::registry::types::FiniteIndex> {
        self.reference.finite_index()
    }

    #[must_use]
    pub(crate) fn finite_index_form(&self) -> Option<NatPolyForm> {
        self.reference.finite_index_form()
    }

    #[must_use]
    pub(crate) fn matches_resolved(&self, expected: &ResolvedIndexName) -> bool {
        self.declared_resolved() == Some(expected)
    }

    #[must_use]
    fn matches_ref(&self, expected: &IndexTypeRef) -> bool {
        self.reference.matches_ref(expected)
    }
}

impl PartialEq for InferredIndex {
    fn eq(&self, other: &Self) -> bool {
        self.reference.matches_ref(&other.reference)
    }
}

impl std::fmt::Display for InferredIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reference.fmt(f)
    }
}

/// Struct/type identity carried by inferred constructor, match, and field types.
///
/// Equality is owner-sensitive; leaf-only names must be resolved before they
/// become inferred semantic types.
#[derive(Debug, Clone, Eq)]
pub(crate) struct InferredStructType {
    reference: StructTypeRef,
}

impl InferredStructType {
    #[must_use]
    pub(crate) fn from_resolved(resolved: ResolvedStructTypeName) -> Self {
        Self {
            reference: StructTypeRef::from_resolved(resolved),
        }
    }

    #[must_use]
    const fn from_ref(reference: StructTypeRef) -> Self {
        Self { reference }
    }

    #[must_use]
    const fn type_ref(&self) -> &StructTypeRef {
        &self.reference
    }

    #[must_use]
    const fn name(&self) -> &StructTypeName {
        self.reference.name()
    }

    #[must_use]
    pub(crate) const fn resolved(&self) -> &ResolvedStructTypeName {
        self.reference.resolved()
    }

    #[must_use]
    fn matches_resolved(&self, expected: &ResolvedStructTypeName) -> bool {
        self.resolved() == expected
    }

    #[must_use]
    fn matches_ref(&self, expected: &StructTypeRef) -> bool {
        self.reference.matches_ref(expected)
    }
}

impl PartialEq for InferredStructType {
    fn eq(&self, other: &Self) -> bool {
        self.reference.matches_ref(&other.reference)
    }
}

impl std::fmt::Display for InferredStructType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reference.fmt(f)
    }
}

impl std::ops::Deref for InferredStructType {
    type Target = StructTypeName;

    fn deref(&self) -> &Self::Target {
        self.name()
    }
}

/// A generic argument inferred at a constructor or type-application site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InferredGenericArg {
    Dim(Dimension),
    Index(InferredIndex),
    Nat(crate::nat::NatPolyForm),
    Type(InferredType),
}

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InferredType {
    Quantity(Dimension),
    /// A complex quantity whose real and imaginary components share one dimension.
    Complex(Dimension),
    Bool,
    Int,
    /// A datetime instant in a specific time scale.
    Datetime(TimeScale),
    /// An index-key value of type `Key<I>`: a first-class element key of
    /// axis `I`.
    Key(InferredIndex),
    /// An index identity carried only while checking an `Index`-sorted generic
    /// argument. This may be a declared or structural finite index and is not a
    /// Graphcal value type.
    IndexArg(InferredIndex),
    /// A struct type with sort-aware generic arguments.
    Struct(InferredStructType, Vec<InferredGenericArg>),
    Indexed {
        element: Box<Self>,
        index: InferredIndex,
    },
}

impl InferredType {
    #[must_use]
    pub(crate) const fn complex_dimension(&self) -> Option<&Dimension> {
        match self {
            Self::Complex(dimension) => Some(dimension),
            _ => None,
        }
    }

    const fn quantity_dimension(&self) -> Option<&Dimension> {
        match self {
            Self::Quantity(dimension) => Some(dimension),
            _ => None,
        }
    }

    /// Number of index axes carried by this type.
    #[must_use]
    fn indexed_rank(&self) -> usize {
        match self {
            Self::Indexed { element, .. } => element.indexed_rank().saturating_add(1),
            _ => 0,
        }
    }
}

/// Per-DAG context bundle threaded through the dimension-check passes.
///
/// Bundles the read-only inputs that every per-declaration check needs
/// (declared types, the locals scope, TIR, registry, builtins, source)
/// so individual helpers take a single `&DimCheckContext` instead of
/// six positional arguments.
#[derive(Clone, Copy)]
struct DimCheckContext<'a> {
    cancellation: &'a crate::cancellation::CancellationToken,
    materialized_shapes: &'a infer::hir::MaterializedShapeCollector,
    declared_types: &'a HashMap<ScopedName, DeclaredType>,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    tir: &'a crate::tir::typed::TIR,
    registry: &'a SemanticRegistry,
    builtin_fns: &'a crate::registry::builtins::BuiltinFunctions,
    src: &'a NamedSource<Arc<String>>,
}

impl<'a> DimCheckContext<'a> {
    /// Re-anchor diagnostics on `body_src`, the source a particular
    /// declaration's spans index into (#868). A declaration merged in from an
    /// instantiated dependency keeps that dependency file's offsets, so its
    /// checks must render against the dependency source rather than the
    /// importer's ambient `src`.
    const fn for_body(self, body_src: &'a NamedSource<Arc<String>>) -> Self {
        Self {
            src: body_src,
            ..self
        }
    }
}

impl DimCheckContext<'_> {
    fn checkpoint(&self) -> Result<(), GraphcalError> {
        self.cancellation.checkpoint().map_err(GraphcalError::from)
    }

    /// Look up the module-aware HIR expression for a local declaration.
    fn hir_expr_for_decl(
        &self,
        name: &crate::syntax::module_name::ScopedName,
    ) -> Option<&crate::hir::Expr> {
        let dag = self.dag?;
        let key = dag.resolved_decl_key_for_local(name);
        dag.value_expr(&key)
    }

    /// Look up the module-aware HIR assertion body for a local assertion.
    fn hir_assert_body(
        &self,
        name: &crate::syntax::module_name::ScopedName,
        span: crate::syntax::span::Span,
    ) -> Result<&crate::hir::AssertBody, GraphcalError> {
        let dag = self.dag.ok_or_else(|| GraphcalError::InternalError {
            message: "HIR assertion lookup requires semantic DAG context".to_string(),
            src: self.src.clone(),
            span: span.into(),
        })?;
        let key = dag.resolved_decl_key_for_local(name);
        dag.assert_body(&key)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("TIR assertion entry missing for `{name}`"),
                src: self.src.clone(),
                span: span.into(),
            })
    }

    /// Infer the type of a module-aware HIR expression using this context's bindings.
    fn infer_hir(
        &self,
        expr: &crate::hir::Expr,
        owner: &ResolvedDeclName,
    ) -> Result<InferredType, GraphcalError> {
        let dag = self.dag.ok_or_else(|| GraphcalError::InternalError {
            message: "HIR assertion inference requires semantic DAG context".to_string(),
            src: self.src.clone(),
            span: expr.span.into(),
        })?;
        infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
            expr,
            Some(owner),
            self.declared_types,
            dag,
            self.tir,
            self.registry,
            self.builtin_fns,
            self.src,
            self.cancellation,
            self.materialized_shapes.clone(),
        )
    }
}

fn validate_decl_concrete_type_obligations(
    ctx: &DimCheckContext<'_>,
    name: &ScopedName,
    type_ann_span: Span,
) -> Result<(), GraphcalError> {
    let declared = ctx
        .declared_types
        .get(name)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("no declared type recorded for `{name}`"),
            src: ctx.src.clone(),
            span: type_ann_span.into(),
        })?;
    let dag = ctx.dag.ok_or_else(|| GraphcalError::InternalError {
        message: format!("semantic DAG missing while validating `{name}`"),
        src: ctx.src.clone(),
        span: type_ann_span.into(),
    })?;
    let inferred = InferredType::from(declared);
    infer::hir::validate_concrete_type_obligations(
        &inferred,
        dag,
        ctx.tir,
        ctx.registry,
        ctx.builtin_fns,
        ctx.src,
        type_ann_span,
        ctx.cancellation,
    )
}

fn validate_hir_concrete_type_obligations(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    let dag = ctx.dag.ok_or_else(|| GraphcalError::InternalError {
        message: "semantic DAG missing while validating constructor applications".to_string(),
        src: ctx.src.clone(),
        span: Span::new(0, 0).into(),
    })?;
    for application in concrete_constructor_applications(ctx.tir, dag, ctx.src)? {
        ctx.checkpoint()?;
        let inferred = InferredType::Struct(
            InferredStructType::from_ref(application.identity().clone()),
            application
                .generic_args()
                .iter()
                .map(InferredGenericArg::from)
                .collect(),
        );
        infer::hir::validate_concrete_type_obligations(
            &inferred,
            dag,
            ctx.tir,
            ctx.registry,
            ctx.builtin_fns,
            ctx.src,
            Span::new(0, 0),
            ctx.cancellation,
        )?;
    }
    Ok(())
}

/// Check that a declaration's expression type matches its declared type annotation.
fn check_decl_expr_type(
    body_ctx: &DimCheckContext<'_>,
    name: &crate::syntax::module_name::ScopedName,
    type_ann_span: &crate::syntax::span::Span,
    annotation_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let declared =
        body_ctx
            .declared_types
            .get(name)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("no declared type recorded for `{name}`"),
                src: annotation_src.clone(),
                span: (*type_ann_span).into(),
            })?;
    let dag = body_ctx.dag.ok_or_else(|| GraphcalError::InternalError {
        message: format!("semantic DAG missing while checking `{name}`"),
        src: body_ctx.src.clone(),
        span: (*type_ann_span).into(),
    })?;
    let hir_expr =
        body_ctx
            .hir_expr_for_decl(name)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("value declaration record missing while checking `{name}`"),
                src: body_ctx.src.clone(),
                span: (*type_ann_span).into(),
            })?;
    let owner = dag.resolved_decl_key_for_local(name);
    let inferred = infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
        hir_expr,
        Some(&owner),
        body_ctx.declared_types,
        dag,
        body_ctx.tir,
        body_ctx.registry,
        body_ctx.builtin_fns,
        body_ctx.src,
        body_ctx.cancellation,
        body_ctx.materialized_shapes.clone(),
    )?;
    let matches = body_ctx
        .dag
        .and_then(|dag| dag.resolved_decl_types.get(name))
        .map_or_else(
            || types_match(declared, &inferred),
            |resolved| resolved_type_matches_inferred(resolved, &inferred),
        );
    if !matches {
        return Err(GraphcalError::DimensionMismatchInAnnotation {
            declared: format_declared_type(declared, body_ctx.registry),
            inferred: format_inferred_type(&inferred, body_ctx.registry),
            src: annotation_src.clone(),
            span: (*type_ann_span).into(),
        });
    }
    check_ineffective_conversions(hir_expr, true, body_ctx.src)?;
    Ok(())
}

/// Require every runtime unit factor to be one scalar Dimensionless quantity.
fn check_dynamic_unit_scale_types(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    let Some(dag) = ctx.dag else {
        return Ok(());
    };
    for entry in dag.semantic.dynamic_unit_scales.values() {
        ctx.checkpoint()?;
        if entry.declared_dimension != entry.base_unit_dimension {
            return Err(GraphcalError::UnitDefinitionDimensionMismatch {
                name: entry.spelling.name().clone(),
                declared: ctx
                    .registry
                    .dimensions
                    .format_dimension(&entry.declared_dimension),
                definition: ctx
                    .registry
                    .dimensions
                    .format_dimension(&entry.base_unit_dimension),
                src: entry.src.clone(),
                span: entry.span.into(),
            });
        }
        let entry_ctx = ctx.for_body(&entry.src);
        let inferred = infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
            &entry.expr,
            None,
            entry_ctx.declared_types,
            dag,
            entry_ctx.tir,
            entry_ctx.registry,
            entry_ctx.builtin_fns,
            entry_ctx.src,
            entry_ctx.cancellation,
            entry_ctx.materialized_shapes.clone(),
        )?;
        if !matches!(
            &inferred,
            InferredType::Quantity(dimension) if dimension.is_dimensionless()
        ) {
            return Err(GraphcalError::DynamicUnitScaleTypeMismatch {
                name: entry.spelling.clone(),
                found: format_inferred_type(&inferred, entry_ctx.registry),
                src: entry.src.clone(),
                span: entry.expr.span.into(),
            });
        }
    }
    Ok(())
}

/// Reject `->` conversions whose display effect is discarded (#648 B3).
///
/// A conversion only matters in a *display position*: the top level of a
/// declaration body, a selected `if`/`match` branch, a constructor field
/// initializer, a map-literal entry, a for-comprehension body, or a
/// `scan`/`unfold` init. Anywhere else — arithmetic operands, function
/// arguments, comparison operands, conditions, scrutinees, assertion bodies —
/// the conversion evaluates to the unchanged SI value and its display target
/// is silently dropped, so it is either a typo or dead code.
fn check_ineffective_conversions(
    expr: &crate::hir::Expr,
    display_position: bool,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Recursion choke point: recurses once per tree level.
    crate::stack::with_stack_growth(|| {
        check_ineffective_conversions_inner(expr, display_position, src)
    })
}

fn check_ineffective_conversions_inner(
    expr: &crate::hir::Expr,
    display_position: bool,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::hir::ExprKind;
    match &expr.kind {
        ExprKind::Convert { expr: inner, .. } | ExprKind::DisplayTimezone { expr: inner, .. } => {
            if !display_position {
                return Err(GraphcalError::IneffectiveConversion {
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            // The operand of a conversion is not itself a display position
            // (direct nesting is already rejected as D012).
            check_ineffective_conversions(inner, false, src)
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_ineffective_conversions(condition, false, src)?;
            check_ineffective_conversions(then_branch, display_position, src)?;
            check_ineffective_conversions(else_branch, display_position, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_ineffective_conversions(scrutinee, false, src)?;
            for arm in arms {
                check_ineffective_conversions(&arm.body, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::ConstructorCall { fields, .. } => {
            for init in fields {
                check_ineffective_conversions(&init.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                check_ineffective_conversions(&entry.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => {
            check_ineffective_conversions(body, display_position, src)
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_ineffective_conversions(source, false, src)?;
            check_ineffective_conversions(init, display_position, src)?;
            check_ineffective_conversions(body, false, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_ineffective_conversions(init, display_position, src)?;
            check_ineffective_conversions(body, false, src)
        }
        ExprKind::KeyForm { arg, .. } => check_ineffective_conversions(arg, false, src),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_ineffective_conversions(lhs, false, src)?;
            check_ineffective_conversions(rhs, false, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_ineffective_conversions(operand, false, src),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                check_ineffective_conversions(arg, false, src)?;
            }
            Ok(())
        }
        ExprKind::FieldAccess { expr: inner, .. } => {
            check_ineffective_conversions(inner, false, src)
        }
        ExprKind::IndexAccess { expr: inner, args } => {
            check_ineffective_conversions(inner, false, src)?;
            for arg in args {
                if let crate::hir::expr::IndexArg::Expr(e) = arg {
                    check_ineffective_conversions(e, false, src)?;
                }
            }
            Ok(())
        }
        // Inline-dag param bindings flow into the callee's params, whose
        // reads propagate display metadata; treat them as display positions.
        ExprKind::DagCall { args, .. } => {
            for binding in args {
                check_ineffective_conversions(&binding.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::Error { .. }
        | ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::OffsetDateTimeLiteral(_)
        | ExprKind::CivilDateTimeLiteral(_)
        | ExprKind::ZonedDateTimeLiteral(_)
        | ExprKind::IanaTimeZoneLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::QuantityLiteral { .. }
        | ExprKind::VariantLiteral(_) => Ok(()),
    }
}

#[derive(Debug)]
struct AssertionIndexShape {
    axes: Vec<InferredIndex>,
}

impl AssertionIndexShape {
    fn from_bool_type(ty: &InferredType) -> Self {
        Self {
            axes: peel_index_axes(ty).0,
        }
    }

    const fn is_indexed(&self) -> bool {
        !self.axes.is_empty()
    }

    const fn rank(&self) -> usize {
        self.axes.len()
    }
}

/// Check dimensions for a lowered HIR assertion body.
fn check_hir_assert_body(
    ctx: &DimCheckContext<'_>,
    owner: &ResolvedDeclName,
    body: &crate::hir::AssertBody,
    span: crate::syntax::span::Span,
) -> Result<AssertionIndexShape, GraphcalError> {
    let registry = ctx.registry;
    let src = ctx.src;
    match body {
        crate::hir::AssertBody::Expr(body_expr) => {
            let inferred = ctx.infer_hir(body_expr, owner)?;
            if !is_bool_type(&inferred) {
                return Err(GraphcalError::AssertBodyNotBool {
                    found: format_inferred_type(&inferred, registry),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(AssertionIndexShape::from_bool_type(&inferred))
        }
        crate::hir::AssertBody::Tolerance {
            actual,
            expected,
            tolerance,
        } => {
            let actual_type = ctx.infer_hir(actual, owner)?;
            let expected_type = ctx.infer_hir(expected, owner)?;
            let tolerance_type = ctx.infer_hir(tolerance, owner)?;

            // Element-wise broadcasting (#809): the assertion's index shape
            // comes from `actual`; `expected` and `tolerance` are each unindexed
            // (broadcast to every key) or indexed by exactly the same axes.
            let (actual_axes, actual_elem) = peel_index_axes(&actual_type);
            let expected_elem = broadcast_operand_element(
                &actual_axes,
                &actual_type,
                &expected_type,
                expected.span,
                registry,
                src,
            )?;
            let tolerance_elem = broadcast_operand_element(
                &actual_axes,
                &actual_type,
                &tolerance_type,
                tolerance.span,
                registry,
                src,
            )?;

            let actual_dim = expect_quantity(actual_elem, registry, src, actual.span)?;
            let expected_dim = expect_quantity(expected_elem, registry, src, expected.span)?;
            if actual_dim != expected_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&actual_dim),
                    found: registry.dimensions.format_dimension(&expected_dim),
                    help: "actual and expected in tolerance assertion must have the same dimension"
                        .to_string(),
                    src: src.clone(),
                    span: expected.span.into(),
                });
            }

            let tolerance_dim = expect_quantity(tolerance_elem, registry, src, tolerance.span)?;
            if tolerance_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&actual_dim),
                    found: format_inferred_type(&tolerance_type, registry),
                    help: "absolute tolerance must have the same dimension as actual/expected"
                        .to_string(),
                    src: src.clone(),
                    span: tolerance.span.into(),
                });
            }

            // A negative tolerance makes the assertion unsatisfiable (even an
            // exact match fails `abs(delta) <= tol`), so a statically-known
            // negative value is a compile error (#815). Tolerances computed
            // at runtime are validated by the evaluator instead.
            if let Some(value) = statically_known_tolerance(tolerance)
                && value < 0.0
            {
                return Err(GraphcalError::NegativeTolerance {
                    found: crate::registry::format::format_number(value),
                    src: src.clone(),
                    span: tolerance.span.into(),
                });
            }
            Ok(AssertionIndexShape { axes: actual_axes })
        }
    }
}

/// Peel the index axes off an inferred type, outermost first.
fn peel_index_axes(ty: &InferredType) -> (Vec<InferredIndex>, &InferredType) {
    let mut axes = Vec::new();
    let mut current = ty;
    while let InferredType::Indexed { element, index } = current {
        axes.push(index.clone());
        current = element;
    }
    (axes, current)
}

/// Validate that a tolerance-assertion operand broadcasts against `actual`'s
/// axes (#809): it is either unindexed (applied to every key) or indexed by
/// exactly the same axes in the same order. Returns the operand's element
/// type.
fn broadcast_operand_element<'a>(
    actual_axes: &[InferredIndex],
    actual_type: &InferredType,
    operand_type: &'a InferredType,
    operand_span: crate::syntax::span::Span,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<&'a InferredType, GraphcalError> {
    let (operand_axes, operand_elem) = peel_index_axes(operand_type);
    if !operand_axes.is_empty() && operand_axes != *actual_axes {
        return Err(GraphcalError::IndexedShapeMismatch {
            context: "tolerance assertion".to_string(),
            lhs: format_inferred_type(actual_type, registry),
            rhs: format_inferred_type(operand_type, registry),
            src: src.clone(),
            span: operand_span.into(),
        });
    }
    Ok(operand_elem)
}

/// Structurally fold a tolerance expression to its written literal value
/// when the sign is statically known: a numeric literal (`0.1`, `5`,
/// `0.1 m` — unit scales are always positive, so the written value carries
/// the sign), optionally under unary negation. Returns `None` for anything
/// computed at runtime; those are sign-checked by the evaluator instead.
fn statically_known_tolerance(expr: &crate::hir::Expr) -> Option<f64> {
    match &expr.kind {
        crate::hir::ExprKind::Number(n) => Some(*n),
        #[expect(
            clippy::cast_precision_loss,
            reason = "tolerance literals are small integers"
        )]
        crate::hir::ExprKind::Integer(i) => Some(*i as f64),
        crate::hir::ExprKind::QuantityLiteral { value, .. } => Some(*value),
        crate::hir::ExprKind::UnaryOp {
            op: crate::syntax::ast::UnaryOp::Neg,
            operand,
        } => statically_known_tolerance(operand).map(|v| -v),
        _ => None,
    }
}

fn expected_fail_key_span(key: &ExpectedFailKey) -> crate::syntax::span::Span {
    key.iter()
        .map(ExpectedFailKeyPart::span)
        .reduce(crate::syntax::span::Span::merge)
        .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0))
}

fn expected_fail_key_signature(
    key: &ExpectedFailKey,
) -> Vec<(Option<IndexTypeRef>, IndexEntryKey)> {
    key.iter()
        .map(|part| (part.named_index().cloned(), part.entry_key()))
        .collect()
}

fn validate_expected_fail_key(
    key: &ExpectedFailKey,
    shape: &AssertionIndexShape,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if key.len() != shape.rank() {
        return Err(GraphcalError::ExpectedFailKeyShapeMismatch {
            expected: shape.rank(),
            found: key.len(),
            src: src.clone(),
            span: expected_fail_key_span(key).into(),
        });
    }

    for (part, expected_axis) in key.iter().zip(&shape.axes) {
        match part {
            ExpectedFailKeyPart::Named { index, .. } => {
                if !index.matches_ref(expected_axis.type_ref()) {
                    return Err(GraphcalError::ExpectedFailKeyIndexMismatch {
                        expected: expected_axis.name().to_string(),
                        found: part.display(),
                        src: src.clone(),
                        span: part.span().into(),
                    });
                }
            }
            ExpectedFailKeyPart::FinitePosition { position, span } => {
                let Some(finite) = expected_axis.type_ref().finite_index_ref() else {
                    return Err(GraphcalError::ExpectedFailKeyIndexMismatch {
                        expected: expected_axis.name().to_string(),
                        found: part.display(),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                };
                // Bound-check `#N` against a statically known Fin cardinality.
                // Symbolic cardinalities are checked after substitution.
                if let Some(concrete) = finite.concrete_index()
                    && *position >= concrete.size_u64()
                {
                    return Err(GraphcalError::ExpectedFailFinitePositionOutOfBounds {
                        position: *position,
                        size: concrete.size_u64(),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
            }
        }
    }

    Ok(())
}

fn validate_expected_fail(
    expected_fail: &ExpectedFail,
    shape: &AssertionIndexShape,
    src: &NamedSource<Arc<String>>,
    attribute_span: crate::syntax::span::Span,
) -> Result<(), GraphcalError> {
    match expected_fail {
        ExpectedFail::All if shape.is_indexed() => Err(GraphcalError::ExpectedFailAllOnIndexed {
            src: src.clone(),
            span: attribute_span.into(),
        }),
        ExpectedFail::All => Ok(()),
        ExpectedFail::Variants(keys) if !shape.is_indexed() => {
            Err(GraphcalError::ExpectedFailNotIndexed {
                src: src.clone(),
                span: keys
                    .first()
                    .map_or(attribute_span, expected_fail_key_span)
                    .into(),
            })
        }
        ExpectedFail::Variants(keys) => {
            let mut seen = HashSet::new();
            for key in keys {
                validate_expected_fail_key(key, shape, src)?;
                if !seen.insert(expected_fail_key_signature(key)) {
                    return Err(GraphcalError::ExpectedFailDuplicateKey {
                        src: src.clone(),
                        span: expected_fail_key_span(key).into(),
                    });
                }
            }
            Ok(())
        }
    }
}

/// Check dimensions for all declarations in a file.
///
/// For each const/param/node, infers the dimension of the RHS expression
/// and verifies it matches the declared type annotation. Uses
/// `tir.build_declared_types()` (derived from `resolved_decl_types`) to validate
/// that every RHS expression matches its declared type annotation.
///
/// This is a pure validation step — returns `()` on success.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if dimensions are inconsistent.
pub fn check_dimensions_tir(
    tir: &mut crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    check_dimensions_tir_with_cancellation(
        tir,
        src,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Check dimensions while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for invalid dimensions or cancellation.
pub fn check_dimensions_tir_with_cancellation(
    tir: &mut crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(), GraphcalError> {
    cancellation.checkpoint()?;
    detect_decl_cycles(tir, src)?;
    detect_cross_dag_cycles(tir, src)?;
    let builtin_fns = builtin_functions();

    // Dim-check the file's own DAGs (root + inline children) against the
    // file's shared registry. Dep DAGs merged in by `merge_dep_dag_tirs`
    // were already dim-checked in their own file's pipeline, against
    // their own registry — re-checking them here against the importer's
    // registry would fail on types renamed by include bindings.
    let checked_dag_facts = tir
        .local_dags()
        .map(|(dag_id, dag)| {
            cancellation.checkpoint()?;
            let collector = infer::hir::MaterializedShapeCollector::default();
            let plot_dimensions = check_dimensions_dag(
                dag,
                tir,
                &tir.registry,
                builtin_fns,
                src,
                cancellation,
                &collector,
            )?;
            Ok((dag_id.clone(), collector.snapshot(), plot_dimensions))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let mut checked_plot_dimensions = HashMap::new();
    for (dag_id, shapes, plot_dimensions) in checked_dag_facts {
        let dag = tir
            .dags
            .get_mut(&dag_id)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("checked DAG `{dag_id}` disappeared while installing shape facts"),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        dag.semantic.materialized_shapes = shapes;
        checked_plot_dimensions.insert(dag_id, plot_dimensions);
    }

    // Validate domain constraints on HIR nominal fields. Types reachable
    // through dep imports were already validated in their defining file's
    // pipeline, so the redundant pass is idempotent. (#450 Position 1+2.)
    cancellation.checkpoint()?;
    check_field_domain_constraint_targets(tir, src)?;
    cancellation.checkpoint()?;
    check_field_domain_constraint_dimensions(tir, &tir.registry, builtin_fns, src, cancellation)?;

    cancellation.checkpoint()?;
    let presentation_facts =
        presentation::collect_presentation_facts(tir, &checked_plot_dimensions, src, cancellation)?;
    for (dag_id, facts) in presentation_facts {
        let dag = tir
            .dags
            .get_mut(&dag_id)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "checked DAG `{dag_id}` disappeared while installing presentation facts"
                ),
                src: src.clone(),
                span: Span::new(0, 0).into(),
            })?;
        dag.semantic.presentation = facts;
    }

    Ok(())
}

/// Canonical nominal identity whose use in a parameter default is queried at
/// an include boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NominalOverrideIdentity {
    Index(ResolvedIndexName),
    Type(ResolvedStructTypeName),
}

/// Canonical nominal dependencies keyed by the producer parameter default that
/// uses them.
pub type OverrideDependencySummary = HashMap<ResolvedDeclName, HashSet<NominalOverrideIdentity>>;

/// Refine include override obligations from independently checked source modules.
///
/// HIR records each possible nominal override before substituting an include
/// instance. Once a source module has been checked, its canonical dependency
/// summary distinguishes a true dependency on the replaced nominal from an
/// unrelated use of the replacement type or index itself.
pub fn reconcile_external_override_dependencies<S>(
    tir: &mut crate::tir::typed::TIR,
    summary: &OverrideDependencySummary,
    complete_owners: &HashSet<crate::dag_id::DagId, S>,
) where
    S: std::hash::BuildHasher,
{
    tir.root_mut()
        .semantic
        .override_reconciliations
        .retain(|_, reconciliations| {
            reconciliations.retain_mut(|reconciliation| {
                if !complete_owners.contains(reconciliation.source_decl.owner()) {
                    return true;
                }
                let dependencies = summary.get(&reconciliation.source_decl);
                reconciliation.targets.retain(|target| {
                    let source = match target {
                        crate::tir::typed::ResolvedOverrideTarget::Index { source, .. } => {
                            NominalOverrideIdentity::Index(source.clone())
                        }
                        crate::tir::typed::ResolvedOverrideTarget::Type { source, .. } => {
                            NominalOverrideIdentity::Type(source.clone())
                        }
                    };
                    dependencies.is_some_and(|dependencies| dependencies.contains(&source))
                });
                !reconciliation.targets.is_empty()
            });
            !reconciliations.is_empty()
        });
}

/// Collect canonical nominal dependencies for every checked parameter default
/// in every DAG module in `tir`.
///
/// Each default is inferred exactly once before include substitution. Canonical
/// field, constructor, match, index, and type-argument events are collected by
/// that inference and filtered to the module's typed `pub(bind)` interface.
/// The resulting summary is reusable at every include site.
///
/// # Errors
///
/// Returns a compiler diagnostic if a supposedly checked TIR can no longer be
/// inferred consistently.
pub fn collect_override_dependency_summary(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<OverrideDependencySummary, GraphcalError> {
    collect_override_dependency_summary_with_cancellation(
        tir,
        src,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

/// Collect override dependencies while observing cooperative cancellation.
///
/// # Errors
///
/// Returns a compiler diagnostic for inconsistent TIR or cancellation.
pub fn collect_override_dependency_summary_with_cancellation(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<OverrideDependencySummary, GraphcalError> {
    let builtin_fns = builtin_functions();
    let mut summary = OverrideDependencySummary::new();

    for (_, dag) in tir.local_dags() {
        cancellation.checkpoint()?;
        let declared_types = dag.build_declared_types(src)?;
        for param in &dag.params {
            let Some(default_expr) = &param.default_expr else {
                continue;
            };
            cancellation.checkpoint()?;
            let default_src =
                param
                    .default_src
                    .as_ref()
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "parameter `{}` is missing default provenance",
                            param.name
                        ),
                        src: src.clone(),
                        span: param.span.into(),
                    })?;
            let body_src = default_src.resolve(src);
            let owner = dag.resolved_decl_key_for_local(&param.name);
            let (_, mut dependencies) =
                infer::hir::infer_hir_type_with_nominal_dependencies_and_cancellation(
                    default_expr,
                    &owner,
                    &declared_types,
                    dag,
                    tir,
                    &tir.registry,
                    builtin_fns,
                    body_src,
                    cancellation,
                )?;
            dependencies.retain(|identity| is_bindable_nominal(dag, identity));
            if !dependencies.is_empty() {
                summary.insert(owner, dependencies);
            }
        }
    }
    Ok(summary)
}

fn is_bindable_nominal(
    dag: &crate::tir::typed::DagTIR,
    identity: &NominalOverrideIdentity,
) -> bool {
    let bindable = match identity {
        NominalOverrideIdentity::Index(index) => {
            crate::tir::typed::BindableNominalIdentity::Index(index.clone())
        }
        NominalOverrideIdentity::Type(struct_type) => {
            crate::tir::typed::BindableNominalIdentity::Type(struct_type.clone())
        }
    };
    dag.semantic.bindable_nominals.contains(&bindable)
}

/// One exact concrete nominal application observed in semantic HIR.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConcreteNominalTypeApplication {
    identity: StructTypeRef,
    generic_args: Vec<crate::registry::declared_type::DeclaredGenericArg>,
}

impl ConcreteNominalTypeApplication {
    #[must_use]
    pub const fn identity(&self) -> &StructTypeRef {
        &self.identity
    }

    #[must_use]
    pub fn generic_args(&self) -> &[crate::registry::declared_type::DeclaredGenericArg] {
        &self.generic_args
    }
}

/// Collect exact concrete nominal applications from every semantic expression
/// in one DAG, including constructor calls inside field bounds.
///
/// # Errors
///
/// Returns a compiler diagnostic when a constructor application is not exact,
/// sort-correct, and concrete.
pub fn concrete_constructor_applications(
    tir: &crate::tir::typed::TIR,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<ConcreteNominalTypeApplication>, GraphcalError> {
    let mut applications = HashSet::new();
    let mut error = None;
    dag.visit_expressions(&mut |expr| {
        if error.is_some() {
            return;
        }
        let resolved = match &expr.kind {
            crate::hir::ExprKind::ConstructorCall {
                callee,
                generic_args,
                ..
            } => concrete_constructor_generic_args(
                tir,
                dag,
                &callee.value,
                generic_args,
                src,
                callee.span,
            )
            .map(|generic_args| (callee.value.clone(), generic_args)),
            crate::hir::ExprKind::ConstRef(target) => match &target.value {
                crate::hir::ConstRef::Constructor(constructor) => {
                    concrete_constructor_generic_args(tir, dag, constructor, &[], src, target.span)
                        .map(|generic_args| (constructor.clone(), generic_args))
                }
                crate::hir::ConstRef::Decl(_)
                | crate::hir::ConstRef::Builtin(_)
                | crate::hir::ConstRef::TimeScale(_)
                | crate::hir::ConstRef::GenericNatParam(_) => return,
            },
            _ => return,
        };
        match resolved {
            Ok((constructor, generic_args)) => {
                let Some(target) = dag
                    .semantic
                    .constructor_refs
                    .constructor_defs
                    .get(&constructor)
                else {
                    error = Some(GraphcalError::InternalError {
                        message: format!(
                            "semantic constructor metadata missing for `{constructor}`"
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                    return;
                };
                applications.insert(ConcreteNominalTypeApplication {
                    identity: StructTypeRef::from_resolved(target.owning_type.clone()),
                    generic_args,
                });
            }
            Err(diagnostic) => error = Some(diagnostic),
        }
    });
    error.map_or_else(|| Ok(applications.into_iter().collect()), Err)
}

/// Resolve one HIR constructor call's complete concrete generic identity.
///
/// Evaluation uses this checked compiler boundary rather than reimplementing
/// generic sorting/default substitution. The returned arguments are exact,
/// concrete, and in declaration order.
///
/// # Errors
///
/// Returns a compiler diagnostic when constructor metadata is missing or the
/// application is not a concrete, valid application of its owning type.
pub fn concrete_constructor_generic_args(
    tir: &crate::tir::typed::TIR,
    dag: &crate::tir::typed::DagTIR,
    constructor: &ResolvedConstructorName,
    applied_generic_args: &[crate::hir::GenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Vec<crate::registry::declared_type::DeclaredGenericArg>, GraphcalError> {
    let target = dag
        .semantic
        .constructor_refs
        .constructor_defs
        .get(constructor)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("semantic constructor metadata missing for `{constructor}`"),
            src: src.clone(),
            span: span.into(),
        })?;
    infer::hir::resolve_concrete_generic_args(
        &target.owning_type,
        &target.type_def,
        applied_generic_args,
        dag,
        tir,
        &tir.registry,
        src,
        span,
    )
}

/// Check one already-lowered external value expression against a concrete
/// declared type in this TIR's root module.
///
/// This is the compiler-facing half of runtime parameter binding: callers may
/// lower a closed value expression independently of declaration compilation,
/// then reuse the normal HIR inference rules rather than duplicating unit,
/// datetime, constructor, generic, key, or indexed type checking.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when the expression is not well typed in the
/// root module or does not exactly match `expected`.
#[expect(
    clippy::implicit_hasher,
    reason = "the inference core uses the compiler's canonical HashMap type"
)]
pub fn check_external_value_expr_type(
    tir: &crate::tir::typed::TIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    expr: &crate::hir::Expr,
    expected: &DeclaredType,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();
    let inferred = infer::hir::infer_hir_type_with_owner(
        expr,
        None,
        declared_types,
        tir.root(),
        tir,
        &tir.registry,
        builtin_fns,
        src,
    )?;
    infer::hir::validate_concrete_type_obligations(
        &inferred,
        tir.root(),
        tir,
        &tir.registry,
        builtin_fns,
        src,
        expr.span,
        &crate::cancellation::CancellationToken::unbounded(),
    )?;
    if types_match(expected, &inferred) {
        Ok(())
    } else {
        Err(GraphcalError::DimensionMismatchInAnnotation {
            declared: format_declared_type(expected, &tir.registry),
            inferred: format_inferred_type(&inferred, &tir.registry),
            src: src.clone(),
            span: expr.span.into(),
        })
    }
}

/// Dim-check a single [`DagTIR`] against the file's shared registry and
/// the full flat dag map.
fn check_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &crate::registry::types::SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
    materialized_shapes: &infer::hir::MaterializedShapeCollector,
) -> Result<plot::CheckedPlotChannelDimensions, GraphcalError> {
    cancellation.checkpoint()?;
    let declared_types = dag.build_declared_types(src)?;
    let ctx = DimCheckContext {
        cancellation,
        materialized_shapes,
        declared_types: &declared_types,
        dag: Some(dag),
        tir,
        registry,
        builtin_fns,
        src,
    };

    // Declarations merged in from instantiated dependencies keep the
    // dependency file's spans, so each is checked against its own source (#868).
    for entry in &dag.consts {
        ctx.checkpoint()?;
        let annotation_src = entry.type_src.resolve(src);
        let annotation_ctx = ctx.for_body(annotation_src);
        validate_decl_concrete_type_obligations(&annotation_ctx, &entry.name, entry.type_ann.span)?;
        let body_ctx = ctx.for_body(entry.body_src.resolve(src));
        check_decl_expr_type(&body_ctx, &entry.name, &entry.type_ann.span, annotation_src)?;
    }
    for entry in &dag.nodes {
        ctx.checkpoint()?;
        let annotation_src = entry.type_src.resolve(src);
        let annotation_ctx = ctx.for_body(annotation_src);
        validate_decl_concrete_type_obligations(&annotation_ctx, &entry.name, entry.type_ann.span)?;
        let body_ctx = ctx.for_body(entry.body_src.resolve(src));
        check_decl_expr_type(&body_ctx, &entry.name, &entry.type_ann.span, annotation_src)?;
    }
    for entry in &dag.params {
        ctx.checkpoint()?;
        let annotation_src = entry.type_src.resolve(src);
        let annotation_ctx = ctx.for_body(annotation_src);
        validate_decl_concrete_type_obligations(&annotation_ctx, &entry.name, entry.type_ann.span)?;
        let Some(_value_expr) = entry.default_expr.as_ref() else {
            continue;
        };
        let Some(default_src) = entry.default_src.as_ref() else {
            return Err(GraphcalError::InternalError {
                message: format!("parameter `{}` is missing default provenance", entry.name),
                src: annotation_src.clone(),
                span: entry.span.into(),
            });
        };
        let body_ctx = ctx.for_body(default_src.resolve(src));
        check_decl_expr_type(&body_ctx, &entry.name, &entry.type_ann.span, annotation_src)?;
    }

    ctx.checkpoint()?;
    validate_hir_concrete_type_obligations(&ctx)?;

    ctx.checkpoint()?;
    check_dynamic_unit_scale_types(&ctx)?;

    for entry in &dag.asserts {
        ctx.checkpoint()?;
        let body_src = entry.body_src.resolve(src);
        let entry_ctx = ctx.for_body(body_src);
        let body = entry_ctx.hir_assert_body(&entry.name, entry.span)?;
        let owner = dag.resolved_decl_key_for_local(&entry.name);
        let shape = check_hir_assert_body(&entry_ctx, &owner, body, entry.span)?;
        if let Some(metadata) = dag.expected_fail.get(&entry.name) {
            validate_expected_fail(
                &metadata.expected,
                &shape,
                &metadata.src,
                metadata.attribute_span,
            )?;
        }
        // Assertion results are never displayed with units, so no position
        // inside an assert body is display-effective.
        match body {
            crate::hir::expr::AssertBody::Expr(e) => {
                check_ineffective_conversions(e, false, body_src)?;
            }
            crate::hir::expr::AssertBody::Tolerance {
                actual,
                expected,
                tolerance,
                ..
            } => {
                check_ineffective_conversions(actual, false, body_src)?;
                check_ineffective_conversions(expected, false, body_src)?;
                check_ineffective_conversions(tolerance, false, body_src)?;
            }
        }
    }

    ctx.checkpoint()?;
    let plot_dimensions = plot::check_plot_properties_dag(&ctx, dag)?;

    ctx.checkpoint()?;
    check_domain_constraint_targets_dag(dag, src)?;
    ctx.checkpoint()?;
    check_domain_constraint_dimensions_dag(&ctx)?;

    Ok(plot_dimensions)
}

/// What a domain bound expression must infer to for a given target type.
enum ExpectedBound {
    /// Bound must be `Quantity(d)`. `Int` is also accepted when `d` is dimensionless.
    Quantity(Dimension),
    /// Bound must be exactly `Int`, preserving the full `i64` range.
    Int,
    /// Bound must be a datetime in exactly this declared time scale.
    Datetime(crate::registry::time_scale::TimeScale),
}

/// Check that domain constraint bound expressions have the correct type.
///
/// For each param/node with `(min: ..., max: ...)` constraints whose target type
/// is a quantity, `Int`, or `Datetime<S>`, infers the type of each bound
/// expression using the regular type checker and verifies it matches:
/// - `Quantity(d)` target: bound must be `Quantity(d)` (or `Int` if `d` is dimensionless).
/// - `Dimensionless` target: bound must be `Quantity(dimensionless)` or `Int`.
/// - `Int` target: bound must be exactly `Int`.
/// - `Datetime<S>` target: bound must be exactly `Datetime<S>`.
///
/// Other targets (e.g., `Bool`) are rejected by
/// [`check_domain_constraint_targets_dag`] before this bound check runs.
fn check_domain_constraint_dimensions_dag(ctx: &DimCheckContext<'_>) -> Result<(), GraphcalError> {
    let dag = ctx.dag.ok_or_else(|| GraphcalError::InternalError {
        message: "domain-bound checking requires semantic DAG context".to_string(),
        src: ctx.src.clone(),
        span: Span::new(0, 0).into(),
    })?;
    // A merged dependency declaration's domain bounds keep the dependency
    // file's spans, so they are checked against that body's source (#868).
    let decl_iter = dag
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_src))
        .chain(dag.params.iter().map(|e| (&e.name, &e.type_src)))
        .chain(dag.nodes.iter().map(|e| (&e.name, &e.type_src)));

    for (name, signature_provenance) in decl_iter {
        let key = dag.resolved_decl_key_for_local(name);
        let bounds = dag.semantic.domain_bounds.get(&key);
        let Some(bounds) = bounds else {
            continue;
        };
        let body_src = signature_provenance.resolve(ctx.src);

        let resolved = dag.resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        let expected = match base_resolved {
            Some(crate::tir::typed::ResolvedTypeExpr::Quantity(dim)) => {
                ExpectedBound::Quantity(dim.clone())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Dimensionless) => {
                ExpectedBound::Quantity(Dimension::dimensionless())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Int) => ExpectedBound::Int,
            Some(crate::tir::typed::ResolvedTypeExpr::Datetime(scale)) => {
                ExpectedBound::Datetime(*scale)
            }
            _ => continue,
        };

        for bound in bounds {
            let inferred = infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
                &bound.value,
                Some(&key),
                ctx.declared_types,
                dag,
                ctx.tir,
                ctx.registry,
                ctx.builtin_fns,
                body_src,
                ctx.cancellation,
                ctx.materialized_shapes.clone(),
            )?;
            check_one_bound(name, bound, &inferred, &expected, ctx.registry, body_src)?;
        }
    }

    Ok(())
}

fn check_one_bound(
    name: &crate::syntax::module_name::ScopedName,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    check_one_bound_with_display_name(&name.to_string(), bound, inferred, expected, registry, src)
}

/// Reject domain constraints on base types that don't accept them.
///
/// Bool, Label, and algebraic types cannot carry `(min: …, max: …)` bounds.
/// The check is a pure function of the resolved declaration type—independent
/// of any bound expression's value—so it belongs in compile-time validation
/// rather than runtime resolution.
fn check_domain_constraint_targets_dag(
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let decl_iter = dag
        .consts
        .iter()
        .map(|entry| (&entry.name, entry.span))
        .chain(dag.params.iter().map(|entry| (&entry.name, entry.span)))
        .chain(dag.nodes.iter().map(|entry| (&entry.name, entry.span)));

    for (name, decl_span) in decl_iter {
        let key = dag.resolved_decl_key_for_local(name);
        if !dag.semantic.domain_bounds.contains_key(&key) {
            continue;
        }
        let Some(resolved) = dag.resolved_decl_types.get(name) else {
            continue;
        };
        if let Some(type_kind) = invalid_domain_target_kind(resolved) {
            return Err(GraphcalError::InvalidDomainTarget {
                type_kind,
                src: src.clone(),
                span: decl_span.into(),
            });
        }
    }
    Ok(())
}

fn invalid_domain_target_kind(resolved: &crate::tir::typed::ResolvedTypeExpr) -> Option<String> {
    use crate::tir::typed::ResolvedTypeExpr;

    match resolved {
        ResolvedTypeExpr::Indexed { base, .. } => invalid_domain_target_kind(base),
        ResolvedTypeExpr::Bool => Some("Bool".to_string()),
        ResolvedTypeExpr::Complex { .. } => Some("Complex".to_string()),
        ResolvedTypeExpr::Key { .. } => Some("Key".to_string()),
        ResolvedTypeExpr::IndexArg(index) => {
            Some(format!("index {}", index.format_for_diagnostic()))
        }
        ResolvedTypeExpr::Struct(struct_name, _)
        | ResolvedTypeExpr::GenericStruct {
            name: struct_name, ..
        } => Some(format!("struct `{}`", struct_name.as_str())),
        ResolvedTypeExpr::GenericTypeParam(param, _) => {
            Some(format!("generic Type parameter `{param}`"))
        }
        ResolvedTypeExpr::Quantity(_)
        | ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericDimExpr { .. } => None,
    }
}

/// Reject constraints on struct/union fields outside the explicitly
/// constrainable value families. This mirrors
/// [`check_domain_constraint_targets_dag`] using the field's resolved semantic
/// type rather than reclassifying source names.
fn check_field_domain_constraint_targets(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut seen = std::collections::HashSet::new();
    for (_, dag) in tir.local_dags() {
        for (key, field_semantics) in dag.semantic.type_defs.constrained_fields() {
            if !seen.insert(key) {
                continue;
            }
            let Some(type_kind) = invalid_domain_target_kind(field_semantics.resolved_type())
            else {
                continue;
            };
            let bounds = field_semantics.domain_bounds();
            let span = field_type_annotation(dag, key).map_or_else(
                || {
                    bounds
                        .first()
                        .map_or_else(|| Span::new(0, 0), |bound| bound.span)
                },
                |field| field.type_annotation().span,
            );
            let diagnostic_src = bounds.first().map_or(src, |bound| &bound.src);
            return Err(GraphcalError::InvalidDomainTarget {
                type_kind,
                src: diagnostic_src.clone(),
                span: span.into(),
            });
        }
    }
    Ok(())
}

fn field_type_annotation<'a>(
    dag: &'a crate::tir::typed::DagTIR,
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
) -> Option<&'a crate::hir::NominalField> {
    dag.semantic
        .type_defs
        .struct_types
        .get(&key.owning_type)?
        .union_members()?
        .iter()
        .find(|member| member.name() == key.constructor)?
        .fields()
        .iter()
        .find(|field| field.name() == &key.field)
}

fn field_constraint_definition_dag<'a>(
    tir: &'a crate::tir::typed::TIR,
    key: &crate::tir::typed::ResolvedStructFieldTypeKey,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<&'a crate::tir::typed::DagTIR, GraphcalError> {
    tir.dag_registry()
        .get(key.owning_type.owner())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "field-constraint owner `{}` has no checked DAG",
                key.owning_type.owner()
            ),
            src: src.clone(),
            span: span.into(),
        })
}

/// Check that domain bound expressions on struct/union fields have the
/// correct type. Mirrors [`check_domain_constraint_dimensions_dag`] for
/// top-level decls.
///
/// Field bounds cross into HIR with their nominal definition; the same
/// owner-qualified field can be referenced
/// from several DAGs, so a seen-set dedupes the checks.
fn check_field_domain_constraint_dimensions(
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(), GraphcalError> {
    let mut seen = HashSet::new();
    for (_, dag) in tir.local_dags() {
        for (key, field_semantics) in dag.semantic.type_defs.constrained_fields() {
            if !seen.insert(key) {
                continue;
            }
            let diagnostic_bound = field_semantics.domain_bounds().first();
            let diagnostic_src = diagnostic_bound.map_or(src, |bound| &bound.src);
            let diagnostic_span = diagnostic_bound.map_or(Span::new(0, 0), |bound| bound.span);
            let type_def = dag
                .semantic
                .type_defs
                .struct_types
                .get(&key.owning_type)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "semantic type metadata missing constrained type `{}`",
                        key.owning_type
                    ),
                    src: diagnostic_src.clone(),
                    span: diagnostic_span.into(),
                })?;
            let (variant, field) = type_def
                .union_members()
                .and_then(|members| {
                    members
                        .iter()
                        .flat_map(|member| member.fields().iter().map(move |field| (member, field)))
                        .find(|(member, field)| {
                            member.name() == key.constructor && field.name() == &key.field
                        })
                })
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "semantic type metadata missing constrained field `{}.{}`",
                        key.constructor, key.field
                    ),
                    src: diagnostic_src.clone(),
                    span: diagnostic_span.into(),
                })?;
            let resolved_target = strip_indexed(field_semantics.resolved_type());
            let expected = expected_bound_from_resolved(resolved_target);
            let deferred_generic_quantity = matches!(
                resolved_target,
                crate::tir::typed::ResolvedTypeExpr::GenericDimParam(_, _)
                    | crate::tir::typed::ResolvedTypeExpr::GenericDimExpr { .. }
            );
            if expected.is_none() && !deferred_generic_quantity {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "constrained field target `{}` was not classified",
                        resolved_target.format(registry)
                    ),
                    src: diagnostic_src.clone(),
                    span: diagnostic_span.into(),
                });
            }
            // For a single-variant collision (record-shape) the display
            // name is `Type.field`; for a true multi-variant union it's
            // `Type.Variant.field` so diagnostics disambiguate which
            // constructor a violating bound belongs to.
            let display_name = if variant.name().as_str() == type_def.name().as_str() {
                format!("{}.{}", type_def.name(), field.name())
            } else {
                format!("{}.{}.{}", type_def.name(), variant.name(), field.name())
            };
            let definition_dag =
                field_constraint_definition_dag(tir, key, diagnostic_src, diagnostic_span)?;
            for bound in field_semantics.domain_bounds() {
                let definition_types = definition_dag.build_declared_types(&bound.src)?;
                let inferred =
                    infer::hir::infer_hir_type_with_materialized_shapes_and_cancellation(
                        &bound.value,
                        None,
                        &definition_types,
                        definition_dag,
                        tir,
                        registry,
                        builtin_fns,
                        &bound.src,
                        cancellation,
                        infer::hir::MaterializedShapeCollector::default(),
                    )?;
                match &expected {
                    Some(expected) => check_one_bound_with_display_name(
                        &display_name,
                        bound,
                        &inferred,
                        expected,
                        registry,
                        &bound.src,
                    )?,
                    None => check_deferred_generic_quantity_bound(
                        &display_name,
                        resolved_target,
                        bound,
                        &inferred,
                        registry,
                    )?,
                }
            }
        }
    }
    Ok(())
}

fn expected_bound_from_resolved(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
) -> Option<ExpectedBound> {
    match resolved {
        crate::tir::typed::ResolvedTypeExpr::Quantity(dimension) => {
            Some(ExpectedBound::Quantity(dimension.clone()))
        }
        crate::tir::typed::ResolvedTypeExpr::Dimensionless => {
            Some(ExpectedBound::Quantity(Dimension::dimensionless()))
        }
        crate::tir::typed::ResolvedTypeExpr::Int => Some(ExpectedBound::Int),
        crate::tir::typed::ResolvedTypeExpr::Datetime(scale) => {
            Some(ExpectedBound::Datetime(*scale))
        }
        _ => None,
    }
}

fn expected_bound_from_inferred(inferred: &InferredType) -> Option<ExpectedBound> {
    match inferred {
        InferredType::Indexed { element, .. } => expected_bound_from_inferred(element),
        InferredType::Quantity(dimension) => Some(ExpectedBound::Quantity(dimension.clone())),
        InferredType::Int => Some(ExpectedBound::Int),
        InferredType::Datetime(scale) => Some(ExpectedBound::Datetime(*scale)),
        InferredType::Complex(_)
        | InferredType::Bool
        | InferredType::Key(_)
        | InferredType::IndexArg(_)
        | InferredType::Struct(..) => None,
    }
}

fn check_deferred_generic_quantity_bound(
    display_name: &str,
    resolved_target: &crate::tir::typed::ResolvedTypeExpr,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &InferredType,
    registry: &SemanticRegistry,
) -> Result<(), GraphcalError> {
    if inferred.quantity_dimension().is_some() || matches!(inferred, InferredType::Int) {
        return Ok(());
    }
    Err(GraphcalError::DomainDimensionMismatch {
        name: display_name.to_string(),
        type_dim: resolved_target.format(registry),
        bound_name: bound.kind.to_string(),
        bound_dim: format_inferred_type(inferred, registry),
        src: bound.src.clone(),
        span: bound.span.into(),
    })
}

/// Variant of [`check_one_bound`] that takes a pre-formatted display name
/// for the constrained target (e.g. `"SatelliteSpec.mass"`) so a single
/// helper can serve both top-level decls and struct fields.
fn check_one_bound_with_display_name(
    display_name: &str,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Quantity(target_dim) => {
            let ok = match inferred {
                InferredType::Int => target_dim.is_dimensionless(),
                other => other.quantity_dimension() == Some(target_dim),
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = inferred.quantity_dimension().map_or_else(
                || format_inferred_type(inferred, registry),
                |d| registry.dimensions.format_dimension(d),
            );
            Err(GraphcalError::DomainDimensionMismatch {
                name: display_name.to_string(),
                type_dim: registry.dimensions.format_dimension(target_dim),
                bound_name: bound.kind.to_string(),
                bound_dim: bound_dim_str,
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Int => {
            if matches!(inferred, InferredType::Int) {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundTypeMismatch {
                name: display_name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Datetime(target_scale) => {
            if matches!(inferred, InferredType::Datetime(bound_scale) if bound_scale == target_scale)
            {
                return Ok(());
            }
            Err(GraphcalError::DatetimeDomainBoundTypeMismatch {
                name: display_name.to_string(),
                target_type: format_inferred_type(&InferredType::Datetime(*target_scale), registry),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}

/// Strip `Indexed` wrappers to get the base resolved type.
fn strip_indexed(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
) -> &crate::tir::typed::ResolvedTypeExpr {
    match resolved {
        crate::tir::typed::ResolvedTypeExpr::Indexed { base, .. } => strip_indexed(base),
        other => other,
    }
}

/// Detect cycles in the cross-dag inline-call graph.
///
/// A dag `A` that transitively inline-calls itself — directly or through a
/// chain `A → B → … → A` — would recurse unboundedly at evaluation time. We
/// reject such programs at compile time with
/// [`GraphcalError::CyclicDependency`] pointing at one dag involved in the
/// cycle (chosen deterministically by the DFS entry order).
///
/// Per the issue thread, a dag — not a file — is the semantic unit of
/// cycle detection, so the same check applies whether the cycle is within
/// a single file or spans multiple files.
enum DagCycleFrame {
    Enter(crate::dag_id::DagId),
    Leave(crate::dag_id::DagId),
}

/// Collect DAG-call targets directly from every semantic HIR root.
fn collect_dag_call_targets_from_dag(
    dag: &crate::tir::typed::DagTIR,
    out: &mut std::collections::BTreeSet<crate::dag_id::DagId>,
) {
    dag.visit_expressions(&mut |expr| {
        if let crate::hir::ExprKind::DagCall { target, .. } = &expr.kind {
            out.insert(target.value.clone());
        }
    });
}

/// Detect cycles in same-file declaration dependencies.
///
/// A graph cycle is a topological property of source — knowable without
/// evaluating any value. This check rejects cyclic params/nodes (`runtime_deps`)
/// and cyclic consts (`const_deps`) at compile time so the diagnostic appears
/// under `graphcal check`, not only at evaluation. Mirrors the toposort-based
/// cycle detection in `graphcal-eval`'s `exec_plan::eval_consts_from_tir` and
/// `build_runtime_dag`, which now act as defense-in-depth backstops.
fn detect_decl_cycles(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use std::collections::BTreeSet;

    use petgraph::algo::toposort;
    use petgraph::graph::DiGraph;

    use crate::syntax::module_name::ScopedName;

    type ResolvedDeclKey = ResolvedDeclName;

    fn check_resolved<'a>(
        dag: &crate::tir::typed::DagTIR,
        names_with_spans: impl Iterator<Item = (&'a ScopedName, crate::syntax::span::Span)>,
        deps: &HashMap<ResolvedDeclKey, BTreeSet<ResolvedDeclKey>>,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let mut graph = DiGraph::<ResolvedDeclKey, ()>::new();
        let mut index_map: HashMap<ResolvedDeclKey, petgraph::graph::NodeIndex> = HashMap::new();
        let mut local_name_by_key: HashMap<ResolvedDeclKey, ScopedName> = HashMap::new();
        let mut span_by_key: HashMap<ResolvedDeclKey, crate::syntax::span::Span> = HashMap::new();
        for (name, span) in names_with_spans {
            let key = dag.resolved_decl_key_for_local(name);
            let idx = graph.add_node(key.clone());
            index_map.insert(key.clone(), idx);
            local_name_by_key.insert(key.clone(), name.clone());
            span_by_key.insert(key, span);
        }
        if index_map.is_empty() {
            return Ok(());
        }
        for (name, dep_set) in deps {
            let Some(&to) = index_map.get(name) else {
                continue;
            };
            for dep in dep_set {
                if let Some(&from) = index_map.get(dep) {
                    graph.add_edge(from, to, ());
                }
            }
        }
        toposort(&graph, None).map(|_| ()).map_err(|cycle| {
            let cycle_node = &graph[cycle.node_id()];
            let span = span_by_key
                .get(cycle_node)
                .copied()
                .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0));
            let name = local_name_by_key
                .get(cycle_node)
                .map_or_else(|| cycle_node.to_string(), std::string::ToString::to_string);
            GraphcalError::CyclicDependency {
                name,
                src: src.clone(),
                span: span.into(),
            }
        })
    }

    for dag in tir.dags.values() {
        let deps = &dag.semantic.dependencies;
        check_resolved(
            dag,
            dag.consts.iter().map(|e| (&e.name, e.span)),
            &deps.const_deps,
            src,
        )?;
        check_resolved(
            dag,
            dag.params
                .iter()
                .map(|e| (&e.name, e.span))
                .chain(dag.nodes.iter().map(|e| (&e.name, e.span))),
            &deps.runtime_deps,
            src,
        )?;
    }
    Ok(())
}

fn detect_cross_dag_cycles(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    use crate::dag_id::DagId;

    let mut edges: BTreeMap<DagId, BTreeSet<DagId>> = BTreeMap::new();
    let mut spans: HashMap<DagId, crate::syntax::span::Span> = HashMap::new();
    for (key, dag_tir) in &tir.dags {
        let mut targets = BTreeSet::new();
        collect_dag_call_targets_from_dag(dag_tir, &mut targets);
        edges.insert(key.clone(), targets);
        // HIR retains child declaration spans under canonical identities, so
        // cycle diagnostics never need to recover them from a syntax registry.
        let span = key
            .parent()
            .and_then(|parent| tir.dags.get(&parent))
            .and_then(|parent| parent.child_dag_spans.get(key))
            .copied()
            .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0));
        spans.insert(key.clone(), span);
    }

    let mut visited: HashSet<DagId> = HashSet::new();
    let mut on_stack: HashSet<DagId> = HashSet::new();

    for start in edges.keys() {
        if visited.contains(start) {
            continue;
        }
        let mut work: Vec<DagCycleFrame> = vec![DagCycleFrame::Enter(start.clone())];
        while let Some(frame) = work.pop() {
            match frame {
                DagCycleFrame::Enter(key) => {
                    if visited.contains(&key) {
                        continue;
                    }
                    if on_stack.contains(&key) {
                        let span = spans
                            .get(&key)
                            .copied()
                            .unwrap_or_else(|| crate::syntax::span::Span::new(0, 0));
                        return Err(GraphcalError::CyclicDependency {
                            name: key.to_string(),
                            src: src.clone(),
                            span: span.into(),
                        });
                    }
                    on_stack.insert(key.clone());
                    work.push(DagCycleFrame::Leave(key.clone()));
                    if let Some(targets) = edges.get(&key) {
                        for t in targets {
                            if edges.contains_key(t) {
                                work.push(DagCycleFrame::Enter(t.clone()));
                            }
                        }
                    }
                }
                DagCycleFrame::Leave(key) => {
                    on_stack.remove(&key);
                    visited.insert(key);
                }
            }
        }
    }

    Ok(())
}

use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::ResolvedStructTypeName;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::dimension::Dimension;
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::resolve_types::{ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart};
use crate::syntax::index_name::{IndexName, IndexVariantName};
use crate::syntax::module_name::ScopedName;
use crate::syntax::type_name::StructTypeName;

use crate::registry::builtins::builtin_functions;
use crate::registry::error::GraphcalError;
use crate::registry::time_scale::TimeScale;
use crate::registry::types::Registry;
use crate::tir::typed::{NatPolyForm, NatRangeIndexIdentity};

pub(crate) use helpers::{expect_scalar, format_inferred_type};

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
mod plot;
#[cfg(test)]
mod tests;

pub use crate::registry::declared_type::DeclaredType;

/// Index identity carried by inferred collection/label types.
///
/// Declared indexes compare by owner-qualified [`IndexTypeRef`]. Nat-range
/// indexes additionally carry their normalized Nat form so generic ranges such
/// as `range(N + 1)` are not encoded in or compared through synthetic strings.
#[derive(Debug, Clone, Eq)]
pub struct InferredIndex {
    reference: IndexTypeRef,
}

impl InferredIndex {
    #[must_use]
    pub fn with_owner(owner: crate::dag_id::DagId, name: IndexName) -> Self {
        Self::from_ref(IndexTypeRef::with_owner(owner, name))
    }

    #[must_use]
    pub fn from_resolved(resolved: ResolvedIndexName) -> Self {
        Self {
            reference: IndexTypeRef::from_resolved(resolved),
        }
    }

    #[must_use]
    pub const fn from_ref(reference: IndexTypeRef) -> Self {
        Self { reference }
    }

    /// Create an inferred Nat range index from a validated Nat-range identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the identity cannot be converted to an index type reference.
    pub fn from_nat_range_identity(
        identity: &NatRangeIndexIdentity,
    ) -> Result<Self, crate::registry::types::NatRangeIndexError> {
        Ok(Self {
            reference: identity.to_index_type_ref()?,
        })
    }

    /// Create an inferred Nat range index from a normalized Nat form.
    ///
    /// # Errors
    ///
    /// Returns an error when the form is a concrete invalid Nat range size.
    pub fn from_nat_range_form(
        form: NatPolyForm,
    ) -> Result<Self, crate::registry::types::NatRangeIndexError> {
        Self::from_nat_range_identity(&NatRangeIndexIdentity::try_from_form(form)?)
    }

    #[must_use]
    pub const fn type_ref(&self) -> &IndexTypeRef {
        &self.reference
    }

    #[must_use]
    pub fn name(&self) -> IndexName {
        self.reference.display_name()
    }

    #[must_use]
    pub const fn declared_resolved(&self) -> Option<&ResolvedIndexName> {
        self.reference.declared_resolved()
    }

    #[must_use]
    pub const fn concrete_nat_range(&self) -> Option<crate::registry::types::NatRangeIndex> {
        self.reference.nat_range()
    }

    #[must_use]
    pub fn nat_range_form(&self) -> Option<NatPolyForm> {
        self.reference.nat_range_form()
    }

    #[must_use]
    pub fn matches_resolved(&self, expected: &ResolvedIndexName) -> bool {
        self.declared_resolved() == Some(expected)
    }

    #[must_use]
    pub fn matches_ref(&self, expected: &IndexTypeRef) -> bool {
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
pub struct InferredStructType {
    reference: StructTypeRef,
}

impl InferredStructType {
    #[must_use]
    pub fn with_owner(owner: crate::dag_id::DagId, name: StructTypeName) -> Self {
        Self {
            reference: StructTypeRef::with_owner(owner, name),
        }
    }

    #[must_use]
    pub fn from_resolved(resolved: ResolvedStructTypeName) -> Self {
        Self {
            reference: StructTypeRef::from_resolved(resolved),
        }
    }

    #[must_use]
    pub const fn from_ref(reference: StructTypeRef) -> Self {
        Self { reference }
    }

    #[must_use]
    pub const fn type_ref(&self) -> &StructTypeRef {
        &self.reference
    }

    #[must_use]
    pub const fn name(&self) -> &StructTypeName {
        self.reference.name()
    }

    #[must_use]
    pub const fn resolved(&self) -> &ResolvedStructTypeName {
        self.reference.resolved()
    }

    #[must_use]
    pub fn matches_resolved(&self, expected: &ResolvedStructTypeName) -> bool {
        self.resolved() == expected
    }

    #[must_use]
    pub fn matches_ref(&self, expected: &StructTypeRef) -> bool {
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

/// The inferred type of an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredType {
    Scalar(Dimension),
    /// A scalar value produced by iterating a declared range index.
    ///
    /// Range labels can participate in scalar arithmetic, but retain the
    /// originating index identity so `@series[t]` can prove that `t` belongs
    /// to the same range axis as `series` instead of reading positionally.
    RangeIndexLabel {
        index: InferredIndex,
        dimension: Dimension,
    },
    Bool,
    Int,
    /// A bounded natural number `Fin(N)`: the type of loop variables over `range(N)`.
    ///
    /// A value of type `Fin(N)` satisfies `0 <= value < N`. This enables compile-time
    /// bounds checking: `v[i]` is valid when `i : Fin(N)` and `v : T[M]` with `N <= M`.
    ///
    /// `Fin(N)` is not a user-declarable type — it only arises as the type of loop
    /// variables in `for i: range(N) { ... }`.
    Fin(NatPolyForm),
    /// A datetime instant in a specific time scale.
    Datetime(TimeScale),
    /// A named index identity in an index-only position.
    ///
    /// This is used for named-index loop variables and `Index` generic
    /// arguments. It is intentionally not a Graphcal value type.
    NamedIndex(InferredIndex),
    /// A struct type, optionally with concrete type arguments for generic structs.
    Struct(InferredStructType, Vec<Self>),
    Indexed {
        element: Box<Self>,
        index: InferredIndex,
    },
}

impl InferredType {
    /// Returns `true` if this type is `Int` or `Fin(N)` (integer-like).
    #[must_use]
    pub const fn is_int_like(&self) -> bool {
        matches!(self, Self::Int | Self::Fin(_))
    }

    #[must_use]
    pub const fn scalar_dimension(&self) -> Option<&Dimension> {
        match self {
            Self::Scalar(dimension) | Self::RangeIndexLabel { dimension, .. } => Some(dimension),
            _ => None,
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
    declared_types: &'a HashMap<ScopedName, DeclaredType>,
    dag: Option<&'a crate::tir::typed::DagTIR>,
    tir: &'a crate::tir::typed::TIR,
    registry: &'a Registry,
    builtin_fns: &'a HashMap<&'a str, crate::registry::builtins::BuiltinFunction>,
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
    /// Look up the module-aware HIR expression for a local declaration.
    fn hir_expr_for_decl(
        &self,
        name: &crate::syntax::module_name::ScopedName,
    ) -> Option<&crate::hir::Expr> {
        let dag = self.dag?;
        let key = dag.resolved_decl_key_for_local(name)?;
        dag.semantic
            .expressions
            .consts
            .get(&key)
            .or_else(|| dag.semantic.expressions.runtime_expr(&key))
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
        let key =
            dag.resolved_decl_key_for_local(name)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!("semantic declaration key missing for assertion `{name}`"),
                    src: self.src.clone(),
                    span: span.into(),
                })?;
        dag.semantic
            .expressions
            .asserts
            .get(&key)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!("semantic HIR body missing for assertion `{name}`"),
                src: self.src.clone(),
                span: span.into(),
            })
    }

    /// Infer the type of a module-aware HIR expression using this context's bindings.
    fn infer_hir(&self, expr: &crate::hir::Expr) -> Result<InferredType, GraphcalError> {
        let dag = self.dag.ok_or_else(|| GraphcalError::InternalError {
            message: "HIR assertion inference requires semantic DAG context".to_string(),
            src: self.src.clone(),
            span: expr.span.into(),
        })?;
        infer::hir::infer_hir_type_with_owner(
            expr,
            None,
            self.declared_types,
            dag,
            self.tir,
            self.registry,
            self.builtin_fns,
            self.src,
        )
    }
}

/// Check that a declaration's expression type matches its declared type annotation.
fn check_decl_expr_type(
    ctx: &DimCheckContext<'_>,
    name: &crate::syntax::module_name::ScopedName,
    type_ann_span: &crate::syntax::span::Span,
) -> Result<(), GraphcalError> {
    let declared = ctx
        .declared_types
        .get(name)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("no declared type recorded for `{name}`"),
            src: ctx.src.clone(),
            span: (*type_ann_span).into(),
        })?;
    let dag = ctx.dag.ok_or_else(|| GraphcalError::InternalError {
        message: format!("semantic DAG missing while checking `{name}`"),
        src: ctx.src.clone(),
        span: (*type_ann_span).into(),
    })?;
    let hir_expr = ctx
        .hir_expr_for_decl(name)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("semantic HIR expression missing for declaration `{name}`"),
            src: ctx.src.clone(),
            span: (*type_ann_span).into(),
        })?;
    let inferred = infer::hir::infer_hir_type_with_owner(
        hir_expr,
        Some(name.member()),
        ctx.declared_types,
        dag,
        ctx.tir,
        ctx.registry,
        ctx.builtin_fns,
        ctx.src,
    )?;
    let matches = ctx
        .dag
        .and_then(|dag| dag.resolved_decl_types.get(name))
        .map_or_else(
            || types_match(declared, &inferred),
            |resolved| resolved_type_matches_inferred(resolved, &inferred),
        );
    if !matches {
        return Err(GraphcalError::DimensionMismatchInAnnotation {
            declared: format_declared_type(declared, ctx.registry),
            inferred: format_inferred_type(&inferred, ctx.registry),
            src: ctx.src.clone(),
            span: (*type_ann_span).into(),
        });
    }
    check_ineffective_conversions(hir_expr, true, ctx.src)?;
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
        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                check_ineffective_conversions(&binding.value, display_position, src)?;
            }
            Ok(())
        }
        ExprKind::Error
        | ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::TypeSystemRef(_)
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::UnitLiteral { .. }
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
    body: &crate::hir::AssertBody,
    span: crate::syntax::span::Span,
) -> Result<AssertionIndexShape, GraphcalError> {
    let registry = ctx.registry;
    let src = ctx.src;
    match body {
        crate::hir::AssertBody::Expr(body_expr) => {
            let inferred = ctx.infer_hir(body_expr)?;
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
            is_relative,
        } => {
            let actual_type = ctx.infer_hir(actual)?;
            let expected_type = ctx.infer_hir(expected)?;
            let tolerance_type = ctx.infer_hir(tolerance)?;

            // Element-wise broadcasting (#809): the assertion's index shape
            // comes from `actual`; `expected` and `tolerance` are each scalar
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

            let actual_dim = expect_scalar(actual_elem, registry, src, actual.span)?;
            let expected_dim = expect_scalar(expected_elem, registry, src, expected.span)?;
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

            let tolerance_ok = if *is_relative {
                tolerance_elem.is_int_like()
                    || tolerance_elem
                        .scalar_dimension()
                        .is_some_and(Dimension::is_dimensionless)
            } else {
                let tolerance_dim = expect_scalar(tolerance_elem, registry, src, tolerance.span)?;
                tolerance_dim == actual_dim
            };
            if !tolerance_ok {
                let (expected_str, help_str) = if *is_relative {
                    (
                        "Dimensionless".to_string(),
                        "relative tolerance (%) must be dimensionless".to_string(),
                    )
                } else {
                    (
                        registry.dimensions.format_dimension(&actual_dim),
                        "absolute tolerance must have the same dimension as actual/expected"
                            .to_string(),
                    )
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: format_inferred_type(&tolerance_type, registry),
                    help: help_str,
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
    registry: &Registry,
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
        crate::hir::ExprKind::UnitLiteral { value, .. } => Some(*value),
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
) -> Vec<(Option<IndexTypeRef>, IndexVariantName)> {
    key.iter()
        .map(|part| (part.named_index().cloned(), part.variant()))
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
            ExpectedFailKeyPart::RangeStep { step, span } => {
                let Some(range) = expected_axis.type_ref().nat_range_ref() else {
                    return Err(GraphcalError::ExpectedFailKeyIndexMismatch {
                        expected: expected_axis.name().to_string(),
                        found: part.display(),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                };
                // Bound-check `#N` against a statically known range size.
                // Symbolic sizes are checked nowhere earlier; an out-of-range
                // step there can never match at runtime, which surfaces as an
                // "unexpected pass" — acceptable for the symbolic case.
                if let Some(concrete) = range.concrete_index()
                    && *step >= concrete.size_u64()
                {
                    return Err(GraphcalError::ExpectedFailRangeStepOutOfBounds {
                        step: *step,
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
    assert_span: crate::syntax::span::Span,
) -> Result<(), GraphcalError> {
    match expected_fail {
        ExpectedFail::All if shape.is_indexed() => Err(GraphcalError::ExpectedFailAllOnIndexed {
            src: src.clone(),
            span: assert_span.into(),
        }),
        ExpectedFail::All => Ok(()),
        ExpectedFail::Variants(keys) if !shape.is_indexed() => {
            Err(GraphcalError::ExpectedFailNotIndexed {
                src: src.clone(),
                span: keys
                    .first()
                    .map_or(assert_span, expected_fail_key_span)
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
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    detect_decl_cycles(tir, src)?;
    detect_cross_dag_cycles(tir, src)?;
    let builtin_fns = builtin_functions();

    // Dim-check the file's own DAGs (root + inline children) against the
    // file's shared registry. Dep DAGs merged in by `merge_dep_dag_tirs`
    // were already dim-checked in their own file's pipeline, against
    // their own registry — re-checking them here against the importer's
    // registry would fail on types renamed by include bindings.
    for (id, dag) in &tir.dags {
        if id == &tir.root_dag_id || id.parent().as_ref() == Some(&tir.root_dag_id) {
            check_dimensions_dag(dag, tir, &tir.registry, builtin_fns, src)?;
        }
    }

    // Validate domain constraints on struct/union member fields. The check
    // walks the registry's `TypeDef`s once per file. Types reachable through
    // dep imports were already validated in their defining file's pipeline,
    // so the redundant pass is idempotent. (#450 Position 1+2.)
    let declared_types = tir.build_declared_types(src)?;
    check_no_constraints_on_generic_type_args(tir, src)?;
    check_field_domain_constraint_targets(tir, src)?;
    check_field_domain_constraint_dimensions(
        tir,
        &declared_types,
        &tir.registry,
        builtin_fns,
        src,
    )?;

    Ok(())
}

/// Dim-check a single [`DagTIR`] against the file's shared registry and
/// the full flat dag map.
fn check_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &crate::registry::types::Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let declared_types = dag.build_declared_types(src)?;
    let ctx = DimCheckContext {
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
        let entry_ctx = ctx.for_body(entry.src.resolve(src));
        check_decl_expr_type(&entry_ctx, &entry.name, &entry.type_ann.span)?;
    }
    for entry in &dag.nodes {
        let entry_ctx = ctx.for_body(entry.src.resolve(src));
        check_decl_expr_type(&entry_ctx, &entry.name, &entry.type_ann.span)?;
    }
    for entry in &dag.params {
        let Some(_value_expr) = entry.default_expr.as_ref() else {
            continue;
        };
        let entry_ctx = ctx.for_body(entry.src.resolve(src));
        check_decl_expr_type(&entry_ctx, &entry.name, &entry.type_ann.span)?;
    }

    for entry in &dag.asserts {
        let body_src = entry.src.resolve(src);
        let entry_ctx = ctx.for_body(body_src);
        let body = entry_ctx.hir_assert_body(&entry.name, entry.span)?;
        let shape = check_hir_assert_body(&entry_ctx, body, entry.span)?;
        if let Some(expected_fail) = dag.expected_fail.get(&entry.name) {
            validate_expected_fail(expected_fail, &shape, body_src, entry.span)?;
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

    plot::check_plot_properties_dag(&ctx)?;

    check_domain_constraint_targets_dag(dag, src)?;
    check_domain_constraint_dimensions_dag(dag, &declared_types, tir, registry, builtin_fns, src)?;

    Ok(())
}

/// What a domain bound expression must infer to for a given target type.
enum ExpectedBound {
    /// Bound must be `Scalar(d)`. `Int` is also accepted when `d` is dimensionless.
    Scalar(Dimension),
    /// Bound must be unitless: `Int`, or `Scalar` with the dimensionless dimension.
    Int,
}

/// Check that domain constraint bound expressions have the correct type.
///
/// For each param/node with `(min: ..., max: ...)` constraints whose target type
/// is `Scalar(d)`, `Dimensionless`, or `Int`, infers the type of each bound
/// expression using the regular type checker and verifies it matches:
/// - `Scalar(d)` target: bound must be `Scalar(d)` (or `Int` if `d` is dimensionless).
/// - `Dimensionless` target: bound must be `Scalar(dimensionless)` or `Int`.
/// - `Int` target: bound must be `Int` or `Scalar(dimensionless)` — units forbidden.
///
/// Other targets (e.g., `Bool`) are skipped here and handled by
/// `validate_constraint_target` in `exec_plan` (which raises `InvalidDomainTarget`).
fn check_domain_constraint_dimensions_dag(
    dag: &crate::tir::typed::DagTIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // A merged dependency declaration's domain bounds keep the dependency
    // file's spans, so they are checked against that body's source (#868).
    let decl_iter = dag
        .consts
        .iter()
        .map(|e| (&e.name, &e.src))
        .chain(dag.params.iter().map(|e| (&e.name, &e.src)))
        .chain(dag.nodes.iter().map(|e| (&e.name, &e.src)));

    for (name, body_provenance) in decl_iter {
        let bounds = dag
            .resolved_decl_key_for_local(name)
            .and_then(|key| dag.semantic.domain_bounds.get(&key));
        let Some(bounds) = bounds else {
            continue;
        };
        let body_src = body_provenance.resolve(src);

        let resolved = dag.resolved_decl_types.get(name);
        let base_resolved = resolved.map(strip_indexed);
        let expected = match base_resolved {
            Some(crate::tir::typed::ResolvedTypeExpr::Scalar(dim)) => {
                ExpectedBound::Scalar(dim.clone())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Dimensionless) => {
                ExpectedBound::Scalar(Dimension::dimensionless())
            }
            Some(crate::tir::typed::ResolvedTypeExpr::Int) => ExpectedBound::Int,
            _ => continue,
        };

        for bound in bounds {
            let inferred = infer::hir::infer_hir_type_with_owner(
                &bound.value,
                None,
                declared_types,
                dag,
                tir,
                registry,
                builtin_fns,
                body_src,
            )?;
            check_one_bound(name, bound, &inferred, &expected, registry, body_src)?;
        }
    }

    Ok(())
}

fn check_one_bound(
    name: &crate::syntax::module_name::ScopedName,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Scalar(target_dim) => {
            let ok = match inferred {
                InferredType::Int => target_dim.is_dimensionless(),
                other => other.scalar_dimension() == Some(target_dim),
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = match inferred.scalar_dimension() {
                Some(d) => registry.dimensions.format_dimension(d),
                None => format_inferred_type(inferred, registry),
            };
            Err(GraphcalError::DomainDimensionMismatch {
                name: name.to_string(),
                type_dim: registry.dimensions.format_dimension(target_dim),
                bound_name: bound.kind.to_string(),
                bound_dim: bound_dim_str,
                src: src.clone(),
                span: bound.span.into(),
            })
        }
        ExpectedBound::Int => {
            let ok = match inferred {
                InferredType::Int => true,
                other => other
                    .scalar_dimension()
                    .is_some_and(Dimension::is_dimensionless),
            };
            if ok {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundNotUnitless {
                name: name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}

/// Reject domain constraints on base types that don't accept them.
///
/// Bool, Datetime, Label, and struct/generic types cannot carry numeric
/// `(min: …, max: …)` bounds. The check is a pure function of the resolved
/// declaration type — independent of any bound expression's value — so it
/// belongs in compile-time validation rather than runtime resolution.
fn check_domain_constraint_targets_dag(
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let decl_iter = dag
        .consts
        .iter()
        .map(|e| (&e.name, &e.type_ann, e.span))
        .chain(dag.params.iter().map(|e| (&e.name, &e.type_ann, e.span)))
        .chain(dag.nodes.iter().map(|e| (&e.name, &e.type_ann, e.span)));

    for (name, type_ann, decl_span) in decl_iter {
        if extract_domain_bounds(type_ann).is_empty() {
            continue;
        }
        let Some(resolved) = dag.resolved_decl_types.get(name) else {
            continue;
        };
        let type_kind = match strip_indexed(resolved) {
            crate::tir::typed::ResolvedTypeExpr::Bool => "Bool".to_string(),
            crate::tir::typed::ResolvedTypeExpr::Datetime(_) => "Datetime".to_string(),
            crate::tir::typed::ResolvedTypeExpr::IndexArg(index) => {
                format!("index {}", index.format_for_diagnostic())
            }
            crate::tir::typed::ResolvedTypeExpr::Struct(struct_name, _)
            | crate::tir::typed::ResolvedTypeExpr::GenericStruct {
                name: struct_name, ..
            } => format!("struct `{}`", struct_name.as_str()),
            crate::tir::typed::ResolvedTypeExpr::Scalar(_)
            | crate::tir::typed::ResolvedTypeExpr::Dimensionless
            | crate::tir::typed::ResolvedTypeExpr::Int
            | crate::tir::typed::ResolvedTypeExpr::GenericDimParam(_, _)
            | crate::tir::typed::ResolvedTypeExpr::GenericTypeParam(_, _)
            | crate::tir::typed::ResolvedTypeExpr::GenericDimExpr { .. }
            | crate::tir::typed::ResolvedTypeExpr::Indexed { .. } => continue,
        };
        return Err(GraphcalError::InvalidDomainTarget {
            type_kind,
            src: src.clone(),
            span: decl_span.into(),
        });
    }
    Ok(())
}

/// Extract `DomainBound`s from a `TypeExpr`, handling indexed types.
///
/// For `Velocity(min: 0)[Maneuver]`, the constraints are on the base `Velocity`,
/// not on the outer `Indexed` wrapper.
fn extract_domain_bounds(
    type_ann: &crate::desugar::desugared_ast::TypeExpr,
) -> &[crate::desugar::desugared_ast::DomainBound] {
    if !type_ann.constraints.is_empty() {
        return &type_ann.constraints;
    }
    if let crate::desugar::desugared_ast::TypeExprKind::Indexed { base, .. } = &type_ann.kind {
        return &base.constraints;
    }
    &[]
}

/// Reject domain constraints on struct/union fields whose target type
/// cannot carry numeric `(min: …, max: …)` bounds (Bool, Datetime, Label,
/// nested struct/union). Mirrors [`check_domain_constraint_targets_dag`]
/// for top-level decls.
///
/// Scans every `TypeDef` in the file's registry. Generic-param fields are
/// skipped (we don't know their concrete type at definition time).
fn check_field_domain_constraint_targets(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for type_def in tir.registry.types.all_types() {
        // Iterate over every variant's payload fields — the n-variant
        // model puts payload fields on the union's members.
        let members: &[crate::registry::types::UnionMemberDef] =
            type_def.union_members().unwrap_or(&[]);
        for field in members.iter().flat_map(|m| m.fields.iter()) {
            if extract_domain_bounds(&field.type_ann).is_empty() {
                continue;
            }
            let kind = field_constraint_target_kind(&field.type_ann, &tir.registry);
            if let Some(type_kind) = kind {
                return Err(GraphcalError::InvalidDomainTarget {
                    type_kind,
                    src: src.clone(),
                    span: field.type_ann.span.into(),
                });
            }
        }
    }
    Ok(())
}

/// Classify a field's `TypeExpr` as either constraint-compatible (returns
/// `None`) or constraint-incompatible (returns `Some(kind_str)` describing
/// why it's incompatible). Strips an outer `Indexed` wrapper before
/// classifying — a `Velocity(min: 0)[Maneuver]` field is constraint-
/// compatible because the base `Velocity` is scalar.
fn field_constraint_target_kind(
    type_ann: &crate::desugar::desugared_ast::TypeExpr,
    registry: &Registry,
) -> Option<String> {
    use crate::desugar::desugared_ast::TypeExprKind;
    let base = match &type_ann.kind {
        TypeExprKind::Indexed { base, .. } => base.as_ref(),
        _ => type_ann,
    };
    match &base.kind {
        TypeExprKind::Bool => Some("Bool".to_string()),
        TypeExprKind::Datetime | TypeExprKind::DatetimeApplication { .. } => {
            Some("Datetime".to_string())
        }
        TypeExprKind::TypeApplication { name, .. } => {
            Some(format!("struct `{}`", name.value.display_path()))
        }
        // The outer `Indexed` wrapper was stripped above; a nested indexed
        // type at this depth is unusual but constraint-compatible (the base
        // dim is what carries the constraint).
        TypeExprKind::Dimensionless | TypeExprKind::Int | TypeExprKind::Indexed { .. } => None,
        TypeExprKind::DimExpr(dim_expr) => {
            // A bare single-name DimExpr could be a struct, an index name, or a
            // dimension. The registry distinguishes them: dim → constraint-
            // compatible scalar; struct → reject; index → reject as an index.
            if dim_expr.terms.len() == 1
                && dim_expr.terms[0].term.power.is_none()
                && let Some(item) = dim_expr.terms.first()
            {
                let Some(name) = item
                    .term
                    .name
                    .value
                    .as_bare()
                    .map(super::super::syntax::names::NameAtom::as_str)
                else {
                    // Qualified type-level references are rejected by type
                    // resolution; skip this compatibility classifier here.
                    return None;
                };
                if registry.dimensions.get_dimension(name).is_some() {
                    None
                } else if registry.types.get_type(name).is_some() {
                    Some(format!("struct `{name}`"))
                } else if registry.indexes.get_index(name).is_some() {
                    Some(format!("index `{name}`"))
                } else {
                    // Generic dim param or unknown name — skip; an unknown name
                    // would already error in type resolution.
                    None
                }
            } else {
                // Compound dim expression like `Length / Time` → constraint-
                // compatible scalar.
                None
            }
        }
    }
}

/// Check that domain bound expressions on struct/union fields have the
/// correct type. Mirrors [`check_domain_constraint_dimensions_dag`] for
/// top-level decls.
///
/// Field bounds live in each DAG's semantic type defs (lowered to HIR at
/// type-resolution time); the same owner-qualified field can be referenced
/// from several DAGs, so a seen-set dedupes the checks.
fn check_field_domain_constraint_dimensions(
    tir: &crate::tir::typed::TIR,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    registry: &Registry,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut seen: std::collections::HashSet<&crate::tir::typed::ResolvedStructFieldTypeKey> =
        std::collections::HashSet::new();
    for (id, dag) in &tir.dags {
        if id != &tir.root_dag_id && id.parent().as_ref() != Some(&tir.root_dag_id) {
            continue;
        }
        for (key, bounds) in &dag.semantic.type_defs.field_bounds {
            if !seen.insert(key) {
                continue;
            }
            let Some(type_def) = dag.semantic.type_defs.struct_types.get(&key.owning_type) else {
                continue;
            };
            let Some((variant, field)) = type_def.union_members().and_then(|members| {
                members
                    .iter()
                    .flat_map(|m| m.fields.iter().map(move |f| (m, f)))
                    .find(|(m, f)| m.name == key.constructor && f.name == key.field)
            }) else {
                continue;
            };
            let Some(expected) = field_expected_bound(&field.type_ann, registry, src)? else {
                continue;
            };
            // For a single-variant collision (record-shape) the display
            // name is `Type.field`; for a true multi-variant union it's
            // `Type.Variant.field` so diagnostics disambiguate which
            // constructor a violating bound belongs to.
            let display_name = if variant.name.as_str() == type_def.name.as_str() {
                format!("{}.{}", type_def.name, field.name)
            } else {
                format!("{}.{}.{}", type_def.name, variant.name, field.name)
            };
            for bound in bounds {
                let inferred = infer::hir::infer_hir_type_with_owner(
                    &bound.value,
                    None,
                    declared_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                check_one_bound_with_display_name(
                    &display_name,
                    bound,
                    &inferred,
                    &expected,
                    registry,
                    src,
                )?;
            }
        }
    }
    Ok(())
}

/// Compute the [`ExpectedBound`] for a struct field's `TypeExpr`. Returns
/// `Ok(None)` when the field's base type isn't `Scalar`/`Dimensionless`/`Int`
/// (in which case the target check has already rejected it, or it's a
/// generic param to be checked at instantiation), and `Err` if dimension
/// arithmetic overflows.
fn field_expected_bound(
    type_ann: &crate::desugar::desugared_ast::TypeExpr,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<ExpectedBound>, GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    let base = match &type_ann.kind {
        TypeExprKind::Indexed { base, .. } => base.as_ref(),
        _ => type_ann,
    };
    match &base.kind {
        TypeExprKind::Dimensionless => Ok(Some(ExpectedBound::Scalar(Dimension::dimensionless()))),
        TypeExprKind::Int => Ok(Some(ExpectedBound::Int)),
        TypeExprKind::DimExpr(_) => Ok(registry
            .dimensions
            .resolve_type_expr(base)
            .map_err(|_| GraphcalError::DimensionOverflow {
                src: src.clone(),
                span: base.span.into(),
            })?
            .map(ExpectedBound::Scalar)),
        _ => Ok(None),
    }
}

/// Variant of [`check_one_bound`] that takes a pre-formatted display name
/// for the constrained target (e.g. `"SatelliteSpec.mass"`) so a single
/// helper can serve both top-level decls and struct fields.
fn check_one_bound_with_display_name(
    display_name: &str,
    bound: &crate::tir::typed::ResolvedDomainBound,
    inferred: &InferredType,
    expected: &ExpectedBound,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match expected {
        ExpectedBound::Scalar(target_dim) => {
            let ok = match inferred {
                InferredType::Int => target_dim.is_dimensionless(),
                other => other.scalar_dimension() == Some(target_dim),
            };
            if ok {
                return Ok(());
            }
            let bound_dim_str = match inferred.scalar_dimension() {
                Some(d) => registry.dimensions.format_dimension(d),
                None => format_inferred_type(inferred, registry),
            };
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
            let ok = match inferred {
                InferredType::Int => true,
                other => other
                    .scalar_dimension()
                    .is_some_and(Dimension::is_dimensionless),
            };
            if ok {
                return Ok(());
            }
            Err(GraphcalError::IntDomainBoundNotUnitless {
                name: display_name.to_string(),
                bound_name: bound.kind.to_string(),
                bound_type: format_inferred_type(inferred, registry),
                src: src.clone(),
                span: bound.span.into(),
            })
        }
    }
}

/// Reject domain constraints on generic type-application arguments.
///
/// Generic args are erased at runtime, so a constraint on `D` in
/// `Vec3<Length(min: 0.0 m)>` has no enforcement site and unclear
/// semantics. Issue #450 Position 4: surface a clear compile-time error
/// directing the user to put the constraint on the field instead.
///
/// Walks every `TypeExpr` reachable through declarations and type-defs
/// in the file. (Type-args themselves can be `TypeApplication`s nested
/// inside other applications, so the walk recurses.)
fn check_no_constraints_on_generic_type_args(
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let walk = |type_expr: &crate::desugar::desugared_ast::TypeExpr| -> Result<(), GraphcalError> {
        check_type_expr_for_generic_arg_constraints(type_expr, src)
    };
    for (id, dag) in &tir.dags {
        if id != &tir.root_dag_id && id.parent().as_ref() != Some(&tir.root_dag_id) {
            continue;
        }
        for entry in &dag.consts {
            walk(&entry.type_ann)?;
        }
        for entry in &dag.params {
            walk(&entry.type_ann)?;
        }
        for entry in &dag.nodes {
            walk(&entry.type_ann)?;
        }
    }
    for type_def in tir.registry.types.all_types() {
        for field in type_def.fields() {
            walk(&field.type_ann)?;
        }
    }
    Ok(())
}

/// Recurse through a `TypeExpr` and reject any `DomainBound` found on a
/// `TypeApplication` argument. The outermost `TypeExpr` may itself carry
/// constraints (the legitimate placement); only constraints under a
/// `TypeApplication.type_args` slot are rejected.
fn check_type_expr_for_generic_arg_constraints(
    type_expr: &crate::desugar::desugared_ast::TypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::TypeExprKind;
    match &type_expr.kind {
        TypeExprKind::Indexed { base, .. } => {
            check_type_expr_for_generic_arg_constraints(base, src)
        }
        TypeExprKind::TypeApplication { type_args, .. }
        | TypeExprKind::DatetimeApplication { type_args } => {
            for arg in type_args {
                if let Some(bound) = arg.constraints.first() {
                    return Err(GraphcalError::GenericTypeArgDomainConstraint {
                        src: src.clone(),
                        span: bound.span.into(),
                    });
                }
                // Recurse so nested generics are checked too.
                check_type_expr_for_generic_arg_constraints(arg, src)?;
            }
            Ok(())
        }
        TypeExprKind::Dimensionless
        | TypeExprKind::Bool
        | TypeExprKind::Int
        | TypeExprKind::Datetime
        | TypeExprKind::DimExpr(_) => Ok(()),
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

/// Check that an applied override has the correct dimension for the given param.
///
/// Overrides are spliced into the IR as the target param's default expression
/// before type resolution, so the override's HIR already lives in the root
/// DAG's semantic expressions; this checks that stored HIR against the param's
/// declared type.
///
/// # Errors
///
/// Returns a [`GraphcalError::DimensionMismatch`] if the override's inferred
/// dimension does not match the declared type of the param.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn check_override_dimension(
    param_name: &str,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    tir: &crate::tir::typed::TIR,
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let builtin_fns = builtin_functions();

    // Override targets are addressed by their bare param name, which is always
    // a top-level local in the file being overridden.
    let param_key = ScopedName::local(param_name);
    let declared =
        declared_types
            .get(&param_key)
            .ok_or_else(|| GraphcalError::OverrideUnknownParam {
                name: crate::syntax::decl_name::DeclName::expect_valid(param_name.to_string()),
            })?;
    let dag = tir.root();
    let hir_expr = dag
        .resolved_decl_key_for_local(&param_key)
        .and_then(|key| dag.semantic.expressions.param_defaults.get(&key))
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("override for `{param_name}` was not applied to the root DAG"),
            src: src.clone(),
            span: crate::syntax::span::Span::new(0, 0).into(),
        })?;
    let inferred = infer::hir::infer_hir_type_with_owner(
        hir_expr,
        Some(param_name),
        declared_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;

    if !types_match(declared, &inferred) {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_declared_type(declared, registry),
            found: format_inferred_type(&inferred, registry),
            help: format!(
                "override for `{param_name}` must have dimension {}",
                format_declared_type(declared, registry)
            ),
            src: src.clone(),
            span: hir_expr.span.into(),
        });
    }
    Ok(())
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

/// Collect inline dag call targets from a compiled DAG's semantic body.
fn collect_dag_call_targets_from_dag(
    dag: &crate::tir::typed::DagTIR,
    out: &mut std::collections::BTreeSet<crate::dag_id::DagId>,
) {
    out.extend(
        dag.semantic
            .inline_dag_refs
            .calls
            .values()
            .map(|call| call.target.clone()),
    );
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

    fn local_resolved_decl_key(
        dag: &crate::tir::typed::DagTIR,
        name: &ScopedName,
        span: crate::syntax::span::Span,
        src: &NamedSource<Arc<String>>,
    ) -> Result<ResolvedDeclKey, GraphcalError> {
        dag.resolved_decl_key_for_local(name)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "semantic dependency metadata contains no local canonical key for declaration `{name}`"
                ),
                src: src.clone(),
                span: span.into(),
            })
    }

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
            let key = local_resolved_decl_key(dag, name, span, src)?;
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
        // Best-effort span: for inline children of this file the parent's
        // registry entry has the AST span; cross-file merged dags fall
        // back to a zero span (no AST in the importer).
        let parent = key.parent();
        let span = if parent.as_ref() == Some(&tir.root_dag_id) {
            tir.registry
                .dags
                .get(key.name())
                .map_or_else(|| crate::syntax::span::Span::new(0, 0), |d| d.name.span)
        } else {
            crate::syntax::span::Span::new(0, 0)
        };
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

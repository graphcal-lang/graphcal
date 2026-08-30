//! HIR-backed expression type inference.
//!
//! This is the semantic expression type checker for module-aware declaration and
//! assertion bodies. It consumes HIR references directly: canonical declaration,
//! index, constructor, DAG-call refs, lexical `LocalId`s, and typed built-in
//! function variants. It must not fall back to source/syntax-AST inference.

use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::type_name::{ResolvedConstructorName, ResolvedStructTypeName};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use miette::NamedSource;

use crate::builtin::{AggregationFn, BuiltinFnName};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::{BaseDimId, Dimension};
use crate::hir::{self, ConstRef, FunctionRef, NominalConstructor, NominalTypeDef};
use crate::nat::NatOverflowError;
use crate::registry::declared_type::IndexTypeRef;
use crate::registry::error::GraphcalError;
use crate::registry::types::{IndexCardinality, SemanticRegistry, TypeGenericConstraint};
use crate::syntax::ast::UnaryOp;
use crate::syntax::index_name::{IndexEntryKey, ResolvedIndexVariant};
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::non_empty::NonEmpty;
use crate::syntax::span::Span;
use crate::syntax::type_name::{FieldName, GenericParamName};
use crate::tir::materialized_shape::{
    MaterializedExpressionKey, MaterializedShape, MaterializedShapeError,
};
use crate::tir::typed::{
    NatPolyForm, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex,
    ResolvedTypeExpr,
};

use super::super::builtins::infer_fn_dim;
use super::super::helpers::{
    expect_quantity, format_distinct_inferred_types, format_inferred_type,
    resolved_type_matches_inferred, struct_type_def_for_inferred,
};
use super::super::{
    DeclaredType, InferredGenericArg, InferredIndex, InferredStructType, InferredType,
    NominalOverrideIdentity,
};
use super::builtin_call::{
    BuiltinTypeRule, DatetimeConstructorFn, TypeConversionFn, type_rule_for_builtin,
};
use super::linear_algebra::{LinearAlgebraTypeError, infer_linear_algebra_type};

#[derive(Clone, Default)]
struct NominalDependencyCollector {
    dependencies: Rc<RefCell<HashSet<NominalOverrideIdentity>>>,
}

impl NominalDependencyCollector {
    fn record_type(&self, identity: &ResolvedStructTypeName) {
        self.dependencies
            .borrow_mut()
            .insert(NominalOverrideIdentity::Type(identity.clone()));
    }

    fn record_index(&self, identity: &IndexTypeRef) {
        if let Some(resolved) = identity.declared_resolved() {
            self.dependencies
                .borrow_mut()
                .insert(NominalOverrideIdentity::Index(resolved.clone()));
        }
    }

    fn snapshot(&self) -> HashSet<NominalOverrideIdentity> {
        self.dependencies.borrow().clone()
    }
}

#[derive(Clone, Default)]
enum NominalDependencyTracking {
    #[default]
    Disabled,
    Collect(NominalDependencyCollector),
}

#[derive(Clone, Default)]
pub(in crate::tir::dim_check) struct MaterializedShapeCollector {
    shapes: Rc<RefCell<HashMap<MaterializedExpressionKey, MaterializedShape>>>,
}

impl MaterializedShapeCollector {
    pub(in crate::tir::dim_check) fn snapshot(
        &self,
    ) -> HashMap<MaterializedExpressionKey, MaterializedShape> {
        self.shapes.borrow().clone()
    }

    fn record(
        &self,
        owner: Option<&ResolvedDeclName>,
        expr: &hir::Expr,
        inferred: &InferredType,
        tir: &crate::tir::typed::TIR,
        src: &NamedSource<Arc<String>>,
    ) -> Result<(), GraphcalError> {
        let Some(shape) = validate_inferred_materialized_shape(inferred, tir, src, expr.span)?
        else {
            return Ok(());
        };
        let Some(owner) = owner else {
            return Ok(());
        };
        let key = MaterializedExpressionKey::new(owner.clone(), expr.span);
        match self.shapes.borrow_mut().entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(shape);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &shape => Ok(()),
            std::collections::hash_map::Entry::Occupied(_) => Err(GraphcalError::InternalError {
                message: format!(
                    "materialized expression in `{owner}` inferred with inconsistent concrete shapes"
                ),
                src: src.clone(),
                span: expr.span.into(),
            }),
        }
    }
}

pub(in crate::tir::dim_check) fn validate_inferred_materialized_shape(
    inferred: &InferredType,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Option<MaterializedShape>, GraphcalError> {
    let mut axes = Vec::new();
    let mut current = inferred;
    while let InferredType::Indexed { element, index } = current {
        let Some(cardinality) = concrete_index_cardinality(index, tir, src, span)? else {
            return Ok(None);
        };
        axes.push(cardinality);
        current = element;
    }
    let Ok(axes) = NonEmpty::try_from_vec(axes) else {
        return Ok(None);
    };
    MaterializedShape::try_new(axes).map(Some).map_err(
        |MaterializedShapeError::ExceedsLimit { maximum }| {
            GraphcalError::MaterializedShapeTooLarge {
                maximum,
                src: src.clone(),
                span: span.into(),
            }
        },
    )
}

fn concrete_index_cardinality(
    index: &InferredIndex,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Option<IndexCardinality>, GraphcalError> {
    if let Some(index) = index.concrete_finite_index() {
        return Ok(Some(index.cardinality()));
    }
    let Some(resolved) = index.declared_resolved() else {
        return Ok(None);
    };
    tir.declared_index_def(resolved)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("concrete index definition `{resolved}` is missing from checked TIR"),
            src: src.clone(),
            span: span.into(),
        })
        .map(crate::registry::types::IndexDef::concrete_cardinality)
}

impl NominalDependencyTracking {
    fn record_type(&self, identity: &ResolvedStructTypeName) {
        match self {
            Self::Disabled => {}
            Self::Collect(collector) => collector.record_type(identity),
        }
    }

    fn record_index(&self, identity: &IndexTypeRef) {
        match self {
            Self::Disabled => {}
            Self::Collect(collector) => collector.record_index(identity),
        }
    }
}

/// Lexical inference environment plus operation-scoped control state.
///
/// Every recursive inference path already carries the local environment. Keeping
/// cancellation, generic substitutions, and nominal-use tracking in the same
/// typed context prevents nested helpers from silently dropping any policy.
struct HirInferenceControl {
    cancellation: crate::cancellation::CancellationToken,
    generic_substitutions: Option<ConcreteGenericSubstitutions>,
    nominal_dependencies: NominalDependencyTracking,
    materialized_shapes: Option<(MaterializedShapeCollector, Option<ResolvedDeclName>)>,
}

struct HirLocalTypes<'a> {
    bindings: hir::LocalEnv<'a, InferredType>,
    control: Rc<HirInferenceControl>,
}

impl HirLocalTypes<'_> {
    fn root(
        cancellation: &crate::cancellation::CancellationToken,
        generic_substitutions: Option<ConcreteGenericSubstitutions>,
    ) -> Self {
        Self::root_with_tracking(
            cancellation,
            generic_substitutions,
            NominalDependencyTracking::Disabled,
            None,
        )
    }

    fn collecting_root(
        cancellation: &crate::cancellation::CancellationToken,
    ) -> (Self, NominalDependencyCollector) {
        let collector = NominalDependencyCollector::default();
        let locals = Self::root_with_tracking(
            cancellation,
            None,
            NominalDependencyTracking::Collect(collector.clone()),
            None,
        );
        (locals, collector)
    }

    fn root_with_materialized_shapes(
        cancellation: &crate::cancellation::CancellationToken,
        collector: MaterializedShapeCollector,
        owner: Option<ResolvedDeclName>,
    ) -> Self {
        Self::root_with_tracking(
            cancellation,
            None,
            NominalDependencyTracking::Disabled,
            Some((collector, owner)),
        )
    }

    fn root_with_tracking(
        cancellation: &crate::cancellation::CancellationToken,
        generic_substitutions: Option<ConcreteGenericSubstitutions>,
        nominal_dependencies: NominalDependencyTracking,
        materialized_shapes: Option<(MaterializedShapeCollector, Option<ResolvedDeclName>)>,
    ) -> Self {
        Self {
            bindings: hir::LocalEnv::root(),
            control: Rc::new(HirInferenceControl {
                cancellation: cancellation.clone(),
                generic_substitutions,
                nominal_dependencies,
                materialized_shapes,
            }),
        }
    }

    fn checkpoint(&self) -> Result<(), GraphcalError> {
        self.control
            .cancellation
            .checkpoint()
            .map_err(GraphcalError::from)
    }

    fn generic_substitutions(&self) -> Option<&ConcreteGenericSubstitutions> {
        self.control.generic_substitutions.as_ref()
    }

    fn get(&self, id: hir::LocalId) -> Option<&InferredType> {
        self.bindings.get(id)
    }

    fn bind(&mut self, id: hir::LocalId, value: InferredType) {
        self.bindings.bind(id, value);
    }

    fn child(&self, bindings: Vec<(hir::LocalId, InferredType)>) -> HirLocalTypes<'_> {
        HirLocalTypes {
            bindings: self.bindings.child(bindings),
            control: Rc::clone(&self.control),
        }
    }
}

type ResolvedDeclKey = ResolvedDeclName;

#[derive(Debug, Clone, Copy)]
enum TypeNominalUse<'a> {
    Field(&'a FieldName),
    Constructor(&'a ResolvedConstructorName),
    TypeArgument,
}

fn check_type_override_dependency(
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    owner_decl: Option<&ResolvedDeclName>,
    actual: &ResolvedStructTypeName,
    nominal_use: TypeNominalUse<'_>,
) -> Result<(), GraphcalError> {
    local_types.control.nominal_dependencies.record_type(actual);
    let Some(owner_decl) = owner_decl else {
        return Ok(());
    };
    let Some(reconciliations) = dag.semantic.override_reconciliations.get(owner_decl) else {
        return Ok(());
    };

    for reconciliation in reconciliations {
        for target in &reconciliation.targets {
            let crate::tir::typed::ResolvedOverrideTarget::Type {
                overridden,
                source,
                replacement,
            } = target
            else {
                continue;
            };
            if actual != source && actual != replacement {
                continue;
            }
            let detail = match nominal_use {
                TypeNominalUse::Field(field) => {
                    format!("field `{field}` of type `{overridden}`")
                }
                TypeNominalUse::Constructor(constructor) => format!(
                    "constructor `{}` of type `{overridden}`",
                    constructor.as_str()
                ),
                TypeNominalUse::TypeArgument => format!("type `{overridden}`"),
            };
            return Err(GraphcalError::IncludeMustReconcileOverride {
                overridden: overridden.to_string(),
                overridden_kind: "type".to_string(),
                orphan_decl: reconciliation.orphan_decl.to_string(),
                detail,
                src: reconciliation.src.clone(),
                span: reconciliation.include_span.into(),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum IndexNominalUse<'a> {
    Label(&'a crate::syntax::index_name::IndexVariantName),
    TypeArgument,
}

fn check_index_override_dependency(
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    owner_decl: Option<&ResolvedDeclName>,
    actual: &IndexTypeRef,
    nominal_use: IndexNominalUse<'_>,
) -> Result<(), GraphcalError> {
    local_types
        .control
        .nominal_dependencies
        .record_index(actual);
    let Some(owner_decl) = owner_decl else {
        return Ok(());
    };
    let Some(reconciliations) = dag.semantic.override_reconciliations.get(owner_decl) else {
        return Ok(());
    };

    for reconciliation in reconciliations {
        for target in &reconciliation.targets {
            let crate::tir::typed::ResolvedOverrideTarget::Index {
                overridden,
                source,
                replacement,
            } = target
            else {
                continue;
            };
            let source_matches = actual.declared_resolved() == Some(source);
            if !source_matches && !replacement.matches_ref(actual) {
                continue;
            }
            let detail = match nominal_use {
                IndexNominalUse::Label(variant) => {
                    format!("index label `{overridden}#{variant}`")
                }
                IndexNominalUse::TypeArgument => format!("index `{overridden}`"),
            };
            return Err(GraphcalError::IncludeMustReconcileOverride {
                overridden: overridden.to_string(),
                overridden_kind: "index".to_string(),
                orphan_decl: reconciliation.orphan_decl.to_string(),
                detail,
                src: reconciliation.src.clone(),
                span: reconciliation.include_span.into(),
            });
        }
    }
    Ok(())
}

fn check_hir_index_ref_override_dependency(
    index: &hir::IndexRef,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    owner_decl: Option<&ResolvedDeclName>,
) -> Result<(), GraphcalError> {
    let actual = match index {
        hir::IndexRef::Concrete(index) => IndexTypeRef::from_resolved(index.value.clone()),
        hir::IndexRef::Finite(cardinality) => {
            let Ok(form) = hir_nat_to_linear_form(cardinality) else {
                return Ok(());
            };
            let Ok(index) = IndexTypeRef::from_finite_index_form(form) else {
                return Ok(());
            };
            index
        }
        hir::IndexRef::GenericParam(_) => return Ok(()),
    };
    check_index_override_dependency(
        local_types,
        dag,
        owner_decl,
        &actual,
        IndexNominalUse::TypeArgument,
    )
}

fn check_hir_generic_arg_override_dependencies(
    arg: &hir::GenericArg,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    owner_decl: Option<&ResolvedDeclName>,
) -> Result<(), GraphcalError> {
    match arg {
        hir::GenericArg::Index(index) => {
            check_hir_index_ref_override_dependency(index, local_types, dag, owner_decl)
        }
        hir::GenericArg::Type(type_expr) => {
            check_hir_type_override_dependencies(type_expr, local_types, dag, owner_decl)
        }
        hir::GenericArg::Dim(_) | hir::GenericArg::Nat(_) => Ok(()),
    }
}

fn check_hir_type_override_dependencies(
    type_expr: &hir::TypeExpr,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    owner_decl: Option<&ResolvedDeclName>,
) -> Result<(), GraphcalError> {
    match &type_expr.kind {
        hir::TypeExprKind::Struct(name) => check_type_override_dependency(
            local_types,
            dag,
            owner_decl,
            &name.value,
            TypeNominalUse::TypeArgument,
        ),
        hir::TypeExprKind::TypeApplication { name, generic_args } => {
            check_type_override_dependency(
                local_types,
                dag,
                owner_decl,
                &name.value,
                TypeNominalUse::TypeArgument,
            )?;
            generic_args.iter().try_for_each(|arg| {
                check_hir_generic_arg_override_dependencies(arg, local_types, dag, owner_decl)
            })
        }
        hir::TypeExprKind::Indexed { base, indexes } => {
            check_hir_type_override_dependencies(base, local_types, dag, owner_decl)?;
            indexes.iter().try_for_each(|index| {
                check_hir_index_ref_override_dependency(index, local_types, dag, owner_decl)
            })
        }
        hir::TypeExprKind::Index(index) | hir::TypeExprKind::Key(index) => {
            check_hir_index_ref_override_dependency(index, local_types, dag, owner_decl)
        }
        hir::TypeExprKind::Builtin(_)
        | hir::TypeExprKind::DimExpr(_)
        | hir::TypeExprKind::GenericTypeParam(_)
        | hir::TypeExprKind::Complex(_) => Ok(()),
    }
}

/// Infer a HIR expression.
pub(in crate::tir::dim_check) fn infer_hir_type_with_owner(
    expr: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    infer_hir_type_with_owner_and_cancellation(
        expr,
        owner_decl_name,
        declared_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
        &crate::cancellation::CancellationToken::unbounded(),
    )
}

pub(in crate::tir::dim_check) fn infer_hir_type_with_owner_and_cancellation(
    expr: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<InferredType, GraphcalError> {
    let locals = HirLocalTypes::root(cancellation, None);
    infer_hir_type(
        expr,
        owner_decl_name,
        declared_types,
        &locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

pub(in crate::tir::dim_check) fn infer_hir_type_with_materialized_shapes_and_cancellation(
    expr: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
    collector: MaterializedShapeCollector,
) -> Result<InferredType, GraphcalError> {
    let locals = HirLocalTypes::root_with_materialized_shapes(
        cancellation,
        collector,
        owner_decl_name.cloned(),
    );
    infer_hir_type(
        expr,
        owner_decl_name,
        declared_types,
        &locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

pub(in crate::tir::dim_check) fn infer_hir_type_with_nominal_dependencies_and_cancellation(
    expr: &hir::Expr,
    owner_decl_name: &ResolvedDeclName,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(InferredType, HashSet<NominalOverrideIdentity>), GraphcalError> {
    let (locals, collector) = HirLocalTypes::collecting_root(cancellation);
    let inferred = infer_hir_type(
        expr,
        Some(owner_decl_name),
        declared_types,
        &locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    Ok((inferred, collector.snapshot()))
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors syntax inference context"
)]
fn infer_hir_type(
    expr: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    local_types.checkpoint()?;
    // Recursion choke point: inference recurses once per tree level
    // (unbounded for left-nested operator chains).
    crate::stack::with_stack_growth(|| {
        infer_hir_type_inner(
            expr,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors infer_hir_type's signature"
)]
fn infer_hir_type_inner(
    expr: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inferred = match &expr.kind {
        // Error nodes exist only in tolerant lowering for IDE consumers; the
        // batch pipeline rejects them before TIR, so inference never sees one.
        hir::ExprKind::Error { .. } => {
            return Err(GraphcalError::InternalError {
                message: "unresolved reference reached type inference".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        hir::ExprKind::Number(_) => InferredType::Quantity(Dimension::dimensionless()),
        hir::ExprKind::Integer(_) => InferredType::Int,
        hir::ExprKind::Bool(_) => InferredType::Bool,
        hir::ExprKind::StringLiteral(_)
        | hir::ExprKind::OffsetDateTimeLiteral(_)
        | hir::ExprKind::CivilDateTimeLiteral(_)
        | hir::ExprKind::ZonedDateTimeLiteral(_)
        | hir::ExprKind::IanaTimeZoneLiteral(_) => {
            return Err(GraphcalError::DimensionMismatch {
                expected: "a numeric or boolean expression".to_string(),
                found: "contextual string literal".to_string(),
                help: "string literals can only be used in their declared datetime contexts"
                    .to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        hir::ExprKind::TypeSystemRef(name) => {
            return Err(GraphcalError::EvalError {
                message: name.value.value_position_error(),
                src: src.clone(),
                span: name.span.into(),
            });
        }
        hir::ExprKind::QuantityLiteral { unit, .. } => infer_hir_quantity_literal(unit, tir, src)?,
        hir::ExprKind::VariantLiteral(variant) => {
            check_index_override_dependency(
                local_types,
                dag,
                owner_decl_name,
                &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                IndexNominalUse::Label(variant.variant.variant()),
            )?;
            // A qualified label is self-typed: `Maneuver#Departure` is a
            // constant of type `Key<Maneuver>` — the axis is in the spelling.
            InferredType::Key(InferredIndex::from_resolved(
                variant.variant.index().clone(),
            ))
        }
        hir::ExprKind::GraphRef(target) => {
            infer_resolved_decl_ref_type(&target.value, target.span, declared_types, dag, src)?
        }
        hir::ExprKind::ConstRef(target) => infer_hir_const_ref(
            target,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            src,
        )?,
        hir::ExprKind::LocalRef(local) => {
            local_types
                .get(local.value)
                .cloned()
                .ok_or_else(|| GraphcalError::UnknownLocalRef {
                    name: format!("#{}", local.value.index()),
                    src: src.clone(),
                    span: local.span.into(),
                })?
        }
        hir::ExprKind::FnCall { callee, args, .. } => {
            if owner_decl_name
                .is_some_and(|owner| dag.semantic.override_reconciliations.contains_key(owner))
            {
                // Function inference has several specialized signature paths.
                // Check each argument once with the declaration identity intact
                // before those paths infer it for their own type rule.
                args.iter().try_for_each(|arg| {
                    infer_hir_type(
                        arg,
                        owner_decl_name,
                        declared_types,
                        local_types,
                        dag,
                        tir,
                        registry,
                        builtin_fns,
                        src,
                    )
                    .map(|_| ())
                })?;
            }
            infer_hir_fn_call(
                callee,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?
        }
        hir::ExprKind::ForComp { bindings, body } => infer_hir_for_comp(
            bindings,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::IndexAccess { expr: inner, args } => infer_hir_index_access(
            expr,
            inner,
            args.as_slice(),
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => infer_hir_if(
            condition,
            then_branch,
            else_branch,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::UnaryOp { op, operand } => infer_hir_unary(
            *op,
            operand,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::BinOp { op, lhs, rhs } => infer_hir_binop(
            expr.span,
            *op,
            lhs,
            rhs,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::Convert {
            expr: inner,
            target,
        } => infer_hir_convert(
            inner,
            target,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::DisplayTimezone {
            expr: inner,
            timezone,
        } => infer_hir_display_timezone(
            inner,
            timezone,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::FieldAccess { expr: inner, field } => infer_hir_field_access(
            inner,
            field,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::ConstructorCall {
            callee,
            generic_args,
            fields,
        } => infer_hir_constructor_call(
            expr,
            callee,
            generic_args,
            fields,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::MapLiteral { entries } => infer_hir_map_literal(
            expr,
            entries,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::Scan {
            source,
            init,
            acc,
            val,
            body,
        } => infer_hir_scan(
            source,
            init,
            acc,
            val,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::Unfold {
            recurrence,
            init,
            body,
        } => infer_hir_unfold(
            &recurrence.axis,
            init,
            &recurrence.previous_state,
            &recurrence.previous_index,
            &recurrence.current_index,
            body,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::KeyForm {
            kind,
            axis,
            axis_span,
            arg,
        } => infer_hir_key_form(
            *kind,
            axis,
            *axis_span,
            arg,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::Match { scrutinee, arms } => infer_hir_match(
            expr,
            scrutinee,
            arms,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
        hir::ExprKind::DagCall {
            target,
            args,
            static_bindings,
            output,
        } => infer_hir_dag_call(
            expr,
            target,
            args,
            static_bindings,
            output,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?,
    };
    if let Some((collector, owner)) = &local_types.control.materialized_shapes {
        collector.record(owner.as_ref(), expr, &inferred, tir, src)?;
    }
    Ok(inferred)
}

fn infer_hir_quantity_literal(
    unit: &hir::ResolvedUnitExpr,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let dim = rules::resolve_unit_dimension_or_diagnose(unit, tir, src)?;
    Ok(InferredType::Quantity(dim))
}

fn infer_resolved_decl_ref_type(
    target: &ResolvedDeclKey,
    span: Span,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let local_name = ScopedName::local(target.to_unowned_def_name());

    if target.owner() == &dag.dag_id
        && let Some(inferred) = infer_bound_decl_type(&local_name, declared_types, dag, src)?
    {
        return Ok(inferred);
    }

    for name in dag
        .semantic
        .decl_bindings
        .iter()
        .filter_map(|(name, resolved)| (resolved == target).then_some(name))
    {
        if let Some(inferred) = infer_bound_decl_type(name, declared_types, dag, src)? {
            return Ok(inferred);
        }
    }

    Err(GraphcalError::UnknownGraphRef {
        name: local_name,
        src: src.clone(),
        span: span.into(),
    })
}

fn infer_bound_decl_type(
    name: &ScopedName,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    dag: &crate::tir::typed::DagTIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Option<InferredType>, GraphcalError> {
    if let Some(resolved_type) = dag.resolved_decl_types.get(name) {
        let dim_sub = HashMap::new();
        let index_sub =
            HashMap::<GenericParamName, crate::registry::declared_type::IndexTypeRef>::new();
        let nat_sub = HashMap::new();
        return crate::tir::typed::substitute_resolved_type(
            resolved_type,
            &dim_sub,
            &index_sub,
            &nat_sub,
            src,
        )
        .map(Some);
    }

    Ok(declared_types.get(name).map(InferredType::from))
}

fn infer_hir_const_ref(
    target: &crate::syntax::span::Spanned<ConstRef>,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match &target.value {
        ConstRef::Decl(resolved) => {
            infer_resolved_decl_ref_type(resolved, target.span, declared_types, dag, src)
        }
        ConstRef::Builtin(_) => Ok(InferredType::Quantity(Dimension::dimensionless())),
        ConstRef::TimeScale(_) => Err(GraphcalError::DimensionMismatch {
            expected: "value expression".to_string(),
            found: "time scale".to_string(),
            help: "time scales are static arguments; write `epoch<TT>(\"...\")`".to_string(),
            src: src.clone(),
            span: target.span.into(),
        }),
        ConstRef::Constructor(constructor) => {
            let target_def = dag
                .semantic
                .constructor_refs
                .constructor_defs
                .get(constructor)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "semantic constructor metadata missing for nullary constructor `{constructor}`"
                    ),
                    src: src.clone(),
                    span: target.span.into(),
                })?;
            check_type_override_dependency(
                local_types,
                dag,
                owner_decl_name,
                &target_def.owning_type,
                TypeNominalUse::Constructor(constructor),
            )?;
            if !target_def.variant.fields().is_empty() {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "constructor `{}` requires field arguments",
                        target_def.variant.name()
                    ),
                    src: src.clone(),
                    span: target.span.into(),
                });
            }
            let type_args = resolve_applied_generic_args(
                &target_def.owning_type,
                &target_def.type_def,
                &[],
                dag,
                tir,
                registry,
                src,
                target.span,
                None,
            )?;
            Ok(InferredType::Struct(
                InferredStructType::from_resolved(target_def.owning_type.clone()),
                type_args,
            ))
        }
        ConstRef::GenericNatParam(param) => Err(GraphcalError::EvalError {
            message: format!(
                "generic Nat parameter `{}` is type-level only and is not a runtime value",
                param.name
            ),
            src: src.clone(),
            span: target.span.into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors syntax inference context"
)]
fn infer_arg(
    arg: &hir::Expr,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    infer_hir_type(
        arg,
        None,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_linear_algebra_call(
    function: crate::builtin::LinearAlgebraFn,
    callee_span: Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let name = function.builtin_name();
    if args.len() != function.arity() {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
            expected: function.arity(),
            got: args.len(),
            src: src.clone(),
            span: callee_span.into(),
        });
    }
    let argument_types = args
        .iter()
        .map(|arg| {
            infer_arg(
                arg,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    infer_linear_algebra_type(function, &argument_types, |index| {
        super::concrete_cardinality_for_inferred(index, Some(dag), registry)
    })
    .map_err(|error| match error {
        LinearAlgebraTypeError::WrongArity { expected, found } => GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
            expected,
            got: found,
            src: src.clone(),
            span: callee_span.into(),
        },
        LinearAlgebraTypeError::ExpectedIndexedQuantity { argument, rank } => {
            GraphcalError::DimensionMismatch {
                expected: format!("rank-{rank} indexed quantity"),
                found: format_inferred_type(&argument_types[argument], registry),
                help: format!(
                    "{}() requires argument {} to be a rank-{rank} indexed quantity",
                    name.as_str(),
                    argument.saturating_add(1)
                ),
                src: src.clone(),
                span: args[argument].span.into(),
            }
        }
        LinearAlgebraTypeError::AxisMismatch {
            argument,
            expected,
            found,
        } => GraphcalError::LinearAlgebraShapeMismatch {
            function: name,
            expected: expected.to_string(),
            found: found.to_string(),
            help: "linear-algebra contractions match axes by typed identity; use the same declared index (or the same Fin(N) structural index) at both contracted positions"
                .to_string(),
            src: src.clone(),
            span: args[argument].span.into(),
        },
        LinearAlgebraTypeError::CardinalityMismatch {
            argument,
            expected,
            found,
        } => GraphcalError::LinearAlgebraShapeMismatch {
            function: name,
            expected: format!("an axis with exactly {expected} entries"),
            found: found.map_or_else(
                || "an axis whose cardinality is not concrete".to_string(),
                |cardinality| format!("an axis with {cardinality} entries"),
            ),
            help: format!("{}() is defined only for three-component vectors", name.as_str()),
            src: src.clone(),
            span: args[argument].span.into(),
        },
        LinearAlgebraTypeError::ConcreteCardinalityRequired { argument } => {
            GraphcalError::LinearAlgebraShapeMismatch {
                function: name,
                expected: "an axis with a concrete cardinality".to_string(),
                found: "an axis whose cardinality is still generic".to_string(),
                help: format!(
                    "{}() needs a concrete matrix size because its result dimension depends on that size",
                    name.as_str()
                ),
                src: src.clone(),
                span: args[argument].span.into(),
            }
        }
        LinearAlgebraTypeError::DimensionOverflow => GraphcalError::DimensionOverflow {
            src: src.clone(),
            span: callee_span.into(),
        },
    })
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_fn_call(
    callee: &crate::syntax::span::Spanned<FunctionRef>,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let (name, epoch_scale) = match &callee.value {
        FunctionRef::Builtin(name) => (*name, None),
        FunctionRef::Epoch { scale } => (BuiltinFnName::Epoch, Some(scale.value)),
        FunctionRef::External(ext) => {
            return infer_extern_fn_call(
                ext,
                callee.span,
                args,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            );
        }
    };
    match type_rule_for_builtin(name) {
        BuiltinTypeRule::Complex(function) => infer_hir_complex_call(
            function,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
        BuiltinTypeRule::CollectionAggregation(kind) => {
            if args.len() != 1 {
                return Err(GraphcalError::WrongArity {
                    name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
                    expected: 1,
                    got: args.len(),
                    src: src.clone(),
                    span: callee.span.into(),
                });
            }
            let arg_type = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            let InferredType::Indexed { element, index } = &arg_type else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "indexed collection".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: format!("{}() requires an indexed value", name.as_str()),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            let rank = arg_type.indexed_rank();
            if rank > 1 {
                return Err(GraphcalError::MultiAxisAggregation {
                    function: kind.builtin_name(),
                    rank,
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            if kind == AggregationFn::Count {
                return Ok(InferredType::Int);
            }
            if matches!(kind, AggregationFn::Argmin | AggregationFn::Argmax) {
                // The extremum's identity: a key of the reduced axis. The
                // element-type requirement below still applies, so check it
                // before returning.
                if element.quantity_dimension().is_none() {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "indexed quantity collection".to_string(),
                        found: format_inferred_type(element, registry),
                        help: format!(
                            "{}() requires every indexed element to be quantity",
                            name.as_str()
                        ),
                        src: src.clone(),
                        span: args[0].span.into(),
                    });
                }
                return Ok(InferredType::Key(index.clone()));
            }
            let Some(dimension) = element.quantity_dimension().cloned() else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "indexed quantity collection".to_string(),
                    found: format_inferred_type(element, registry),
                    help: format!(
                        "{}() requires every indexed element to be quantity",
                        name.as_str()
                    ),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            if kind != AggregationFn::Product || dimension.is_dimensionless() {
                return Ok(InferredType::Quantity(dimension));
            }
            let cardinality = super::concrete_cardinality_for_inferred(index, Some(dag), registry)
                .ok_or_else(|| GraphcalError::AggregationCardinalityUnknown {
                    function: kind.builtin_name(),
                    src: src.clone(),
                    span: args[0].span.into(),
                })?;
            let exponent =
                i32::try_from(cardinality).map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: args[0].span.into(),
                })?;
            dimension
                .pow(exponent)
                .map(InferredType::Quantity)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: args[0].span.into(),
                })
        }
        BuiltinTypeRule::LinearAlgebra(function) => infer_hir_linear_algebra_call(
            function,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
        BuiltinTypeRule::TypeConversion(kind) => infer_hir_type_conversion(
            kind,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
        BuiltinTypeRule::TimeScaleConversion(scale) => infer_hir_timescale_conversion(
            name,
            scale,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
        BuiltinTypeRule::DatetimeConstructor(kind) => infer_hir_datetime_constructor(
            kind,
            epoch_scale,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
        BuiltinTypeRule::DatetimeExtract => infer_hir_datetime_unary(
            name,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
            InferredType::Int,
        ),
        BuiltinTypeRule::DatetimeFromNumeric => {
            if args.len() != 1 {
                return Err(GraphcalError::WrongArity {
                    name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
                    expected: 1,
                    got: args.len(),
                    src: src.clone(),
                    span: callee.span.into(),
                });
            }
            let arg_type = infer_arg(
                &args[0],
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            match &arg_type {
                t if t
                    .quantity_dimension()
                    .is_some_and(Dimension::is_dimensionless) => {}
                InferredType::Int => {}
                _ => {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Dimensionless or Int".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        help: format!(
                            "{}() requires a dimensionless numeric argument",
                            name.as_str()
                        ),
                        src: src.clone(),
                        span: args[0].span.into(),
                    });
                }
            }
            Ok(InferredType::Datetime(
                crate::registry::time_scale::TimeScale::UTC,
            ))
        }
        BuiltinTypeRule::DatetimeToNumeric => infer_hir_datetime_unary(
            name,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
            InferredType::Quantity(Dimension::dimensionless()),
        ),
        BuiltinTypeRule::RegistrySignature => infer_hir_builtin_fn(
            name,
            callee.span,
            args,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        ),
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_complex_call(
    function: crate::builtin::ComplexFn,
    callee_span: Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use super::complex::ComplexTypeError;

    let inferred = args
        .iter()
        .map(|arg| {
            infer_arg(
                arg,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    super::complex::infer(function, &inferred).map_err(|error| match error {
        ComplexTypeError::WrongArity { expected, got } => GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(
                function.builtin_name().as_str(),
            ),
            expected,
            got,
            src: src.clone(),
            span: callee_span.into(),
        },
        ComplexTypeError::ExpectedQuantity { argument } => GraphcalError::DimensionMismatch {
            expected: "quantity type".to_string(),
            found: format_inferred_type(&inferred[argument], registry),
            help: format!(
                "{}() requires a quantity in argument {}",
                function.builtin_name().as_str(),
                argument.saturating_add(1)
            ),
            src: src.clone(),
            span: args[argument].span.into(),
        },
        ComplexTypeError::ExpectedComplex { argument } => GraphcalError::DimensionMismatch {
            expected: "Complex<D>".to_string(),
            found: format_inferred_type(&inferred[argument], registry),
            help: format!(
                "{}() requires a complex quantity",
                function.builtin_name().as_str()
            ),
            src: src.clone(),
            span: args[argument].span.into(),
        },
        ComplexTypeError::ExpectedQuantityOrComplex { argument } => {
            GraphcalError::DimensionMismatch {
                expected: "a real or complex quantity".to_string(),
                found: format_inferred_type(&inferred[argument], registry),
                help: format!(
                    "{}() requires a real or complex quantity",
                    function.builtin_name().as_str()
                ),
                src: src.clone(),
                span: args[argument].span.into(),
            }
        }
        ComplexTypeError::DimensionMismatch { left, right } => GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&inferred[left], registry),
            found: format_inferred_type(&inferred[right], registry),
            help: "real and imaginary components must have the same dimension".to_string(),
            src: src.clone(),
            span: args[right].span.into(),
        },
        ComplexTypeError::ExpectedAngle { argument } => GraphcalError::DimensionMismatch {
            expected: "Angle".to_string(),
            found: format_inferred_type(&inferred[argument], registry),
            help: "polar() phase must be an Angle quantity".to_string(),
            src: src.clone(),
            span: args[argument].span.into(),
        },
        ComplexTypeError::ExpectedDimensionless { argument } => GraphcalError::DimensionMismatch {
            expected: "Dimensionless or Complex<Dimensionless>".to_string(),
            found: format_inferred_type(&inferred[argument], registry),
            help: "exp() requires a dimensionless real or complex argument".to_string(),
            src: src.clone(),
            span: args[argument].span.into(),
        },
    })
}

/// Check an extern (plugin) function call against its declared
/// [`crate::function_signature::FunctionSignature`], using the same
/// bind/check dimension-variable walk as built-in signatures.
#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_extern_fn_call(
    ext: &hir::ExternFnRef,
    callee_span: Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::function_signature::{ScalarValueKind, ValueKind};

    use super::super::builtins::{check_quantity_param, eval_result_monomial};

    let Some(function) = tir.extern_functions.get(&ext.key()) else {
        return Err(GraphcalError::UnknownExternFunction {
            alias: ext.alias.clone(),
            name: ext.name.clone(),
            src: src.clone(),
            span: callee_span.into(),
        });
    };
    let sig = &function.signature;
    if args.len() != sig.arity() {
        return Err(GraphcalError::WrongArity {
            name: ext.name.clone(),
            expected: sig.arity(),
            got: args.len(),
            src: src.clone(),
            span: callee_span.into(),
        });
    }

    // Boundary rendering for diagnostics only.
    let display_name = ext.to_string();
    let mut bindings: HashMap<crate::syntax::dimension::DimVarName, Dimension> = HashMap::new();
    let mut index_bindings: HashMap<crate::syntax::index_name::IndexVarName, InferredIndex> =
        HashMap::new();
    for (param, arg) in sig.params().iter().zip(args) {
        let arg_type = infer_arg(
            arg,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        match &param.kind {
            ValueKind::Scalar(ScalarValueKind::Bool) => {
                if !matches!(arg_type, InferredType::Bool) {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Bool".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        help: format!("parameter `{}` requires Bool", param.name),
                        src: src.clone(),
                        span: arg.span.into(),
                    });
                }
            }
            ValueKind::Scalar(ScalarValueKind::Int) => {
                if arg_type != InferredType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_inferred_type(&arg_type, registry),
                        help: format!("parameter `{}` requires Int", param.name),
                        src: src.clone(),
                        span: arg.span.into(),
                    });
                }
            }
            ValueKind::Scalar(ScalarValueKind::Quantity(monomial)) => {
                let arg_dim = expect_quantity(&arg_type, registry, src, arg.span)?;
                check_quantity_param(
                    &display_name,
                    sig,
                    &param.name,
                    monomial,
                    &arg_dim,
                    &mut bindings,
                    registry,
                    src,
                    arg.span,
                )?;
            }
            ValueKind::Struct(_) => {
                // FunctionSignature::try_new rejects struct parameters.
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "extern function `{display_name}` carries a struct parameter past signature validation"
                    ),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            }
            ValueKind::Indexed { element, indexes } => {
                let mut current = &arg_type;
                let mut arg_indexes = Vec::with_capacity(indexes.len());
                for _ in indexes {
                    let InferredType::Indexed {
                        element,
                        index: arg_index,
                    } = current
                    else {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format!("a rank-{} indexed collection", indexes.len()),
                            found: format_inferred_type(&arg_type, registry),
                            help: format!(
                                "parameter `{}` of `{display_name}` takes one axis for each declared index variable",
                                param.name
                            ),
                            src: src.clone(),
                            span: arg.span.into(),
                        });
                    };
                    arg_indexes.push(arg_index);
                    current = element;
                }
                match element {
                    ScalarValueKind::Quantity(monomial) => {
                        let Some(arg_dim) = current.quantity_dimension().cloned() else {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!(
                                    "a rank-{} indexed quantity collection",
                                    indexes.len()
                                ),
                                found: format_inferred_type(&arg_type, registry),
                                help: format!(
                                    "parameter `{}` of `{display_name}` requires quantity elements",
                                    param.name
                                ),
                                src: src.clone(),
                                span: arg.span.into(),
                            });
                        };
                        check_quantity_param(
                            &display_name,
                            sig,
                            &param.name,
                            monomial,
                            &arg_dim,
                            &mut bindings,
                            registry,
                            src,
                            arg.span,
                        )?;
                    }
                    scalar @ (ScalarValueKind::Bool | ScalarValueKind::Int) => {
                        let name = if matches!(scalar, ScalarValueKind::Bool) {
                            "Bool"
                        } else {
                            "Int"
                        };
                        let matches = matches!(
                            (scalar, current),
                            (ScalarValueKind::Bool, InferredType::Bool)
                                | (ScalarValueKind::Int, InferredType::Int)
                        );
                        if !matches {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!(
                                    "{name} with exactly {} indexed axes",
                                    indexes.len()
                                ),
                                found: format_inferred_type(&arg_type, registry),
                                help: format!(
                                    "parameter `{}` of `{display_name}` requires {name} elements",
                                    param.name
                                ),
                                src: src.clone(),
                                span: arg.span.into(),
                            });
                        }
                    }
                }
                for (index, arg_index) in indexes.iter().zip(arg_indexes) {
                    match index_bindings.entry(index.clone()) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            slot.insert(arg_index.clone());
                        }
                        std::collections::hash_map::Entry::Occupied(bound) => {
                            if bound.get() != arg_index {
                                return Err(GraphcalError::DimensionMismatch {
                                    expected: format!(
                                        "an axis over `{}` (index variable `{index}` was bound by an earlier argument)",
                                        bound.get()
                                    ),
                                    found: format_inferred_type(&arg_type, registry),
                                    help: format!(
                                        "axes sharing index variable `{index}` of `{display_name}` must use the same typed index"
                                    ),
                                    src: src.clone(),
                                    span: arg.span.into(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    match sig.result() {
        ValueKind::Scalar(ScalarValueKind::Bool) => Ok(InferredType::Bool),
        ValueKind::Scalar(ScalarValueKind::Int) => Ok(InferredType::Int),
        ValueKind::Scalar(ScalarValueKind::Quantity(monomial)) => {
            eval_result_monomial(&display_name, monomial, &bindings, src, callee_span)
                .map(InferredType::Quantity)
        }
        ValueKind::Indexed { element, indexes } => {
            let leaf = match element {
                ScalarValueKind::Quantity(monomial) => {
                    eval_result_monomial(&display_name, monomial, &bindings, src, callee_span)
                        .map(InferredType::Quantity)?
                }
                ScalarValueKind::Bool => InferredType::Bool,
                ScalarValueKind::Int => InferredType::Int,
            };
            indexes.iter().rev().try_fold(leaf, |element, index| {
                let Some(bound) = index_bindings.get(index) else {
                    // try_new guarantees every result index variable indexes
                    // some parameter, so this is a compiler bug.
                    return Err(GraphcalError::InternalError {
                        message: format!(
                            "result index variable `{index}` of `{display_name}` was not bound by any argument"
                        ),
                        src: src.clone(),
                        span: callee_span.into(),
                    });
                };
                Ok(InferredType::Indexed {
                    element: Box::new(element),
                    index: bound.clone(),
                })
            })
        }
        ValueKind::Struct(_) => {
            // The nominal identity lives on the entry (the shape in the
            // signature is the manifest-facing contract); extern struct
            // returns are non-generic records, so the argument list is empty.
            let Some(result_struct) = &function.result_struct else {
                return Err(GraphcalError::InternalError {
                    message: format!(
                        "extern function `{display_name}` declares a struct result without a resolved record type"
                    ),
                    src: src.clone(),
                    span: callee_span.into(),
                });
            };
            Ok(InferredType::Struct(
                InferredStructType::from_resolved(result_struct.resolved.clone()),
                Vec::new(),
            ))
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_builtin_fn(
    name: BuiltinFnName,
    callee_span: Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let Some(func) = builtin_fns.get(&name) else {
        return Err(GraphcalError::UnknownFunction {
            name: name.as_str().to_string(),
            src: src.clone(),
            span: callee_span.into(),
        });
    };
    if args.len() != func.arity() {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
            expected: func.arity(),
            got: args.len(),
            src: src.clone(),
            span: callee_span.into(),
        });
    }
    let dimension_args = args
        .iter()
        .map(|arg| {
            let inferred = infer_arg(
                arg,
                declared_types,
                local_types,
                dag,
                tir,
                registry,
                builtin_fns,
                src,
            )?;
            let dimension = expect_quantity(&inferred, registry, src, arg.span)?;
            Ok(crate::syntax::span::Spanned::new(dimension, arg.span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    infer_fn_dim(
        name.as_str(),
        func.signature(),
        &dimension_args,
        callee_span,
        registry,
        src,
    )
    .map(InferredType::Quantity)
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_type_conversion(
    kind: TypeConversionFn,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let expected_arity = 1;
    if args.len() != expected_arity {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(kind.as_str()),
            expected: expected_arity,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    match kind {
        TypeConversionFn::ToFloat => {
            if arg_type != InferredType::Int {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "to_float() requires an Int argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            Ok(InferredType::Quantity(Dimension::dimensionless()))
        }
        TypeConversionFn::ToInt => {
            // A `Fin`-axis key exposes its position: the position is the
            // key's semantic content. Named and coordinate keys stay opaque.
            if let InferredType::Key(index) = &arg_type {
                if index.finite_index_form().is_some() {
                    return Ok(InferredType::Int);
                }
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Key<Fin(N)>".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "to_int() extracts positions from Fin-axis keys only; \
                           named and coordinate keys have no ordinal"
                        .to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            let dim = expect_quantity(&arg_type, registry, src, args[0].span)?;
            if !dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&dim),
                    help: "to_int() requires a Dimensionless argument".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            Ok(InferredType::Int)
        }
        TypeConversionFn::Coord => {
            let InferredType::Key(index) = &arg_type else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Key<C> for a coordinate axis C".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "coord() extracts the coordinate quantity of a \
                           coordinate-axis key"
                        .to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            };
            if index.finite_index_form().is_some() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Key<C> for a coordinate axis C".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "coord() applies to coordinate-axis keys only; named \
                           keys are opaque and Fin keys expose to_int()"
                        .to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            let index_def =
                super::index_def_for_inferred(index, Some(dag), registry).ok_or_else(|| {
                    GraphcalError::UnknownIndex {
                        name: index.name(),
                        src: src.clone(),
                        span: args[0].span.into(),
                    }
                })?;
            match &index_def.kind {
                crate::registry::types::IndexKind::Coordinate(data) => {
                    Ok(InferredType::Quantity(data.dimension.clone()))
                }
                crate::registry::types::IndexKind::RequiredCoordinate { dimension } => {
                    Ok(InferredType::Quantity(dimension.clone()))
                }
                _ => Err(GraphcalError::DimensionMismatch {
                    expected: "Key<C> for a coordinate axis C".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "coord() applies to coordinate-axis keys only; named \
                           keys are opaque and Fin keys expose to_int()"
                        .to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                }),
            }
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_timescale_conversion(
    name: BuiltinFnName,
    scale: crate::registry::time_scale::TimeScale,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if args.len() != 1 {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
            expected: 1,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(arg_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&arg_type, registry),
            help: format!("{}() requires a Datetime argument", name.as_str()),
            src: src.clone(),
            span: args[0].span.into(),
        });
    }
    Ok(InferredType::Datetime(scale))
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_datetime_constructor(
    kind: DatetimeConstructorFn,
    epoch_scale: Option<crate::registry::time_scale::TimeScale>,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    match kind {
        DatetimeConstructorFn::Datetime => {
            if args.is_empty() || args.len() > 2 {
                return Err(GraphcalError::EvalError {
                    message: format!("datetime() expects 1 or 2 arguments, got {}", args.len()),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            let first_is_valid = match args.len() {
                1 => matches!(args[0].kind, hir::ExprKind::OffsetDateTimeLiteral(_)),
                2 => matches!(args[0].kind, hir::ExprKind::ZonedDateTimeLiteral(_)),
                _ => false,
            };
            if !first_is_valid {
                let found = infer_arg(
                    &args[0],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                return Err(GraphcalError::DimensionMismatch {
                    expected: "datetime literal".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "datetime() requires a contextual datetime string literal".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            if args.len() == 2 && !matches!(args[1].kind, hir::ExprKind::IanaTimeZoneLiteral(_)) {
                let found = infer_arg(
                    &args[1],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                return Err(GraphcalError::DimensionMismatch {
                    expected: "timezone literal".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "datetime() second argument must be an IANA timezone literal".to_string(),
                    src: src.clone(),
                    span: args[1].span.into(),
                });
            }
            let resolved_timezone_matches_argument = match args {
                [
                    hir::Expr {
                        kind: hir::ExprKind::ZonedDateTimeLiteral(datetime),
                        ..
                    },
                    hir::Expr {
                        kind: hir::ExprKind::IanaTimeZoneLiteral(time_zone),
                        ..
                    },
                ] => datetime.time_zone() == time_zone,
                _ => true,
            };
            if !resolved_timezone_matches_argument {
                return Err(GraphcalError::InternalError {
                    message: "resolved datetime timezone does not match its source argument"
                        .to_string(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(InferredType::Datetime(
                crate::registry::time_scale::TimeScale::UTC,
            ))
        }
        DatetimeConstructorFn::Epoch => {
            if args.len() != 1 {
                return Err(GraphcalError::WrongArity {
                    name: crate::syntax::function_name::FnName::expect_valid("epoch"),
                    expected: 1,
                    got: args.len(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            if !matches!(args[0].kind, hir::ExprKind::CivilDateTimeLiteral(_)) {
                let found = infer_arg(
                    &args[0],
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                return Err(GraphcalError::DimensionMismatch {
                    expected: "scale-free datetime literal".to_string(),
                    found: format_inferred_type(&found, registry),
                    help: "epoch<S>() requires one civil datetime string literal".to_string(),
                    src: src.clone(),
                    span: args[0].span.into(),
                });
            }
            epoch_scale
                .map(InferredType::Datetime)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: "epoch call reached type inference without a static time scale"
                        .to_string(),
                    src: src.clone(),
                    span: span.into(),
                })
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "function-call context")]
fn infer_hir_datetime_unary(
    name: BuiltinFnName,
    span: crate::syntax::span::Span,
    args: &[hir::Expr],
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    result: InferredType,
) -> Result<InferredType, GraphcalError> {
    if args.len() != 1 {
        return Err(GraphcalError::WrongArity {
            name: crate::syntax::function_name::FnName::expect_valid(name.as_str()),
            expected: 1,
            got: args.len(),
            src: src.clone(),
            span: span.into(),
        });
    }
    let arg_type = infer_arg(
        &args[0],
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(arg_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&arg_type, registry),
            help: format!("{}() requires a Datetime argument", name.as_str()),
            src: src.clone(),
            span: args[0].span.into(),
        });
    }
    Ok(result)
}

#[expect(clippy::too_many_arguments, reason = "if expression context")]
fn infer_hir_if(
    condition: &hir::Expr,
    then_branch: &hir::Expr,
    else_branch: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let infer = |expr: &hir::Expr| {
        infer_hir_type(
            expr,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )
    };
    let cond_type = infer(condition)?;
    let then_type = infer(then_branch)?;
    let else_type = infer(else_branch)?;
    rules::if_rule(
        &Operand {
            ty: cond_type,
            span: condition.span,
        },
        &Operand {
            ty: then_type,
            span: then_branch.span,
        },
        &Operand {
            ty: else_type,
            span: else_branch.span,
        },
        registry,
        src,
    )
}

#[expect(clippy::too_many_arguments, reason = "unary expression context")]
fn infer_hir_unary(
    op: crate::desugar::desugared_ast::UnaryOp,
    operand: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let operand_type = infer_hir_type(
        operand,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    rules::unary_rule(
        op,
        &Operand {
            ty: operand_type,
            span: operand.span,
        },
        registry,
        src,
    )
}

use super::rules::{self, Operand};

fn try_const_int(expr: &hir::Expr) -> Option<i64> {
    use crate::desugar::desugared_ast::BinOp;
    match &expr.kind {
        hir::ExprKind::Integer(n) => Some(*n),
        hir::ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => try_const_int(operand)?.checked_neg(),
        hir::ExprKind::BinOp { op, lhs, rhs } => {
            let l = try_const_int(lhs)?;
            let r = try_const_int(rhs)?;
            match op {
                BinOp::Add => l.checked_add(r),
                BinOp::Sub => l.checked_sub(r),
                BinOp::Mul => l.checked_mul(r),
                BinOp::Div if r != 0 => l.checked_div(r),
                BinOp::Mod if r != 0 => l.checked_rem(r),
                BinOp::Pow(_) if r >= 0 => u32::try_from(r).ok().and_then(|e| l.checked_pow(e)),
                _ => None,
            }
        }
        _ => None,
    }
}

#[expect(clippy::too_many_arguments, reason = "binary expression context")]
fn infer_hir_binop(
    span: crate::syntax::span::Span,
    op: crate::desugar::desugared_ast::BinOp,
    lhs: &hir::Expr,
    rhs: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::desugar::desugared_ast::BinOp;
    let lhs_type = infer_hir_type(
        lhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let rhs_type = infer_hir_type(
        rhs,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Exact exponent shape is carried by `BinOp::Pow`; constant folding is
    // needed for runtime-classified right-associated Int power chains and for
    // the additive Fin-key rule (`k + c` shifts the bound by a static Nat).
    let rhs_const_int = if matches!(op, BinOp::Pow(_) | BinOp::Add) {
        try_const_int(rhs)
    } else {
        None
    };
    rules::binop_rule(
        span,
        op,
        &Operand {
            ty: lhs_type,
            span: lhs.span,
        },
        &Operand {
            ty: rhs_type,
            span: rhs.span,
        },
        rhs_const_int,
        registry,
        src,
    )
}

pub(in crate::tir::dim_check) fn hir_nat_to_linear_form(
    expr: &hir::NatExpr,
) -> Result<NatPolyForm, NatOverflowError> {
    match expr {
        hir::NatExpr::Literal(n, _) => Ok(NatPolyForm::from_constant(*n)),
        hir::NatExpr::Param(param) => Ok(NatPolyForm::from_var(param.value.name.clone())),
        hir::NatExpr::Add(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(0), |sum, operand| {
                sum.add(&hir_nat_to_linear_form(operand)?)
            }),
        hir::NatExpr::Mul(operands, _) => operands
            .iter()
            .try_fold(NatPolyForm::from_constant(1), |product, operand| {
                product.mul(&hir_nat_to_linear_form(operand)?)
            }),
    }
}

fn resolve_hir_nat_form(
    expr: &hir::NatExpr,
    substitutions: Option<&ConcreteGenericSubstitutions>,
    src: &NamedSource<Arc<String>>,
) -> Result<NatPolyForm, GraphcalError> {
    let form = hir_nat_to_linear_form(expr)
        .map_err(|error| nat_overflow_error(error, src, expr.span()))?;
    let Some(substitutions) = substitutions else {
        return Ok(form);
    };
    form.evaluate(&substitutions.bindings().nats)
        .map(NatPolyForm::from_constant)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!(
                "Nat expression `{}` could not be concretely instantiated",
                form.format()
            ),
            src: src.clone(),
            span: expr.span().into(),
        })
}

fn nat_overflow_error(
    err: NatOverflowError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

fn finite_index_error(
    err: crate::registry::types::FiniteIndexError,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: err.to_string(),
        src: src.clone(),
        span: span.into(),
    }
}

/// Infer a key introduction form.
///
/// `key(Fin(N), c)` is compile-time membership-checked and infallible;
/// `fin_key(Fin(N), e)` is the explicit runtime-checked constructor; the
/// coordinate searches require a coordinate axis and a matching-dimension
/// quantity argument.
#[expect(clippy::too_many_arguments, reason = "expression inference context")]
fn infer_hir_key_form(
    kind: crate::syntax::ast::KeyFormKind,
    axis: &hir::expr::ForBindingIndex,
    axis_span: Span,
    arg: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    use crate::syntax::ast::KeyFormKind;

    let arg_type = infer_hir_type(
        arg,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // Resolve the axis identity and, for Fin axes, its cardinality form.
    let (index_identity, finite_form) = match axis {
        hir::expr::ForBindingIndex::Named(index) => {
            let identity = InferredIndex::from_resolved(index.value.clone());
            let idx_def = super::index_def_for_inferred(&identity, Some(dag), registry)
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: identity.name(),
                    src: src.clone(),
                    span: index.span.into(),
                })?;
            let finite_form = idx_def.finite_index_size().map(NatPolyForm::from_constant);
            (identity, finite_form)
        }
        hir::expr::ForBindingIndex::Finite { cardinality, span } => {
            let form = resolve_hir_nat_form(cardinality, local_types.generic_substitutions(), src)?;
            let identity = InferredIndex::from_finite_index_form(form.clone())
                .map_err(|err| finite_index_error(err, src, *span))?;
            (identity, Some(form))
        }
    };
    match kind {
        KeyFormKind::Static => {
            let Some(form) = &finite_form else {
                return Err(GraphcalError::EvalError {
                    message: "key() constructs Fin-axis keys; named-axis keys are written as \
                              qualified labels and coordinate keys come from argmax/argmin or \
                              the coordinate searches"
                        .to_string(),
                    src: src.clone(),
                    span: axis_span.into(),
                });
            };
            if arg_type != InferredType::Int {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "a static Nat position".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "key(Fin(N), position) takes an integer position".to_string(),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            }
            let Some(position) = try_const_int(arg) else {
                return Err(GraphcalError::EvalError {
                    message: "key() requires a static position; use fin_key() for a \
                              runtime-checked position"
                        .to_string(),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            };
            if position < 0 {
                return Err(GraphcalError::EvalError {
                    message: format!("key() position evaluated to negative value: {position}"),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            }
            if form.is_constant() {
                let size = form.constant();
                let position_u64 = u64::try_from(position).unwrap_or(u64::MAX);
                if position_u64 >= size {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "key() position {position} is out of bounds for Fin({size})"
                        ),
                        src: src.clone(),
                        span: arg.span.into(),
                    });
                }
            }
            Ok(InferredType::Key(index_identity))
        }
        KeyFormKind::Fin => {
            if finite_form.is_none() {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "fin_key() requires a Fin(...) axis, got `{}`",
                        index_identity.name()
                    ),
                    src: src.clone(),
                    span: axis_span.into(),
                });
            }
            if arg_type != InferredType::Int {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: format_inferred_type(&arg_type, registry),
                    help: "fin_key(Fin(N), position) takes an Int position, checked at \
                           runtime"
                        .to_string(),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            }
            Ok(InferredType::Key(index_identity))
        }
        KeyFormKind::Floor | KeyFormKind::Ceil | KeyFormKind::Nearest => {
            let idx_def = super::index_def_for_inferred(&index_identity, Some(dag), registry);
            let dimension = match idx_def.map(|def| &def.kind) {
                Some(crate::registry::types::IndexKind::Coordinate(data)) => data.dimension.clone(),
                Some(crate::registry::types::IndexKind::RequiredCoordinate { dimension }) => {
                    dimension.clone()
                }
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "{}() requires a coordinate axis, got `{}`",
                            kind.as_str(),
                            index_identity.name()
                        ),
                        src: src.clone(),
                        span: axis_span.into(),
                    });
                }
            };
            let arg_dim = expect_quantity(&arg_type, registry, src, arg.span)?;
            if arg_dim != dimension {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(&dimension),
                    found: registry.dimensions.format_dimension(&arg_dim),
                    help: format!("{}() takes a quantity in the axis dimension", kind.as_str()),
                    src: src.clone(),
                    span: arg.span.into(),
                });
            }
            Ok(InferredType::Key(index_identity))
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "for-comprehension context")]
fn infer_hir_for_comp(
    bindings: &[hir::expr::ForBinding],
    body: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let mut inner_locals = local_types.child(Vec::new());
    for binding in bindings {
        // Every loop variable is a key of its axis. Coordinate arithmetic
        // goes through coord(), integer use of a Fin key through to_int().
        let var_type = match &binding.index {
            hir::expr::ForBindingIndex::Named(index) => {
                let index_identity = InferredIndex::from_resolved(index.value.clone());
                super::index_def_for_inferred(&index_identity, Some(dag), registry).ok_or_else(
                    || GraphcalError::UnknownIndex {
                        name: index_identity.name(),
                        src: src.clone(),
                        span: index.span.into(),
                    },
                )?;
                InferredType::Key(index_identity)
            }
            hir::expr::ForBindingIndex::Finite { cardinality, span } => {
                let form =
                    resolve_hir_nat_form(cardinality, local_types.generic_substitutions(), src)?;
                InferredType::Key(
                    InferredIndex::from_finite_index_form(form)
                        .map_err(|err| finite_index_error(err, src, *span))?,
                )
            }
        };
        inner_locals.bind(binding.local.id, var_type);
    }
    let mut result = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &inner_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    for binding in bindings.iter().rev() {
        let index = match &binding.index {
            hir::expr::ForBindingIndex::Named(index) => {
                InferredIndex::from_resolved(index.value.clone())
            }
            hir::expr::ForBindingIndex::Finite { cardinality, span } => {
                let form =
                    resolve_hir_nat_form(cardinality, local_types.generic_substitutions(), src)?;
                InferredIndex::from_finite_index_form(form)
                    .map_err(|err| finite_index_error(err, src, *span))?
            }
        };
        result = InferredType::Indexed {
            element: Box::new(result),
            index,
        };
    }
    Ok(result)
}

fn finite_axis_form(
    index: &InferredIndex,
    declared_definition: Option<&crate::registry::types::IndexDef>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Option<NatPolyForm>, GraphcalError> {
    match index.type_ref() {
        IndexTypeRef::Finite(reference) => Ok(Some(reference.form())),
        IndexTypeRef::Declared(reference) => {
            let definition = declared_definition.ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "declared indexed axis `{}` has no semantic index definition",
                        reference.resolved()
                    ),
                    src,
                    DiagnosticAnchor::Source(span),
                )
            })?;
            Ok(definition
                .finite_index_size()
                .map(NatPolyForm::from_constant))
        }
    }
}

#[cfg(test)]
mod finite_axis_form_tests {
    use super::*;
    use crate::dag_id::DagId;
    use crate::syntax::index_name::{IndexName, ResolvedIndexName};
    use std::path::Path;

    #[test]
    fn structural_finite_axis_does_not_require_a_registry_definition() {
        let source = NamedSource::new("test.gcl", Arc::new(String::new()));
        let form = NatPolyForm::from_constant(5);
        let index = InferredIndex::from_finite_index_form(form.clone()).unwrap();

        assert_eq!(
            finite_axis_form(&index, None, &source, Span::new(0, 0)).unwrap(),
            Some(form)
        );
    }

    #[test]
    fn declared_axis_without_a_semantic_definition_is_an_internal_error() {
        let source = NamedSource::new("test.gcl", Arc::new("values[key]".to_string()));
        let owner = DagId::from_virtual_relative_path(Path::new("test.gcl")).unwrap();
        let resolved = ResolvedIndexName::from_def(owner, IndexName::expect_valid("Missing"));
        let index = InferredIndex::from_resolved(resolved);

        let error = finite_axis_form(&index, None, &source, Span::new(7, 3)).unwrap_err();
        match error {
            GraphcalError::InternalError { message, .. } => assert!(
                message.contains(
                    "declared indexed axis `test.Missing` has no semantic index definition"
                ),
                "{message}"
            ),
            other => panic!("expected internal error, got {other:?}"),
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "index-access context")]
fn infer_hir_index_access(
    expr: &hir::Expr,
    inner: &hir::Expr,
    args: &[hir::expr::IndexArg],
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let mut current = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    for arg in args {
        let InferredType::Indexed { element, index } = current else {
            return Err(GraphcalError::EvalError {
                message: "indexing a non-indexed value".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        };
        match arg {
            hir::expr::IndexArg::Variant(variant) => {
                check_index_override_dependency(
                    local_types,
                    dag,
                    owner_decl_name,
                    &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                    IndexNominalUse::Label(variant.variant.variant()),
                )?;
                let arg_index = InferredIndex::from_resolved(variant.variant.index().clone());
                if arg_index != index {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index.name(),
                        found: arg_index.name(),
                        src: src.clone(),
                        span: variant.path_span().into(),
                    });
                }
            }
            hir::expr::IndexArg::Var(local) => {
                let Some(var_type) = local_types.get(local.value) else {
                    return Err(GraphcalError::UnknownLocalRef {
                        name: format!("#{}", local.value.index()),
                        src: src.clone(),
                        span: local.span.into(),
                    });
                };
                match var_type {
                    // Loop variables are keys of their axes: accept on axis
                    // identity, with Fin widening (`N <= M`).
                    InferredType::Key(key_index) => {
                        let axis_form = finite_axis_form(
                            &index,
                            super::index_def_for_inferred(&index, Some(dag), registry),
                            src,
                            local.span,
                        )?;
                        let accepted = match (key_index.finite_index_form(), &axis_form) {
                            (Some(key_form), Some(axis_form)) => key_form.is_leq(axis_form),
                            _ => key_index == &index,
                        };
                        if !accepted {
                            return Err(GraphcalError::IndexMismatch {
                                expected: index.name(),
                                found: key_index.name(),
                                src: src.clone(),
                                span: local.span.into(),
                            });
                        }
                    }
                    InferredType::Quantity(_) => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "quantity local cannot index into coordinate index `{}`; use that coordinate index's loop variable",
                                index.name()
                            ),
                            src: src.clone(),
                            span: local.span.into(),
                        });
                    }
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "`#{}` is not a valid index variable",
                                local.value.index()
                            ),
                            src: src.clone(),
                            span: local.span.into(),
                        });
                    }
                }
            }
            hir::expr::IndexArg::Expr(index_expr) => {
                let expr_type = infer_hir_type(
                    index_expr,
                    owner_decl_name,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?;
                let index_form = finite_axis_form(
                    &index,
                    super::index_def_for_inferred(&index, Some(dag), registry),
                    src,
                    index_expr.span,
                )?;
                // A key-typed expression selects by axis identity: exact for
                // named and coordinate axes, widening (`N <= M`) for Fin.
                if let InferredType::Key(key_index) = &expr_type {
                    let accepted = match (key_index.finite_index_form(), &index_form) {
                        (Some(key_form), Some(axis_form)) => key_form.is_leq(axis_form),
                        _ => *key_index == index,
                    };
                    if !accepted {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index.name(),
                            found: key_index.name(),
                            src: src.clone(),
                            span: index_expr.span.into(),
                        });
                    }
                    current = *element;
                    continue;
                }
                let Some(index_form) = index_form else {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "integer expression cannot index into non-finite-index index `{}`",
                            index.name()
                        ),
                        src: src.clone(),
                        span: index_expr.span.into(),
                    });
                };
                match expr_type {
                    InferredType::Int => {
                        // Runtime-checked Int indexing was removed: only a
                        // statically discharged constant selects implicitly;
                        // a runtime Int goes through the explicit fin_key().
                        if try_const_int(index_expr).is_none() {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "a runtime Int cannot index `{}` implicitly; write \
                                     `fin_key({}, ...)` to make the range check explicit",
                                    index.name(),
                                    index.name(),
                                ),
                                src: src.clone(),
                                span: index_expr.span.into(),
                            });
                        }
                        check_constant_finite_index_index(index_expr, &index_form, src)?;
                    }
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "index expression must be an integer type, got {}",
                                format_inferred_type(&expr_type, registry)
                            ),
                            src: src.clone(),
                            span: index_expr.span.into(),
                        });
                    }
                }
            }
        }
        current = *element;
    }
    Ok(current)
}

fn check_constant_finite_index_index(
    index_expr: &hir::Expr,
    index_form: &NatPolyForm,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let Some(index) = try_const_int(index_expr) else {
        return Ok(());
    };
    let Ok(index_u64) = u64::try_from(index) else {
        return Err(GraphcalError::EvalError {
            message: format!("index expression evaluated to negative value: {index}"),
            src: src.clone(),
            span: index_expr.span.into(),
        });
    };
    if !index_form.is_constant() {
        return Ok(());
    }
    let size = index_form.constant();
    if index_u64 >= size {
        return Err(GraphcalError::EvalError {
            message: format!(
                "index {index} out of bounds for Fin({})",
                index_form.format()
            ),
            src: src.clone(),
            span: index_expr.span.into(),
        });
    }
    Ok(())
}

/// Reject `(expr -> u) -> v` and its timezone-display analogues (#648 B2).
///
/// The surface grammar already rejects bare chaining (`expr -> u -> v`), but
/// parentheses used to smuggle a nested conversion through; only the outermost
/// target ever took effect, so the inner one is either a typo or dead code.
/// Parens are flattened in HIR, so a direct nested `Convert`/`DisplayTimezone`
/// operand is exactly the parenthesized-chain shape.
fn reject_nested_conversion(
    inner: &hir::Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if matches!(
        inner.kind,
        hir::ExprKind::Convert { .. } | hir::ExprKind::DisplayTimezone { .. }
    ) {
        return Err(GraphcalError::NestedConversion {
            src: src.clone(),
            span: inner.span.into(),
        });
    }
    Ok(())
}

#[expect(clippy::too_many_arguments, reason = "conversion expression context")]
fn infer_hir_convert(
    inner: &hir::Expr,
    target: &hir::ResolvedUnitExpr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    reject_nested_conversion(inner, src)?;
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    // `->` distributes element-wise over indexed values (#648 U1): the quantity
    // element dimension must match the target. Multi-axis values unwrap
    // through each nested Indexed layer.
    let mut element = &inner_type;
    while let InferredType::Indexed {
        element: nested, ..
    } = element
    {
        element = nested;
    }
    let expr_dim = match element.complex_dimension() {
        Some(dimension) => dimension.clone(),
        None => expect_quantity(element, registry, src, inner.span)?,
    };
    let target_dim = rules::resolve_unit_dimension_or_diagnose(target, tir, src)?;

    if expr_dim != target_dim {
        return Err(GraphcalError::ConversionDimensionMismatch {
            target: registry.dimensions.format_dimension(&target_dim),
            expr_dim: registry.dimensions.format_dimension(&expr_dim),
            src: src.clone(),
            span: target.span.into(),
        });
    }

    Ok(inner_type)
}

#[expect(
    clippy::too_many_arguments,
    reason = "display timezone expression context"
)]
fn infer_hir_display_timezone(
    inner: &hir::Expr,
    timezone: &crate::registry::time_zone::IanaTimeZoneId,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    reject_nested_conversion(inner, src)?;
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if !matches!(&inner_type, InferredType::Datetime(_)) {
        return Err(GraphcalError::DimensionMismatch {
            expected: "Datetime".to_string(),
            found: format_inferred_type(&inner_type, registry),
            help: format!("timezone display `-> \"{timezone}\"` requires a Datetime expression"),
            src: src.clone(),
            span: inner.span.into(),
        });
    }
    Ok(inner_type)
}

fn resolved_type_field_key(
    owning_type: &ResolvedStructTypeName,
    constructor: &NominalConstructor,
    field: &FieldName,
) -> crate::tir::typed::ResolvedStructFieldTypeKey {
    crate::tir::typed::ResolvedStructFieldTypeKey {
        owning_type: owning_type.clone(),
        constructor: constructor.name(),
        field: field.clone(),
    }
}

fn generic_substitution_prefix(
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<GenericSubstitutions, GraphcalError> {
    if type_args.len() > type_def.generic_params().len() {
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects at most {} generic arguments, got {}",
                type_def.name(),
                type_def.generic_params().len(),
                type_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }

    let mut subs = GenericSubstitutions::default();
    for (param, arg) in type_def.generic_params().iter().zip(type_args) {
        match param.constraint() {
            TypeGenericConstraint::Dim => match arg {
                InferredGenericArg::Dim(dim) => {
                    subs.dims.insert(param.name().clone(), dim.clone());
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            TypeGenericConstraint::Index => match arg {
                InferredGenericArg::Index(index) if inferred_index_is_concrete(index) => {
                    subs.indexes
                        .insert(param.name().clone(), index.type_ref().clone());
                }
                InferredGenericArg::Index(index) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &index.to_string(),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            TypeGenericConstraint::Nat => match arg {
                InferredGenericArg::Nat(form) if form.is_constant() => {
                    subs.nats.insert(param.name().clone(), form.constant());
                }
                InferredGenericArg::Nat(form) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &form.format(),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
            TypeGenericConstraint::Type => match arg {
                InferredGenericArg::Type(type_expr) if inferred_type_is_concrete(type_expr) => {
                    subs.types.insert(param.name().clone(), type_expr.clone());
                }
                InferredGenericArg::Type(type_expr) => {
                    return Err(non_concrete_generic_argument(
                        param.name(),
                        &format!("{type_expr:?}"),
                        src,
                        span,
                    ));
                }
                _ => return Err(generic_arg_internal_sort_error(param, src, span)),
            },
        }
    }
    Ok(subs)
}

fn concrete_generic_substitutions(
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ConcreteGenericSubstitutions, GraphcalError> {
    if type_args.len() != type_def.generic_params().len() {
        return Err(GraphcalError::EvalError {
            message: format!(
                "concrete type `{}` requires exactly {} generic arguments, got {}",
                type_def.name(),
                type_def.generic_params().len(),
                type_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    generic_substitution_prefix(type_def, type_args, src, span).map(ConcreteGenericSubstitutions)
}

fn inferred_index_is_concrete(index: &InferredIndex) -> bool {
    index
        .finite_index_form()
        .is_none_or(|form| form.is_constant())
}

fn inferred_type_is_concrete(inferred: &InferredType) -> bool {
    match inferred {
        InferredType::Key(index) | InferredType::IndexArg(index) => {
            inferred_index_is_concrete(index)
        }
        InferredType::Struct(_, args) => args.iter().all(|arg| match arg {
            InferredGenericArg::Dim(_) => true,
            InferredGenericArg::Index(index) => inferred_index_is_concrete(index),
            InferredGenericArg::Nat(form) => form.is_constant(),
            InferredGenericArg::Type(type_expr) => inferred_type_is_concrete(type_expr),
        }),
        InferredType::Indexed { element, index } => {
            inferred_index_is_concrete(index) && inferred_type_is_concrete(element)
        }
        InferredType::Quantity(_)
        | InferredType::Complex(_)
        | InferredType::Bool
        | InferredType::Int
        | InferredType::Datetime(_) => true,
    }
}

fn non_concrete_generic_argument(
    parameter: &GenericParamName,
    argument: &str,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::EvalError {
        message: format!("generic argument `{argument}` for `{parameter}` is not concrete"),
        src: src.clone(),
        span: span.into(),
    }
}

fn generic_arg_internal_sort_error(
    param: &crate::hir::NominalGenericParam,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> GraphcalError {
    GraphcalError::InternalError {
        message: format!(
            "generic argument for `{}` does not match its registered sort",
            param.name()
        ),
        src: src.clone(),
        span: span.into(),
    }
}

#[derive(Clone, Default)]
struct GenericSubstitutions {
    dims: HashMap<GenericParamName, Dimension>,
    indexes: HashMap<GenericParamName, IndexTypeRef>,
    nats: HashMap<GenericParamName, u64>,
    types: HashMap<GenericParamName, InferredType>,
}

/// Complete, sort-checked, concrete bindings for one nominal application.
///
/// This wrapper can only be constructed after exact arity, sort, and
/// concreteness validation, so instantiated field-bound inference cannot be
/// called with a partial generic environment.
#[derive(Clone)]
struct ConcreteGenericSubstitutions(GenericSubstitutions);

impl ConcreteGenericSubstitutions {
    const fn bindings(&self) -> &GenericSubstitutions {
        &self.0
    }
}

fn substitute_resolved_type_with_type_params(
    resolved: &crate::tir::typed::ResolvedTypeExpr,
    subs: &GenericSubstitutions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    crate::tir::typed::substitute_resolved_type_with_types(
        resolved,
        &subs.dims,
        &subs.indexes,
        &subs.nats,
        &subs.types,
        src,
    )
}

fn substitute_resolved_generic_arg_with_type_params(
    resolved: &crate::tir::typed::ResolvedGenericArg,
    subs: &GenericSubstitutions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredGenericArg, GraphcalError> {
    crate::tir::typed::substitute_resolved_generic_arg(
        resolved,
        &subs.dims,
        &subs.indexes,
        &subs.nats,
        &subs.types,
        src,
    )
}

pub(in crate::tir::dim_check) fn resolved_field_type(
    owning_type: &ResolvedStructTypeName,
    constructor: &NominalConstructor,
    field: &FieldName,
    type_def: &NominalTypeDef,
    type_args: &[InferredGenericArg],
    dag: &crate::tir::typed::DagTIR,
    _registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<InferredType, GraphcalError> {
    let key = resolved_type_field_key(owning_type, constructor, field);
    let resolved =
        dag.semantic
            .type_defs
            .field_type(&key)
            .ok_or_else(|| GraphcalError::InternalError {
                message: format!(
                    "semantic type metadata missing field type for `{}.{}`",
                    constructor.name(),
                    field
                ),
                src: src.clone(),
                span: span.into(),
            })?;
    let subs = concrete_generic_substitutions(type_def, type_args, src, span)?;
    substitute_resolved_type_with_type_params(resolved, subs.bindings(), src)
}

#[derive(Clone, PartialEq, Eq)]
struct ConcreteStructApplicationKey {
    type_name: InferredStructType,
    type_args: Vec<InferredGenericArg>,
}

struct ConcreteStructApplication<'a> {
    key: ConcreteStructApplicationKey,
    type_def: &'a NominalTypeDef,
    substitutions: ConcreteGenericSubstitutions,
}

impl<'a> ConcreteStructApplication<'a> {
    fn new(
        type_name: &InferredStructType,
        type_args: &[InferredGenericArg],
        type_def: &'a NominalTypeDef,
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<Self, GraphcalError> {
        Ok(Self {
            key: ConcreteStructApplicationKey {
                type_name: type_name.clone(),
                type_args: type_args.to_vec(),
            },
            type_def,
            substitutions: concrete_generic_substitutions(type_def, type_args, src, span)?,
        })
    }
}

struct ConcreteObligationContext<'a> {
    dag: &'a crate::tir::typed::DagTIR,
    tir: &'a crate::tir::typed::TIR,
    registry: &'a SemanticRegistry,
    builtin_fns: &'a crate::registry::builtins::BuiltinFunctions,
    src: &'a NamedSource<Arc<String>>,
    span: Span,
    cancellation: &'a crate::cancellation::CancellationToken,
}

pub(in crate::tir::dim_check) fn validate_concrete_type_obligations(
    inferred: &InferredType,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
    span: Span,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<(), GraphcalError> {
    let ctx = ConcreteObligationContext {
        dag,
        tir,
        registry,
        builtin_fns,
        src,
        span,
        cancellation,
    };
    validate_concrete_type_obligations_inner(inferred, &ctx, &mut Vec::new())
}

fn validate_concrete_type_obligations_inner(
    inferred: &InferredType,
    ctx: &ConcreteObligationContext<'_>,
    stack: &mut Vec<ConcreteStructApplicationKey>,
) -> Result<(), GraphcalError> {
    ctx.cancellation.checkpoint()?;
    validate_inferred_materialized_shape(inferred, ctx.tir, ctx.src, ctx.span)?;
    match inferred {
        InferredType::Struct(type_name, type_args) => {
            for arg in type_args {
                if let InferredGenericArg::Type(type_arg) = arg {
                    validate_concrete_type_obligations_inner(type_arg, ctx, stack)?;
                }
            }
            let type_def = struct_type_def_for_inferred(type_name, Some(ctx.dag), ctx.registry)
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: type_name.to_string(),
                    src: ctx.src.clone(),
                    span: ctx.span.into(),
                })?;
            let application =
                ConcreteStructApplication::new(type_name, type_args, type_def, ctx.src, ctx.span)?;
            let field_types = validate_concrete_field_obligations(&application, ctx)?;

            if let Some(ancestor) = stack
                .iter()
                .find(|ancestor| ancestor.type_name == application.key.type_name)
            {
                if ancestor == &application.key {
                    return Ok(());
                }
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "recursive generic type `{}` changes its arguments; concrete field obligations cannot be discharged finitely",
                        application.key.type_name
                    ),
                    src: ctx.src.clone(),
                    span: ctx.span.into(),
                });
            }

            stack.push(application.key);
            for field_type in field_types {
                validate_concrete_type_obligations_inner(&field_type, ctx, stack)?;
            }
            stack.pop();
            Ok(())
        }
        InferredType::Indexed { element, index } => {
            validate_concrete_index(index, ctx)?;
            validate_concrete_type_obligations_inner(element, ctx, stack)
        }
        InferredType::Key(index) | InferredType::IndexArg(index) => {
            validate_concrete_index(index, ctx)
        }
        InferredType::Quantity(_)
        | InferredType::Complex(_)
        | InferredType::Bool
        | InferredType::Int
        | InferredType::Datetime(_) => Ok(()),
    }
}

fn validate_concrete_index(
    index: &InferredIndex,
    ctx: &ConcreteObligationContext<'_>,
) -> Result<(), GraphcalError> {
    let Some(form) = index.finite_index_form() else {
        return Ok(());
    };
    if !form.is_constant() {
        return Err(GraphcalError::EvalError {
            message: format!(
                "unresolved finite-index obligation `Fin({})`",
                form.format()
            ),
            src: ctx.src.clone(),
            span: ctx.span.into(),
        });
    }
    if form.constant() == 0 {
        return Err(GraphcalError::EvalError {
            message: "Fin(0) is invalid: finite indexes must contain at least one key".to_string(),
            src: ctx.src.clone(),
            span: ctx.span.into(),
        });
    }
    Ok(())
}

fn validate_concrete_field_obligations(
    application: &ConcreteStructApplication<'_>,
    ctx: &ConcreteObligationContext<'_>,
) -> Result<Vec<InferredType>, GraphcalError> {
    let Some(members) = application.type_def.union_members() else {
        return Ok(Vec::new());
    };
    members
        .iter()
        .flat_map(|member| member.fields().iter().map(move |field| (member, field)))
        .map(|(member, field)| {
            ctx.cancellation.checkpoint()?;
            let key =
                resolved_type_field_key(application.key.type_name.resolved(), member, field.name());
            let field_semantics = ctx.dag.semantic.type_defs.field(&key).ok_or_else(|| {
                GraphcalError::InternalError {
                    message: format!(
                        "semantic type metadata missing field `{}.{}`",
                        member.name(),
                        field.name()
                    ),
                    src: ctx.src.clone(),
                    span: ctx.span.into(),
                }
            })?;
            let field_type = substitute_resolved_type_with_type_params(
                field_semantics.resolved_type(),
                application.substitutions.bindings(),
                ctx.src,
            )?;
            if !field_semantics.domain_bounds().is_empty() {
                validate_instantiated_field_bounds(
                    application,
                    member,
                    field,
                    field_semantics.domain_bounds(),
                    &field_type,
                    ctx,
                )?;
            }
            Ok(field_type)
        })
        .collect()
}

fn validate_instantiated_field_bounds(
    application: &ConcreteStructApplication<'_>,
    member: &NominalConstructor,
    field: &crate::hir::NominalField,
    bounds: &[crate::tir::typed::ResolvedDomainBound],
    field_type: &InferredType,
    ctx: &ConcreteObligationContext<'_>,
) -> Result<(), GraphcalError> {
    let expected = super::super::expected_bound_from_inferred(field_type).ok_or_else(|| {
        GraphcalError::InvalidDomainTarget {
            type_kind: format_inferred_type(field_type, ctx.registry),
            src: bounds
                .first()
                .map_or_else(|| ctx.src.clone(), |bound| bound.src.clone()),
            span: bounds.first().map_or(ctx.span, |bound| bound.span).into(),
        }
    })?;
    let display_name = if member.name().as_str() == application.type_def.name().as_str() {
        format!("{}.{}", application.type_def.name(), field.name())
    } else {
        format!(
            "{}.{}.{}",
            application.type_def.name(),
            member.name(),
            field.name()
        )
    };

    let definition_dag = ctx
        .tir
        .dag_registry()
        .get(application.key.type_name.resolved().owner())
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "field-constraint owner `{}` has no checked DAG",
                application.key.type_name.resolved().owner()
            ),
            src: ctx.src.clone(),
            span: ctx.span.into(),
        })?;
    for bound in bounds {
        ctx.cancellation.checkpoint()?;
        let definition_types = definition_dag.build_declared_types(&bound.src)?;
        let locals = HirLocalTypes::root(ctx.cancellation, Some(application.substitutions.clone()));
        let inferred = infer_hir_type(
            &bound.value,
            None,
            &definition_types,
            &locals,
            definition_dag,
            ctx.tir,
            ctx.registry,
            ctx.builtin_fns,
            &bound.src,
        )?;
        super::super::check_one_bound_with_display_name(
            &display_name,
            bound,
            &inferred,
            &expected,
            ctx.registry,
            &bound.src,
        )?;
    }
    Ok(())
}

fn record_member(type_def: &NominalTypeDef) -> Option<&NominalConstructor> {
    let members = type_def.union_members()?;
    let [only] = members else {
        return None;
    };
    (only.name().as_str() == type_def.name().as_str()).then_some(only)
}

#[expect(clippy::too_many_arguments, reason = "field-access expression context")]
fn infer_hir_field_access(
    inner: &hir::Expr,
    field: &crate::syntax::span::Spanned<FieldName>,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let inner_type = infer_hir_type(
        inner,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let InferredType::Struct(type_name, type_args) = &inner_type else {
        return Err(GraphcalError::NotAStruct {
            name: format_inferred_type(&inner_type, registry),
            src: src.clone(),
            span: inner.span.into(),
        });
    };
    check_type_override_dependency(
        local_types,
        dag,
        owner_decl_name,
        type_name.resolved(),
        TypeNominalUse::Field(&field.value),
    )?;
    let type_def =
        struct_type_def_for_inferred(type_name, Some(dag), registry).ok_or_else(|| {
            GraphcalError::UnknownStructType {
                name: type_name.to_string(),
                src: src.clone(),
                span: inner.span.into(),
            }
        })?;
    let member = record_member(type_def).ok_or_else(|| {
        let detail = if type_def.is_required() {
            format!("required type `{}` has no fields", type_name.name())
        } else {
            format!(
                "union type `{}` (use `match` to access fields)",
                type_name.name()
            )
        };
        GraphcalError::NotAStruct {
            name: detail,
            src: src.clone(),
            span: inner.span.into(),
        }
    })?;
    if !member
        .fields()
        .iter()
        .any(|field_def| field_def.name() == &field.value)
    {
        return Err(GraphcalError::UnknownField {
            type_name: type_name.name().clone(),
            field_name: field.value.clone(),
            src: src.clone(),
            span: field.span.into(),
        });
    }
    resolved_field_type(
        type_name.resolved(),
        member,
        &field.value,
        type_def,
        type_args,
        dag,
        registry,
        src,
        field.span,
    )
}

fn infer_hir_generic_type_arg(
    type_expr: &hir::TypeExpr,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    substitutions: Option<&ConcreteGenericSubstitutions>,
) -> Result<InferredType, GraphcalError> {
    match &type_expr.kind {
        hir::TypeExprKind::Builtin(hir::BuiltinType::Dimensionless) => {
            Ok(InferredType::Quantity(Dimension::dimensionless()))
        }
        hir::TypeExprKind::Builtin(hir::BuiltinType::Bool) => Ok(InferredType::Bool),
        hir::TypeExprKind::Builtin(hir::BuiltinType::Int) => Ok(InferredType::Int),
        hir::TypeExprKind::Builtin(hir::BuiltinType::Datetime(scale)) => {
            Ok(InferredType::Datetime(*scale))
        }
        hir::TypeExprKind::DimExpr(dim_expr) => {
            infer_hir_dim_expr_arg(dim_expr, tir, src, substitutions).map(InferredType::Quantity)
        }
        hir::TypeExprKind::Complex(dimension) => match dimension {
            hir::DimArg::Dimensionless(_) => Ok(InferredType::Complex(Dimension::dimensionless())),
            hir::DimArg::Expr(dim_expr) => {
                infer_hir_dim_expr_arg(dim_expr, tir, src, substitutions).map(InferredType::Complex)
            }
        },
        hir::TypeExprKind::Index(index) => Ok(InferredType::IndexArg(
            inferred_index_from_type_arg(index, src, substitutions)?,
        )),
        hir::TypeExprKind::Key(index) => Ok(InferredType::Key(inferred_index_from_type_arg(
            index,
            src,
            substitutions,
        )?)),
        hir::TypeExprKind::Struct(name) => Ok(InferredType::Struct(
            InferredStructType::from_resolved(name.value.clone()),
            vec![],
        )),
        hir::TypeExprKind::GenericTypeParam(param) => substitutions
            .and_then(|substitutions| substitutions.bindings().types.get(&param.value.name))
            .cloned()
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "generic type parameter `{}` is not concretely bound",
                    param.value.name
                ),
                src: src.clone(),
                span: param.span.into(),
            }),
        hir::TypeExprKind::TypeApplication { name, generic_args } => {
            let type_def = dag
                .semantic
                .type_defs
                .struct_types
                .get(&name.value)
                .ok_or_else(|| GraphcalError::InternalError {
                    message: format!(
                        "semantic type metadata missing generic type `{}`",
                        name.value
                    ),
                    src: src.clone(),
                    span: name.span.into(),
                })?;
            Ok(InferredType::Struct(
                InferredStructType::from_resolved(name.value.clone()),
                resolve_applied_generic_args(
                    &name.value,
                    type_def,
                    generic_args,
                    dag,
                    tir,
                    registry,
                    src,
                    name.span,
                    substitutions,
                )?,
            ))
        }
        hir::TypeExprKind::Indexed { base, indexes } => {
            let mut result =
                infer_hir_generic_type_arg(base, dag, tir, registry, src, substitutions)?;
            for index in indexes.iter().rev() {
                let inferred_index = inferred_index_from_type_arg(index, src, substitutions)?;
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: inferred_index,
                };
            }
            Ok(result)
        }
    }
}

fn infer_hir_sorted_generic_arg(
    arg: &hir::GenericArg,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    substitutions: Option<&ConcreteGenericSubstitutions>,
) -> Result<InferredGenericArg, GraphcalError> {
    match arg {
        hir::GenericArg::Dim(hir::DimArg::Dimensionless(_)) => {
            Ok(InferredGenericArg::Dim(Dimension::dimensionless()))
        }
        hir::GenericArg::Dim(hir::DimArg::Expr(dim_expr)) => {
            infer_hir_dim_expr_arg(dim_expr, tir, src, substitutions).map(InferredGenericArg::Dim)
        }
        hir::GenericArg::Index(index) => {
            inferred_index_from_type_arg(index, src, substitutions).map(InferredGenericArg::Index)
        }
        hir::GenericArg::Nat(nat) => {
            resolve_hir_nat_form(nat, substitutions, src).map(InferredGenericArg::Nat)
        }
        hir::GenericArg::Type(type_expr) => {
            infer_hir_generic_type_arg(type_expr, dag, tir, registry, src, substitutions)
                .map(InferredGenericArg::Type)
        }
    }
}

fn inferred_index_from_type_arg(
    index: &hir::IndexRef,
    src: &NamedSource<Arc<String>>,
    substitutions: Option<&ConcreteGenericSubstitutions>,
) -> Result<InferredIndex, GraphcalError> {
    match index {
        hir::IndexRef::Concrete(name) => Ok(InferredIndex::from_resolved(name.value.clone())),
        hir::IndexRef::GenericParam(param) => substitutions
            .and_then(|substitutions| substitutions.bindings().indexes.get(&param.value.name))
            .cloned()
            .map(InferredIndex::from_ref)
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "generic index parameter `{}` is not concretely bound",
                    param.value.name
                ),
                src: src.clone(),
                span: param.span.into(),
            }),
        hir::IndexRef::Finite(nat_expr) => {
            let form = resolve_hir_nat_form(nat_expr, substitutions, src)?;
            InferredIndex::from_finite_index_form(form)
                .map_err(|err| finite_index_error(err, src, nat_expr.span()))
        }
    }
}

fn infer_hir_dim_expr_arg(
    dim_expr: &hir::DimExpr,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    substitutions: Option<&ConcreteGenericSubstitutions>,
) -> Result<Dimension, GraphcalError> {
    dim_expr
        .terms
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, item| {
            let (dim, power, span) = match &item.term.target {
                hir::DimTermTarget::Dimension(target) => {
                    let dim = tir.dimension(&target.value).cloned().ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: NamePath::from(target.value.atom().clone()),
                            src: src.clone(),
                            span: target.span.into(),
                        }
                    })?;
                    (dim, item.term.power, item.term.span)
                }
                hir::DimTermTarget::GenericParam(param) => {
                    let dim = substitutions
                        .and_then(|substitutions| {
                            substitutions.bindings().dims.get(&param.value.name)
                        })
                        .cloned()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!(
                                "generic dimension parameter `{}` is not concretely bound",
                                param.value.name
                            ),
                            src: src.clone(),
                            span: param.span.into(),
                        })?;
                    (dim, item.term.power, item.term.span)
                }
            };
            let powered = dim
                .pow(power)
                .map_err(|_| GraphcalError::DimensionOverflow {
                    src: src.clone(),
                    span: span.into(),
                })?;
            match item.op {
                crate::desugar::desugared_ast::MulDivOp::Mul => {
                    acc.checked_mul(&powered)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })
                }
                crate::desugar::desugared_ast::MulDivOp::Div => {
                    acc.checked_div(&powered)
                        .map_err(|_| GraphcalError::DimensionOverflow {
                            src: src.clone(),
                            span: span.into(),
                        })
                }
            }
        })
}

#[expect(
    clippy::too_many_arguments,
    reason = "constructor-call expression context"
)]
fn infer_hir_constructor_call(
    expr: &hir::Expr,
    callee: &crate::syntax::span::Spanned<ResolvedConstructorName>,
    constructor_generic_args: &[hir::GenericArg],
    fields: &[hir::expr::FieldInit],
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let target = dag
        .semantic
        .constructor_refs
        .constructor_defs
        .get(&callee.value)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!(
                "semantic TIR missing constructor call target for `{}`",
                callee.value
            ),
            src: src.clone(),
            span: callee.span.into(),
        })?;
    check_type_override_dependency(
        local_types,
        dag,
        owner_decl_name,
        &target.owning_type,
        TypeNominalUse::Constructor(&callee.value),
    )?;
    constructor_generic_args.iter().try_for_each(|arg| {
        check_hir_generic_arg_override_dependencies(arg, local_types, dag, owner_decl_name)
    })?;
    let type_def = &target.type_def;
    let variant = &target.variant;
    let owning_type_identity = InferredStructType::from_resolved(target.owning_type.clone());
    let owning_type_name = type_def.name();

    let resolved_type_args = resolve_applied_generic_args(
        &target.owning_type,
        type_def,
        constructor_generic_args,
        dag,
        tir,
        registry,
        src,
        callee.span,
        local_types.generic_substitutions(),
    )?;

    let def_field_names: std::collections::HashSet<&str> = variant
        .fields()
        .iter()
        .map(|field| field.name().as_str())
        .collect();
    let provided_names: Vec<&str> = fields
        .iter()
        .map(|field| field.name.value.as_str())
        .collect();
    let mut seen_fields = std::collections::HashSet::new();
    for field in fields {
        if !seen_fields.insert(field.name.value.clone()) {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "duplicate field `{}` in constructor `{}`",
                    field.name.value,
                    variant.name()
                ),
                src: src.clone(),
                span: field.name.span.into(),
            });
        }
    }
    let extra: Vec<FieldName> = provided_names
        .iter()
        .filter(|name| !def_field_names.contains(**name))
        .map(|name| FieldName::expect_valid(*name))
        .collect();
    if !extra.is_empty() {
        return Err(GraphcalError::ExtraFields {
            type_name: owning_type_name,
            extra,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let provided_set: std::collections::HashSet<&str> = provided_names.iter().copied().collect();
    let missing: Vec<FieldName> = variant
        .fields()
        .iter()
        .filter(|field| !provided_set.contains(field.name().as_str()))
        .map(|field| field.name().clone())
        .collect();
    if !missing.is_empty() {
        return Err(GraphcalError::MissingFields {
            type_name: owning_type_name,
            missing,
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    for field_init in fields {
        let field_def = variant
            .fields()
            .iter()
            .find(|field| field.name() == &field_init.name.value)
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: unknown field `{}` in constructor `{}`",
                    field_init.name.value,
                    variant.name()
                ),
                src: src.clone(),
                span: field_init.name.span.into(),
            })?;
        let value_type = infer_hir_type(
            &field_init.value,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        let expected = resolved_field_type(
            &target.owning_type,
            variant,
            field_def.name(),
            type_def,
            &resolved_type_args,
            dag,
            registry,
            src,
            field_init.name.span,
        )?;
        if value_type != expected {
            let (expected, found) =
                format_distinct_inferred_types(&expected, &value_type, registry);
            return Err(GraphcalError::FieldDimensionMismatch {
                type_name: owning_type_name,
                field_name: field_init.name.value.clone(),
                expected,
                found,
                src: src.clone(),
                span: field_init.name.span.into(),
            });
        }
    }

    Ok(InferredType::Struct(
        owning_type_identity,
        resolved_type_args,
    ))
}

pub(in crate::tir::dim_check) fn resolve_concrete_generic_args(
    owning_type: &ResolvedStructTypeName,
    type_def: &NominalTypeDef,
    applied_generic_args: &[hir::GenericArg],
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Vec<crate::registry::declared_type::DeclaredGenericArg>, GraphcalError> {
    resolve_applied_generic_args(
        owning_type,
        type_def,
        applied_generic_args,
        dag,
        tir,
        registry,
        src,
        span,
        None,
    )
    .map(|args| {
        args.iter()
            .map(crate::registry::declared_type::DeclaredGenericArg::from)
            .collect()
    })
}

fn resolve_applied_generic_args(
    owning_type: &ResolvedStructTypeName,
    type_def: &NominalTypeDef,
    applied_generic_args: &[hir::GenericArg],
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    span: Span,
    substitutions: Option<&ConcreteGenericSubstitutions>,
) -> Result<Vec<InferredGenericArg>, GraphcalError> {
    if applied_generic_args.is_empty() && type_def.generic_params().is_empty() {
        return Ok(Vec::new());
    }
    let total_params = type_def.generic_params().len();
    let required_count = type_def
        .generic_params()
        .iter()
        .rposition(|param| param.default().is_none())
        .map_or(0, |index| index.saturating_add(1));
    if applied_generic_args.len() < required_count || applied_generic_args.len() > total_params {
        let hint = if required_count == total_params {
            format!("{total_params}")
        } else {
            format!("{required_count}..{total_params}")
        };
        return Err(GraphcalError::EvalError {
            message: format!(
                "type `{}` expects {hint} generic argument(s), got {}",
                type_def.name(),
                applied_generic_args.len()
            ),
            src: src.clone(),
            span: span.into(),
        });
    }
    let mut args = Vec::with_capacity(total_params);
    for (param, arg) in type_def.generic_params().iter().zip(applied_generic_args) {
        let inferred = infer_hir_sorted_generic_arg(arg, dag, tir, registry, src, substitutions)?;
        let matches_sort = matches!(
            (param.constraint(), &inferred),
            (TypeGenericConstraint::Dim, InferredGenericArg::Dim(_))
                | (TypeGenericConstraint::Index, InferredGenericArg::Index(_))
                | (TypeGenericConstraint::Nat, InferredGenericArg::Nat(_))
                | (TypeGenericConstraint::Type, InferredGenericArg::Type(_))
        );
        if !matches_sort {
            return Err(generic_arg_internal_sort_error(param, src, arg.span()));
        }
        args.push(inferred);
    }
    for param in type_def
        .generic_params()
        .iter()
        .skip(applied_generic_args.len())
    {
        let resolved_default = &dag
            .semantic
            .type_defs
            .generic_defaults
            .get(&(owning_type.clone(), param.name().clone()))
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!(
                    "internal: generic parameter `{}` has no default",
                    param.name()
                ),
                src: src.clone(),
                span: span.into(),
            })?
            .resolved;
        let subs = generic_substitution_prefix(type_def, &args, src, span)?;
        args.push(substitute_resolved_generic_arg_with_type_params(
            resolved_default,
            &subs,
            src,
        )?);
    }
    Ok(args)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MapLiteralVariantKey {
    Declared(ResolvedIndexVariant),
    Finite { form: NatPolyForm, position: u64 },
}

impl MapLiteralVariantKey {
    fn entry_key(&self) -> IndexEntryKey {
        match self {
            Self::Declared(resolved) => IndexEntryKey::named(resolved.variant().clone()),
            Self::Finite { position, .. } => IndexEntryKey::position(*position),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Declared(resolved) => resolved.to_string(),
            Self::Finite { form, position } => format!("Fin({}).#{position}", form.format()),
        }
    }
}

#[derive(Debug, Clone)]
struct MapLiteralAxis {
    index: InferredIndex,
    entry_keys: Vec<IndexEntryKey>,
}

/// Checked size of a map literal's Cartesian key space.
///
/// Construction performs checked multiplication, so downstream coverage logic
/// cannot accidentally compare against a wrapped cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MapCoverageCardinality(usize);

impl MapCoverageCardinality {
    fn checked_from_axes(
        axes: &[Vec<MapLiteralVariantKey>],
        src: &NamedSource<Arc<String>>,
        span: Span,
    ) -> Result<Self, GraphcalError> {
        axes.iter()
            .try_fold(1_usize, |product, axis| {
                product
                    .checked_mul(axis.len())
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "map literal key-space cardinality exceeds supported size"
                            .to_string(),
                        src: src.clone(),
                        span: span.into(),
                    })
            })
            .map(Self)
    }

    const fn get(self) -> usize {
        self.0
    }
}

/// Find one absent Cartesian tuple without materializing the product.
///
/// Among the first `provided.len() + 1` tuples at least one must be absent, so
/// this walk is bounded by source-sized input even when the declared product is
/// enormous.
fn first_missing_map_tuple(
    axes: &[Vec<MapLiteralVariantKey>],
    provided: &std::collections::HashSet<Vec<MapLiteralVariantKey>>,
) -> Option<Vec<MapLiteralVariantKey>> {
    let mut offsets = vec![0_usize; axes.len()];
    for _ in 0..=provided.len() {
        let candidate = axes
            .iter()
            .zip(&offsets)
            .map(|(axis, offset)| axis[*offset].clone())
            .collect::<Vec<_>>();
        if !provided.contains(&candidate) {
            return Some(candidate);
        }

        let mut advanced = false;
        for axis in (0..axes.len()).rev() {
            offsets[axis] = offsets[axis].checked_add(1)?;
            if offsets[axis] < axes[axis].len() {
                advanced = true;
                break;
            }
            offsets[axis] = 0;
        }
        if !advanced {
            return None;
        }
    }
    None
}

impl MapLiteralAxis {
    fn variant_key(&self, key: IndexEntryKey) -> Result<MapLiteralVariantKey, IndexEntryKey> {
        match (self.index.type_ref(), key) {
            (IndexTypeRef::Declared(reference), IndexEntryKey::Named(variant)) => {
                Ok(MapLiteralVariantKey::Declared(ResolvedIndexVariant::new(
                    reference.resolved().clone(),
                    variant,
                )))
            }
            (IndexTypeRef::Finite(reference), IndexEntryKey::Position(position)) => {
                Ok(MapLiteralVariantKey::Finite {
                    form: reference.form(),
                    position,
                })
            }
            (_, incompatible) => Err(incompatible),
        }
    }
}

fn inferred_index_for_hir_map_key(
    key: &hir::expr::MapEntryKey,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredIndex, GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => Ok(InferredIndex::from_resolved(
            variant.variant.index().clone(),
        )),
        hir::expr::MapEntryKey::FinitePosition { size, position } => {
            InferredIndex::from_finite_index_form(NatPolyForm::from_constant(*size))
                .map_err(|err| finite_index_error(err, src, position.span))
        }
    }
}

fn hir_map_entry_key(key: &hir::expr::MapEntryKey) -> IndexEntryKey {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => {
            IndexEntryKey::named(variant.variant.variant().clone())
        }
        hir::expr::MapEntryKey::FinitePosition { position, .. } => {
            IndexEntryKey::position(position.value)
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "map literal expression context")]
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive validation of map literal entries"
)]
fn infer_hir_map_literal(
    expr: &hir::Expr,
    entries: &[hir::expr::MapEntry],
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    for entry in entries {
        for key in &entry.keys {
            if let hir::expr::MapEntryKey::IndexVariant(variant) = key {
                check_index_override_dependency(
                    local_types,
                    dag,
                    owner_decl_name,
                    &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                    IndexNominalUse::Label(variant.variant.variant()),
                )?;
            }
        }
    }
    let Some(first_entry) = entries.first() else {
        return Err(GraphcalError::EvalError {
            message: "empty map literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        });
    };
    let arity = first_entry.keys.len();
    for entry in entries.iter().skip(1) {
        if entry.keys.len() != arity {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "map literal entries have inconsistent key arity: expected {arity}, found {}",
                    entry.keys.len()
                ),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
    }

    let mut axes = Vec::with_capacity(arity);
    for key in &first_entry.keys {
        let index = inferred_index_for_hir_map_key(key, src)?;
        let idx_def =
            super::index_def_for_inferred(&index, Some(dag), registry).ok_or_else(|| {
                GraphcalError::UnknownIndex {
                    name: index.name(),
                    src: src.clone(),
                    span: expr.span.into(),
                }
            })?;
        if idx_def.is_coordinate() {
            return Err(GraphcalError::EvalError {
                message: format!(
                    "coordinate index `{}` cannot be used as a map/table literal key; use a `for` comprehension instead",
                    index.name()
                ),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        axes.push(MapLiteralAxis {
            index,
            entry_keys: idx_def.entry_keys(),
        });
    }
    for entry in entries.iter().skip(1) {
        for (i, key) in entry.keys.iter().enumerate() {
            let key_index = inferred_index_for_hir_map_key(key, src)?;
            if key_index != axes[i].index {
                return Err(GraphcalError::IndexMismatch {
                    expected: axes[i].index.name(),
                    found: key_index.name(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
        }
    }

    let incompatible_key_error = |key: IndexEntryKey| GraphcalError::EvalError {
        message: format!("map entry key `{key}` does not match its index category"),
        src: src.clone(),
        span: expr.span.into(),
    };
    let axes_variant_keys: Vec<Vec<MapLiteralVariantKey>> = axes
        .iter()
        .map(|axis| {
            axis.entry_keys
                .iter()
                .cloned()
                .map(|key| axis.variant_key(key).map_err(&incompatible_key_error))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_cardinality =
        MapCoverageCardinality::checked_from_axes(&axes_variant_keys, src, expr.span)?;
    let mut provided_tuples = std::collections::HashSet::new();
    for entry in entries {
        local_types.checkpoint()?;
        let tuple: Vec<MapLiteralVariantKey> = entry
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let entry_key = hir_map_entry_key(key);
                if !axes[i].entry_keys.contains(&entry_key) {
                    return match (arity, entry_key) {
                        (1, extra) => Err(GraphcalError::ExtraVariants {
                            index_name: axes[0].index.name(),
                            extra: vec![extra],
                            src: src.clone(),
                            span: expr.span.into(),
                        }),
                        (_, IndexEntryKey::Named(variant_name)) => {
                            Err(GraphcalError::UnknownVariant {
                                index_name: axes[i].index.name(),
                                variant_name,
                                src: src.clone(),
                                span: expr.span.into(),
                            })
                        }
                        (_, IndexEntryKey::Position(position)) => Err(GraphcalError::EvalError {
                            message: format!(
                                "position #{position} is outside index `{}`",
                                axes[i].index.name()
                            ),
                            src: src.clone(),
                            span: expr.span.into(),
                        }),
                    };
                }
                axes[i]
                    .variant_key(entry_key)
                    .map_err(&incompatible_key_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !provided_tuples.insert(tuple) {
            return Err(GraphcalError::EvalError {
                message: "duplicate map literal entry".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            });
        }
    }

    if provided_tuples.len() < expected_cardinality.get() {
        if arity == 1 {
            let missing = axes_variant_keys[0]
                .iter()
                .filter(|key| !provided_tuples.contains(&vec![(*key).clone()]))
                .map(MapLiteralVariantKey::entry_key)
                .collect();
            return Err(GraphcalError::MissingVariants {
                index_name: axes[0].index.name(),
                missing,
                src: src.clone(),
                span: expr.span.into(),
            });
        }
        let first_missing = first_missing_map_tuple(&axes_variant_keys, &provided_tuples)
            .ok_or_else(|| GraphcalError::InternalError {
                message: "map coverage count and tuple membership disagree".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            })?;
        let witness = first_missing
            .iter()
            .map(MapLiteralVariantKey::display)
            .collect::<Vec<_>>()
            .join(", ");
        let missing_count = expected_cardinality
            .get()
            .checked_sub(provided_tuples.len())
            .ok_or_else(|| GraphcalError::InternalError {
                message: "map coverage cardinality underflow".to_string(),
                src: src.clone(),
                span: expr.span.into(),
            })?;
        return Err(GraphcalError::EvalError {
            message: format!(
                "non-exhaustive map literal: missing {missing_count} entries; first missing entry is ({witness})"
            ),
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let first_type = infer_hir_type(
        &first_entry.value,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if let InferredType::Indexed { index, .. } = &first_type {
        let inner_is_label = super::index_def_for_inferred(index, Some(dag), registry)
            .is_some_and(|def| !def.is_coordinate());
        if inner_is_label {
            return Err(GraphcalError::EvalError {
                message: "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals".to_string(),
                src: src.clone(),
                span: first_entry.value.span.into(),
            });
        }
    }
    for entry in entries.iter().skip(1) {
        let entry_type = infer_hir_type(
            &entry.value,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        if entry_type != first_type {
            return Err(GraphcalError::DimensionMismatchInAnnotation {
                declared: format_inferred_type(&first_type, registry),
                inferred: format_inferred_type(&entry_type, registry),
                src: src.clone(),
                span: entry.value.span.into(),
            });
        }
    }
    let mut result = first_type;
    for axis in axes.iter().rev() {
        result = InferredType::Indexed {
            element: Box::new(result),
            index: axis.index.clone(),
        };
    }
    Ok(result)
}

#[expect(clippy::too_many_arguments, reason = "scan expression context")]
fn infer_hir_scan(
    source: &hir::Expr,
    init: &hir::Expr,
    acc: &hir::LocalDef,
    val: &hir::LocalDef,
    body: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let source_type = infer_hir_type(
        source,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let source_rank = source_type.indexed_rank();
    let InferredType::Indexed { element, index } = source_type else {
        return Err(GraphcalError::EvalError {
            message: "scan source must be an indexed value".to_string(),
            src: src.clone(),
            span: source.span.into(),
        });
    };
    if source_rank > 1 {
        return Err(GraphcalError::MultiAxisScanSource {
            rank: source_rank,
            src: src.clone(),
            span: source.span.into(),
        });
    }
    let accumulator_type = infer_hir_type(
        init,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let scan_locals =
        local_types.child(vec![(acc.id, accumulator_type.clone()), (val.id, *element)]);
    let body_type = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &scan_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != accumulator_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&accumulator_type, registry),
            found: format_inferred_type(&body_type, registry),
            help: "scan body must return the same type as the accumulator".to_string(),
            src: src.clone(),
            span: body.span.into(),
        });
    }
    Ok(InferredType::Indexed {
        element: Box::new(accumulator_type),
        index,
    })
}

#[expect(clippy::too_many_arguments, reason = "unfold expression context")]
fn infer_hir_unfold(
    axis: &crate::syntax::span::Spanned<crate::syntax::index_name::ResolvedIndexName>,
    init: &hir::Expr,
    prev_state: &hir::LocalDef,
    prev_index: &hir::LocalDef,
    current_index: &hir::LocalDef,
    body: &hir::Expr,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let init_type = infer_hir_type(
        init,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    let index = InferredIndex::from_resolved(axis.value.clone());
    let idx_def = dag
        .semantic
        .collection_refs
        .index_defs
        .get(&axis.value)
        .ok_or_else(|| GraphcalError::InternalError {
            message: format!("missing resolved unfold axis `{}`", axis.value),
            src: src.clone(),
            span: axis.span.into(),
        })?;
    match &idx_def.kind {
        crate::registry::types::IndexKind::Coordinate(_)
        | crate::registry::types::IndexKind::RequiredCoordinate { .. } => {}
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("unfold requires a coordinate index, got `{}`", index.name()),
                src: src.clone(),
                span: axis.span.into(),
            });
        }
    }
    // The recurrence coordinate binders are keys of the axis; the coordinate
    // quantity is extracted with coord().
    let coordinate_type = InferredType::Key(index.clone());
    let unfold_locals = local_types.child(vec![
        (prev_state.id, init_type.clone()),
        (prev_index.id, coordinate_type.clone()),
        (current_index.id, coordinate_type),
    ]);
    let body_type = infer_hir_type(
        body,
        owner_decl_name,
        declared_types,
        &unfold_locals,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    if body_type != init_type {
        return Err(GraphcalError::DimensionMismatch {
            expected: format_inferred_type(&init_type, registry),
            found: format_inferred_type(&body_type, registry),
            help: "unfold body must return the same type as the previous state".to_string(),
            src: src.clone(),
            span: body.span.into(),
        });
    }
    Ok(InferredType::Indexed {
        element: Box::new(init_type),
        index,
    })
}

fn constructor_field_type(
    field: &crate::syntax::span::Spanned<FieldName>,
    variant: &NominalConstructor,
    owning_type: &ResolvedStructTypeName,
    type_def: &NominalTypeDef,
    scrutinee_type_args: &[InferredGenericArg],
    dag: &crate::tir::typed::DagTIR,
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    if !variant
        .fields()
        .iter()
        .any(|field_def| field_def.name() == &field.value)
    {
        return Err(GraphcalError::UnknownField {
            type_name: type_def.name(),
            field_name: field.value.clone(),
            src: src.clone(),
            span: field.span.into(),
        });
    }
    resolved_field_type(
        owning_type,
        variant,
        &field.value,
        type_def,
        scrutinee_type_args,
        dag,
        registry,
        src,
        field.span,
    )
}

#[expect(clippy::too_many_arguments, reason = "match expression context")]
#[expect(clippy::too_many_lines, reason = "exhaustive handling of match arms")]
fn infer_hir_match(
    expr: &hir::Expr,
    scrutinee: &hir::Expr,
    arms: &[hir::expr::MatchArm],
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let scrutinee_type = infer_hir_type(
        scrutinee,
        owner_decl_name,
        declared_types,
        local_types,
        dag,
        tir,
        registry,
        builtin_fns,
        src,
    )?;
    match &scrutinee_type {
        InferredType::Key(index_identity) => {
            if index_identity.finite_index_form().is_some() {
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "cannot match on `Key<{}>`; only named-axis keys support label matching",
                        index_identity.name()
                    ),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                });
            }
            let index_def = super::index_def_for_inferred(index_identity, Some(dag), registry)
                .ok_or_else(|| GraphcalError::UnknownIndex {
                    name: index_identity.name(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                })?;
            let variants = match &index_def.kind {
                crate::registry::types::IndexKind::Named { variants } => variants.clone(),
                crate::registry::types::IndexKind::RequiredNamed => vec![],
                _ => {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "cannot match on coordinate index `{}`; only named indexes can be matched",
                            index_identity.name()
                        ),
                        src: src.clone(),
                        span: scrutinee.span.into(),
                    });
                }
            };
            let mut covered = std::collections::HashSet::new();
            let mut arm_types = Vec::new();
            for arm in arms {
                let hir::expr::MatchPattern::IndexLabel { variant, span } = &arm.pattern else {
                    return Err(GraphcalError::EvalError {
                        message: "label match arms must use index-label patterns".to_string(),
                        src: src.clone(),
                        span: arm.span.into(),
                    });
                };
                check_index_override_dependency(
                    local_types,
                    dag,
                    owner_decl_name,
                    &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                    IndexNominalUse::Label(variant.variant.variant()),
                )?;
                if !index_identity.matches_resolved(variant.variant.index()) {
                    return Err(GraphcalError::IndexMismatch {
                        expected: index_identity.name(),
                        found: variant.variant.index().to_unowned_def_name(),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                let variant_name = variant.variant.variant();
                if !variants.iter().any(|v| v == variant_name) {
                    return Err(GraphcalError::UnknownVariant {
                        index_name: index_identity.name(),
                        variant_name: variant_name.clone(),
                        src: src.clone(),
                        span: variant.path_span().into(),
                    });
                }
                if !covered.insert(variant_name.clone()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for variant `{variant_name}`"),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                arm_types.push(infer_hir_type(
                    &arm.body,
                    owner_decl_name,
                    declared_types,
                    local_types,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?);
            }
            for variant in variants {
                if !covered.contains(&variant) {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "non-exhaustive match: variant `{}` not covered",
                            variant.qualified_by(&index_identity.name())
                        ),
                        src: src.clone(),
                        span: expr.span.into(),
                    });
                }
            }
            hir_arm_types_match(&arm_types, arms, registry, src, expr)
        }
        InferredType::Struct(type_name, scrutinee_type_args) => {
            let type_def = struct_type_def_for_inferred(type_name, Some(dag), registry)
                .ok_or_else(|| GraphcalError::UnknownStructType {
                    name: type_name.to_string(),
                    src: src.clone(),
                    span: scrutinee.span.into(),
                })?;
            let mut covered = std::collections::HashSet::new();
            let mut arm_types = Vec::new();
            for arm in arms {
                let hir::expr::MatchPattern::Constructor {
                    constructor,
                    bindings,
                    span,
                } = &arm.pattern
                else {
                    return Err(GraphcalError::EvalError {
                        message: "union match arms must use constructor patterns".to_string(),
                        src: src.clone(),
                        span: arm.span.into(),
                    });
                };
                let target = dag
                    .semantic
                    .constructor_refs
                    .constructor_defs
                    .get(&constructor.value)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic TIR missing constructor match target for `{}`",
                            constructor.value
                        ),
                        src: src.clone(),
                        span: constructor.span.into(),
                    })?;
                check_type_override_dependency(
                    local_types,
                    dag,
                    owner_decl_name,
                    &target.owning_type,
                    TypeNominalUse::Constructor(&constructor.value),
                )?;
                if !type_name.matches_resolved(&target.owning_type) {
                    return Err(GraphcalError::UnknownField {
                        type_name: type_name.name().clone(),
                        field_name: FieldName::expect_valid(target.variant.name().as_str()),
                        src: src.clone(),
                        span: constructor.span.into(),
                    });
                }
                if !covered.insert(target.variant.name().clone()) {
                    return Err(GraphcalError::EvalError {
                        message: format!("duplicate match arm for `{}`", target.variant.name()),
                        src: src.clone(),
                        span: (*span).into(),
                    });
                }
                let mut arm_locals = local_types.child(Vec::new());
                let mut seen_pattern_fields = std::collections::HashSet::new();
                for binding in bindings {
                    let field = match binding {
                        hir::expr::PatternBinding::Bind { field, .. }
                        | hir::expr::PatternBinding::Wildcard { field, .. } => field,
                    };
                    if !seen_pattern_fields.insert(field.value.clone()) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "duplicate pattern binding for field `{}` in `{}`",
                                field.value,
                                target.variant.name()
                            ),
                            src: src.clone(),
                            span: field.span.into(),
                        });
                    }
                    let field_type = constructor_field_type(
                        field,
                        &target.variant,
                        &target.owning_type,
                        &target.type_def,
                        scrutinee_type_args,
                        dag,
                        registry,
                        src,
                    )?;
                    match binding {
                        hir::expr::PatternBinding::Bind { local, .. } => {
                            arm_locals.bind(local.id, field_type);
                        }
                        hir::expr::PatternBinding::Wildcard { .. } => {}
                    }
                }
                arm_types.push(infer_hir_type(
                    &arm.body,
                    owner_decl_name,
                    declared_types,
                    &arm_locals,
                    dag,
                    tir,
                    registry,
                    builtin_fns,
                    src,
                )?);
            }
            if let Some(members) = type_def.union_members() {
                for member in members {
                    if !covered.contains(&member.name()) {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "non-exhaustive match: member `{}` not covered",
                                member.name()
                            ),
                            src: src.clone(),
                            span: expr.span.into(),
                        });
                    }
                }
            }
            hir_arm_types_match(&arm_types, arms, registry, src, expr)
        }
        _ => Err(GraphcalError::EvalError {
            message: format!(
                "cannot match on type `{}`; expected a tagged union or label value",
                format_inferred_type(&scrutinee_type, registry)
            ),
            src: src.clone(),
            span: scrutinee.span.into(),
        }),
    }
}

fn hir_arm_types_match(
    arm_types: &[InferredType],
    arms: &[hir::expr::MatchArm],
    registry: &SemanticRegistry,
    src: &NamedSource<Arc<String>>,
    expr: &hir::Expr,
) -> Result<InferredType, GraphcalError> {
    rules::match_arms_rule(arm_types, |i| arms[i].body.span, expr.span, registry, src)
}

fn specialize_dag_call_dimension(
    dimension: &Dimension,
    bindings: &hir::expr::DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    dimension.iter().try_fold(
        Dimension::dimensionless(),
        |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => bindings
                    .dimensions
                    .get(name)
                    .map_or_else(
                        || Ok(Dimension::base(base.clone())),
                        |target| {
                            tir.dimension(target).cloned().ok_or_else(|| {
                                GraphcalError::InternalError {
                                    message: format!(
                                        "DAG-call dimension binding target `{target}` is absent from the semantic registry"
                                    ),
                                    src: src.clone(),
                                    span: span.into(),
                                }
                            })
                        },
                    )?,
                BaseDimId::Prelude(_) => Dimension::base(base.clone()),
            };
            let factor = factor.pow(*exponent).map_err(|error| {
                GraphcalError::InternalError {
                    message: format!("DAG-call dimension substitution overflowed: {error}"),
                    src: src.clone(),
                    span: span.into(),
                }
            })?;
            acc.checked_mul(&factor).map_err(|error| {
                GraphcalError::InternalError {
                    message: format!("DAG-call dimension substitution overflowed: {error}"),
                    src: src.clone(),
                    span: span.into(),
                }
            })
        },
    )
}

fn specialize_dag_call_index(
    index: &ResolvedIndex,
    bindings: &hir::expr::DagCallStaticBindings,
) -> ResolvedIndex {
    match index {
        ResolvedIndex::Concrete(name, span) => bindings.indexes.get(name).map_or_else(
            || index.clone(),
            |target| match target {
                hir::expr::DagCallIndexBinding::Declared(target) => {
                    ResolvedIndex::Concrete(target.clone(), *span)
                }
                hir::expr::DagCallIndexBinding::Finite(target) => {
                    ResolvedIndex::Finite(NatPolyForm::from_constant(target.size_u64()), *span)
                }
            },
        ),
        ResolvedIndex::GenericParam(_, _) | ResolvedIndex::Finite(_, _) => index.clone(),
    }
}

fn specialize_dag_call_dim_arg(
    dimension: &ResolvedDimArg,
    bindings: &hir::expr::DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ResolvedDimArg, GraphcalError> {
    match dimension {
        ResolvedDimArg::Concrete(dimension) => {
            { specialize_dag_call_dimension(dimension, bindings, tir, src, span) }
                .map(ResolvedDimArg::Concrete)
        }
        ResolvedDimArg::Expr { terms, span } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    specialize_dag_call_dimension(dim, bindings, tir, src, *span).map(|dim| {
                        ResolvedDimTerm::Concrete {
                            dim,
                            power: *power,
                            op: *op,
                        }
                    })
                }
                ResolvedDimTerm::GenericParam { .. } => Ok(term.clone()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|terms| ResolvedDimArg::Expr { terms, span: *span }),
        ResolvedDimArg::Dimensionless | ResolvedDimArg::GenericParam(_, _) => Ok(dimension.clone()),
    }
}

fn specialize_dag_call_type(
    resolved: &ResolvedTypeExpr,
    bindings: &hir::expr::DagCallStaticBindings,
    tir: &crate::tir::typed::TIR,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let recurse =
        |resolved: &ResolvedTypeExpr| specialize_dag_call_type(resolved, bindings, tir, src, span);
    match resolved {
        ResolvedTypeExpr::Quantity(dimension) => {
            { specialize_dag_call_dimension(dimension, bindings, tir, src, span) }
                .map(ResolvedTypeExpr::Quantity)
        }
        ResolvedTypeExpr::Complex {
            dimension,
            span: type_span,
        } => specialize_dag_call_dim_arg(dimension, bindings, tir, src, span).map(|dimension| {
            ResolvedTypeExpr::Complex {
                dimension,
                span: *type_span,
            }
        }),
        ResolvedTypeExpr::IndexArg(index) => Ok(ResolvedTypeExpr::IndexArg(
            specialize_dag_call_index(index, bindings),
        )),
        ResolvedTypeExpr::Key {
            index,
            span: type_span,
        } => Ok(ResolvedTypeExpr::Key {
            index: specialize_dag_call_index(index, bindings),
            span: *type_span,
        }),
        ResolvedTypeExpr::Struct(name, type_span) => Ok(ResolvedTypeExpr::Struct(
            bindings.types.get(name).unwrap_or(name).clone(),
            *type_span,
        )),
        ResolvedTypeExpr::GenericStruct {
            name,
            generic_args,
            span: type_span,
        } => {
            let generic_args = generic_args
                .iter()
                .map(|argument| match argument {
                    ResolvedGenericArg::Dim(dimension) => {
                        { specialize_dag_call_dim_arg(dimension, bindings, tir, src, span) }
                            .map(ResolvedGenericArg::Dim)
                    }
                    ResolvedGenericArg::Index(index) => Ok(ResolvedGenericArg::Index(
                        specialize_dag_call_index(index, bindings),
                    )),
                    ResolvedGenericArg::Nat(_, _) => Ok(argument.clone()),
                    ResolvedGenericArg::Type(resolved) => {
                        recurse(resolved).map(ResolvedGenericArg::Type)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericStruct {
                name: bindings.types.get(name).unwrap_or(name).clone(),
                generic_args,
                span: *type_span,
            })
        }
        ResolvedTypeExpr::GenericDimExpr {
            terms,
            span: type_span,
        } => {
            let terms = terms
                .iter()
                .map(|term| match term {
                    ResolvedDimTerm::Concrete { dim, power, op } => {
                        specialize_dag_call_dimension(dim, bindings, tir, src, span).map(|dim| {
                            ResolvedDimTerm::Concrete {
                                dim,
                                power: *power,
                                op: *op,
                            }
                        })
                    }
                    ResolvedDimTerm::GenericParam { .. } => Ok(term.clone()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericDimExpr {
                terms,
                span: *type_span,
            })
        }
        ResolvedTypeExpr::Indexed { base, indexes } => Ok(ResolvedTypeExpr::Indexed {
            base: Box::new(recurse(base)?),
            indexes: indexes
                .iter()
                .map(|index| specialize_dag_call_index(index, bindings))
                .collect(),
        }),
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _) => Ok(resolved.clone()),
    }
}

#[expect(clippy::too_many_arguments, reason = "DAG-call expression context")]
fn infer_hir_dag_call(
    expr: &hir::Expr,
    target: &crate::syntax::span::Spanned<crate::dag_id::DagId>,
    args: &[hir::expr::ParamBinding],
    static_bindings: &hir::expr::DagCallStaticBindings,
    output: &crate::syntax::span::Spanned<ResolvedDeclName>,
    owner_decl_name: Option<&ResolvedDeclName>,
    declared_types: &HashMap<ScopedName, DeclaredType>,
    local_types: &HirLocalTypes<'_>,
    dag: &crate::tir::typed::DagTIR,
    tir: &crate::tir::typed::TIR,
    registry: &SemanticRegistry,
    builtin_fns: &crate::registry::builtins::BuiltinFunctions,
    src: &NamedSource<Arc<String>>,
) -> Result<InferredType, GraphcalError> {
    let display_path = target.value.to_string();
    let dag_tir = tir
        .dags
        .get(&target.value)
        .ok_or_else(|| GraphcalError::UnknownDag {
            name: display_path.clone(),
            src: src.clone(),
            span: target.span.into(),
        })?;

    let mut required_param_keys = std::collections::HashSet::new();
    let param_decl_types_by_key: HashMap<ResolvedDeclKey, &crate::tir::typed::ResolvedTypeExpr> =
        dag_tir
            .params
            .iter()
            .map(|param| {
                let key = dag_tir.require_bound_decl_identity(
                    &param.name,
                    src,
                    DiagnosticAnchor::Source(param.span),
                )?;
                if param.default_expr.is_none() {
                    required_param_keys.insert(key.clone());
                }
                let resolved = dag_tir
                    .resolved_decl_types
                    .get(&param.name)
                    .ok_or_else(|| GraphcalError::InternalError {
                        message: format!(
                            "semantic type missing for DAG-call param `{}`",
                            param.name
                        ),
                        src: src.clone(),
                        span: param.type_ann.span.into(),
                    })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;
    let node_decl_types_by_key: HashMap<ResolvedDeclKey, &crate::tir::typed::ResolvedTypeExpr> =
        dag_tir
            .nodes
            .iter()
            .map(|node| {
                let key = dag_tir.require_bound_decl_identity(
                    &node.name,
                    src,
                    DiagnosticAnchor::Source(node.span),
                )?;
                let resolved = dag_tir.resolved_decl_types.get(&node.name).ok_or_else(|| {
                    GraphcalError::InternalError {
                        message: format!("semantic type missing for DAG-call node `{}`", node.name),
                        src: src.clone(),
                        span: node.type_ann.span.into(),
                    }
                })?;
                Ok((key, resolved))
            })
            .collect::<Result<_, GraphcalError>>()?;

    let mut bound_resolved_names: std::collections::HashSet<ResolvedDeclKey> =
        std::collections::HashSet::with_capacity(args.len());
    for binding in args {
        let target_key = &binding.target.value;
        bound_resolved_names.insert(target_key.clone());
        let expected = param_decl_types_by_key.get(target_key).ok_or_else(|| {
            GraphcalError::UnknownDagParam {
                name: target_key.as_str().to_string(),
                dag_name: display_path.clone(),
                src: src.clone(),
                span: binding.target.span.into(),
            }
        })?;
        let found = infer_hir_type(
            &binding.value,
            owner_decl_name,
            declared_types,
            local_types,
            dag,
            tir,
            registry,
            builtin_fns,
            src,
        )?;
        let expected =
            specialize_dag_call_type(expected, static_bindings, tir, src, binding.target.span)?;
        if !resolved_type_matches_inferred(&expected, &found) {
            return Err(GraphcalError::DagArgTypeMismatch {
                param_name: target_key.as_str().to_string(),
                expected: expected.format(registry),
                found: format_inferred_type(&found, registry),
                src: src.clone(),
                span: binding.value.span.into(),
            });
        }
    }

    let mut missing: Vec<String> = required_param_keys
        .iter()
        .filter(|param| !bound_resolved_names.contains(*param))
        .map(|param| param.as_str().to_string())
        .collect();
    if !missing.is_empty() {
        missing.sort();
        return Err(GraphcalError::MissingDagBindings {
            missing,
            dag_name: display_path.clone(),
            src: src.clone(),
            span: expr.span.into(),
        });
    }

    let output_key = &output.value;
    let output_decl = node_decl_types_by_key
        .get(output_key)
        .or_else(|| param_decl_types_by_key.get(output_key))
        .ok_or_else(|| GraphcalError::UnknownDagOutput {
            name: output_key.as_str().to_string(),
            dag_name: display_path.clone(),
            src: src.clone(),
            span: output.span.into(),
        })?;
    let output_name = output_key.as_str();
    if !dag_tir.projectable_outputs.contains(output_name) {
        return Err(GraphcalError::ImportPrivateItem {
            name: output_name.to_string(),
            file_path: display_path,
            src: src.clone(),
            span: output.span.into(),
        });
    }
    let output_decl =
        specialize_dag_call_type(output_decl, static_bindings, tir, src, output.span)?;
    substitute_resolved_type_with_type_params(&output_decl, &GenericSubstitutions::default(), src)
}

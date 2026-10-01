//! The recursive expression-kind dispatch of HIR inference.

use crate::dimension::Dimension;
use crate::graphcal_error::GraphcalError;
use crate::hir::expr::{Expr, ExprKind};
use crate::outcome::Outcome;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::structure::StructError;

use crate::semantic::checked_type::CheckedType;

use super::context::{Infer, PrecheckedArgs};
use super::override_deps::IndexNominalUse;
use super::refs::infer_hir_quantity_literal;

impl Infer<'_> {
    pub(super) fn infer_hir_type(
        &self,
        expr: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        self.control.checkpoint()?;
        // Recursion choke point: inference recurses once per tree level
        // (unbounded for left-nested operator chains).
        crate::stack::with_stack_growth(|| self.outside_call().infer_hir_type_inner(expr))
    }

    fn infer_hir_type_inner(
        &self,
        expr: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<GraphcalError>> {
        let inferred = match expr.kind() {
            ExprKind::Error(no_error) => no_error.absurd(),
            ExprKind::Number(_) => CheckedType::Quantity(Dimension::dimensionless()),
            ExprKind::Integer(_) => CheckedType::Int,
            ExprKind::Bool(_) => CheckedType::Bool,
            ExprKind::StringLiteral(_)
            | ExprKind::OffsetDateTimeLiteral(_)
            | ExprKind::CivilDateTimeLiteral(_)
            | ExprKind::ZonedDateTimeLiteral(_)
            | ExprKind::IanaTimeZoneLiteral(_) => {
                return Err(GraphcalError::located(
                    self.env.src,
                    expr.span,
                    DimensionError::DimensionMismatch {
                        expected: "a numeric or boolean expression".to_string(),
                        found: "contextual string literal".to_string(),
                        help:
                            "string literals can only be used in their declared datetime contexts"
                                .to_string(),
                    },
                )
                .into());
            }
            ExprKind::TypeSystemRef(name) => {
                return Err(GraphcalError::located(
                    self.env.src,
                    name.span,
                    EvaluationError::Failed {
                        message: name.value.value_position_error(),
                    },
                )
                .into());
            }
            ExprKind::QuantityLiteral { unit, .. } => {
                infer_hir_quantity_literal(unit, self.env.tir, self.env.src)?
            }
            ExprKind::VariantLiteral(variant) => {
                self.check_index_override_dependency(
                    &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                    IndexNominalUse::Label(variant.variant.variant()),
                )?;
                // A qualified label is self-typed: `Maneuver#Departure` is a
                // constant of type `Key<Maneuver>` — the axis is in the spelling.
                CheckedType::Key(IndexTypeRef::from_resolved(variant.variant.index().clone()))
            }
            ExprKind::GraphRef(target) => self
                .env
                .infer_resolved_decl_ref_type(&target.value, target.span)?,
            ExprKind::ConstRef(target) => self.infer_hir_const_ref(target)?,
            ExprKind::LocalRef(local) => {
                self.locals.get(local.value).cloned().ok_or_else(|| {
                    GraphcalError::located(
                        self.env.src,
                        local.span,
                        StructError::UnknownLocalRef {
                            name: format!("#{}", local.value.index()),
                        },
                    )
                })?
            }
            ExprKind::FnCall { callee, args, .. } => {
                if self.owner.is_some_and(|owner| {
                    self.env
                        .dag
                        .semantic
                        .override_reconciliations
                        .contains_key(owner)
                }) {
                    // Function inference has several specialized signature paths.
                    // Check each argument once with the declaration identity intact;
                    // those paths then reuse the checked argument types.
                    let types = args
                        .iter()
                        .map(|arg| self.infer_hir_type(arg))
                        .collect::<Result<Vec<_>, _>>()?;
                    let prechecked = PrecheckedArgs::new(args, types);
                    self.with_prechecked_args(&prechecked)
                        .infer_hir_fn_call(callee, args)?
                } else {
                    self.infer_hir_fn_call(callee, args)?
                }
            }
            ExprKind::ForComp { bindings, body } => self.infer_hir_for_comp(bindings, body)?,
            ExprKind::IndexAccess { expr: inner, args } => {
                self.infer_hir_index_access(expr, inner, args.as_slice())?
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.infer_hir_if(condition, then_branch, else_branch)?,
            ExprKind::UnaryOp { op, operand } => self.infer_hir_unary(*op, operand)?,
            ExprKind::BinOp { op, lhs, rhs } => self.infer_hir_binop(expr.span, *op, lhs, rhs)?,
            ExprKind::Convert {
                expr: inner,
                target,
            } => self.infer_hir_convert(inner, target)?,
            ExprKind::DisplayTimezone {
                expr: inner,
                timezone,
            } => self.infer_hir_display_timezone(inner, timezone)?,
            ExprKind::FieldAccess { expr: inner, field } => {
                self.infer_hir_field_access(inner, field)?
            }
            ExprKind::ConstructorCall {
                callee,
                generic_args,
                fields,
            } => self.infer_hir_constructor_call(expr, callee, generic_args, fields)?,
            ExprKind::MapLiteral { entries } => self.infer_hir_map_literal(expr, entries)?,
            ExprKind::Scan {
                source,
                init,
                acc,
                val,
                body,
            } => self.infer_hir_scan(source, init, acc, val, body)?,
            ExprKind::Unfold {
                recurrence,
                init,
                body,
            } => self.infer_hir_unfold(
                &recurrence.axis,
                init,
                &recurrence.previous_state,
                &recurrence.previous_index,
                &recurrence.current_index,
                body,
            )?,
            ExprKind::KeyForm {
                kind,
                axis,
                axis_span,
                arg,
            } => self.infer_hir_key_form(expr, *kind, axis, *axis_span, arg)?,
            ExprKind::Match { scrutinee, arms } => self.infer_hir_match(expr, scrutinee, arms)?,
            ExprKind::DagCall {
                target,
                args,
                static_bindings,
                output,
            } => self.infer_hir_dag_call(expr, target, args, static_bindings, output)?,
        };
        self.control.observations().record(
            expr,
            &inferred,
            self.env.dag,
            self.env.tir,
            self.env.src,
        )?;
        Ok(inferred)
    }
}

//! Lowering of constant-expression positions from the desugared AST into
//! [`crate::hir::const_expr`] trees.
//!
//! Every operator is typed with the shared rules of
//! [`crate::tir::operator_typing`], the rules node bodies use, so an
//! expression means the same in a unit scale, a coordinate, and a node:
//! `Int` literals are `Int`, an `Int` scales a quantity in `*` and `/`, and
//! `/` on two `Int` operands is rejected. A position adds only what it
//! admits on top: the constant sublanguage (literals, built-in constants,
//! arithmetic, `to_float`, and `div_*`), quantity literals in coordinates,
//! and the type of the whole expression.
//!
//! Unit literals in constant positions are resolved by the declaring module's
//! canonical unit scope, not against module-level Term declarations: a name
//! is either a [`BuiltinConst`] or an error, and a unit literal must name a
//! unit with a static scale.

use std::marker::PhantomData;

use crate::builtin::{BuiltinConst, BuiltinFn, ConversionFn};
use crate::desugar::desugared_ast::{self as ast, BinOp, UnaryOp};
use crate::dimension::Dimension;
use crate::finite_value::FiniteQuantity;
use crate::hir::const_expr::{
    ConstExprError, ConstInt, ConstIntKind, ConstPosition, ConstReal, ConstRealKind, ConstUnit,
    CoordinateExpr, CoordinatePosition, UnitScaleExpr, UnitScalePosition,
};
use crate::hir::types::NatExpr;
use crate::semantic::checked_type::{CheckedType, Symbolic};
use crate::semantic::dimension_table::DimensionFormattingRegistry;
use crate::semantic::unit_scale::{PositiveFiniteScale, UnitResolveError};
use crate::semantic_error::SemanticError;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::NonQuantityValue;
use crate::semantic_error::name::{CalledFunction, NameError};
use crate::source_id::SourceId;
use crate::syntax::ast::{IdentPath, PowerExponent, UnresolvedRef};
use crate::syntax::dimension::UnitRef;
use crate::syntax::module_name::ScopedName;
use crate::syntax::names::NamePath;
use crate::syntax::span::{Span, Spanned};
use crate::syntax::visitor::ExprVisitor;
use crate::tir::operator_typing::{self, Operand};
use crate::tir::texpr::operators::{ArithOp, IntArithOp, IntConversion};

/// A unit definition's scale expression, classified by whether it reads the
/// graph.
#[derive(Debug, Clone)]
pub enum UnitScaleSource<'a> {
    /// No graph reference: the scale is a compile-time constant.
    Static(StaticScaleSource<'a>),
    /// At least one graph reference: the scale is evaluated at runtime.
    Dynamic {
        /// The first graph reference in source order.
        first_graph_ref: FirstGraphRef,
    },
}

/// A scale expression proven free of graph references.
#[derive(Debug, Clone, Copy)]
pub struct StaticScaleSource<'a>(&'a ast::Expr);

/// The first graph reference of a dynamic scale expression, in source order.
pub type FirstGraphRef = Spanned<ScopedName>;

/// Classify a unit definition's scale expression.
pub fn classify_unit_scale(expr: &ast::Expr) -> UnitScaleSource<'_> {
    struct FirstGraphRefVisitor(Option<Spanned<ScopedName>>);

    impl ExprVisitor<crate::syntax::phase::Desugared> for FirstGraphRefVisitor {
        type Error = std::convert::Infallible;

        fn visit_graph_ref(&mut self, expr: &ast::Expr) -> Result<(), Self::Error> {
            if self.0.is_none()
                && let ast::ExprKind::GraphRef(name) = &expr.kind
            {
                self.0 = Some(Spanned::new(name.to_scoped_name(), name.span()));
            }
            Ok(())
        }
    }

    let mut visitor = FirstGraphRefVisitor(None);
    let _ = visitor.visit_expr(expr);
    visitor.0.map_or(
        UnitScaleSource::Static(StaticScaleSource(expr)),
        |first_graph_ref| UnitScaleSource::Dynamic { first_graph_ref },
    )
}

/// What typing a constant expression needs: the dimension spellings its
/// diagnostics use and the source they point into.
#[derive(Debug, Clone, Copy)]
pub struct ConstTyping<'a> {
    pub registry: &'a DimensionFormattingRegistry,
    pub src: SourceId,
}

/// Why a constant position could not be lowered.
#[derive(Debug, Clone)]
pub enum ConstLowerError {
    /// A construct outside the position's sublanguage, an unresolvable unit,
    /// or a failed evaluation.
    Expr(ConstExprError),
    /// A type error reported by the shared operator rules or the position.
    Type(Box<SemanticError>),
}

impl From<ConstExprError> for ConstLowerError {
    fn from(error: ConstExprError) -> Self {
        Self::Expr(error)
    }
}

impl From<SemanticError> for ConstLowerError {
    fn from(error: SemanticError) -> Self {
        Self::Type(Box::new(error))
    }
}

impl StaticScaleSource<'_> {
    /// Lower and type the scale expression into its constant HIR tree.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstLowerError`] for a construct outside the constant
    /// sublanguage, an operator the shared rules reject, or a scale of `unit`
    /// that is neither an `Int` nor a `Dimensionless` quantity. Errors are
    /// reported in source order.
    pub fn lower(
        self,
        unit: &UnitRef,
        typing: ConstTyping<'_>,
    ) -> Result<UnitScaleExpr, ConstLowerError> {
        let lowering = Lowering::<UnitScalePosition> {
            typing,
            resolve_unit: None,
            position: PhantomData,
        };
        match lowering.lower(self.0)? {
            Typed::Int(expr) => Ok(UnitScaleExpr::Int(expr)),
            Typed::Real(expr, dimension) if dimension.is_dimensionless() => {
                Ok(UnitScaleExpr::Real(expr))
            }
            typed @ Typed::Real(..) => Err(SemanticError::located(
                typing.src,
                self.0.span,
                DimensionError::UnitScaleTypeMismatch {
                    name: unit.clone(),
                    found: typed.checked_type().spelling(typing.registry),
                },
            )
            .into()),
        }
    }
}

/// Resolve a unit literal to its dimension and static scale in the declaring
/// module's canonical unit scope.
pub type UnitExprResolver<'a> =
    dyn Fn(&ast::UnitExpr) -> Result<(Dimension, PositiveFiniteScale), UnitResolveError> + 'a;

/// Lower and type a coordinate bound or step, resolving unit literals with
/// `resolve_unit`.
///
/// # Errors
///
/// Returns a [`ConstLowerError`] for a non-finite literal, an unresolvable or
/// dynamic unit, a construct outside the constant sublanguage, an operator
/// the shared rules reject, or a value that is not a real quantity. Errors
/// are reported in source order.
pub fn lower_coordinate_expr(
    expr: &ast::Expr,
    resolve_unit: &UnitExprResolver<'_>,
    typing: ConstTyping<'_>,
) -> Result<CoordinateExpr, ConstLowerError> {
    let lowering = Lowering::<CoordinatePosition> {
        typing,
        resolve_unit: Some(resolve_unit),
        position: PhantomData,
    };
    match lowering.lower(expr)? {
        Typed::Real(expr, dimension) => Ok(CoordinateExpr::new(expr, dimension)),
        Typed::Int(int) => Err(operator_typing::not_a_quantity(
            NonQuantityValue::Int,
            CheckedType::<Symbolic>::Int.spelling(typing.registry),
            typing.src,
            int.span(),
        )
        .into()),
    }
}

/// Lower a static natural-number expression (a `linspace` point count).
///
/// A static count has no generic scope, so a name is rejected here rather
/// than resolved.
///
/// # Errors
///
/// Returns [`ConstExprError::NonConstantNat`] for the first name in source
/// order.
pub fn lower_static_nat_expr(expr: &ast::NatExpr) -> Result<NatExpr, ConstExprError> {
    match expr {
        ast::NatExpr::Literal(value, span) => Ok(NatExpr::Literal(*value, *span)),
        ast::NatExpr::Var(ident) => Err(ConstExprError::NonConstantNat {
            name: ident.as_generic_param_name(),
            span: ident.span,
        }),
        ast::NatExpr::Add(operands, span) => Ok(NatExpr::Add(
            operands.try_map_ref(lower_static_nat_expr)?,
            *span,
        )),
        ast::NatExpr::Mul(operands, span) => Ok(NatExpr::Mul(
            operands.try_map_ref(lower_static_nat_expr)?,
            *span,
        )),
    }
}

/// The diagnostics a position reports for constructs outside its
/// sublanguage.
trait PositionErrors: ConstPosition {
    /// A name that is not a built-in constant.
    fn unknown_constant(path: Spanned<NamePath>) -> ConstExprError;
    /// A construct outside the constant sublanguage.
    fn non_constant(span: Span) -> ConstExprError;
}

impl PositionErrors for UnitScalePosition {
    fn unknown_constant(path: Spanned<NamePath>) -> ConstExprError {
        ConstExprError::UnknownScaleConstant { path }
    }

    fn non_constant(span: Span) -> ConstExprError {
        ConstExprError::NonConstantScale { span }
    }
}

impl PositionErrors for CoordinatePosition {
    fn unknown_constant(path: Spanned<NamePath>) -> ConstExprError {
        ConstExprError::UnknownCoordinateConstant { path }
    }

    fn non_constant(span: Span) -> ConstExprError {
        ConstExprError::NonStaticCoordinate { span }
    }
}

/// A lowered subexpression with the type the shared rules gave it.
enum Typed<P: ConstPosition> {
    Int(ConstInt),
    Real(ConstReal<P>, Dimension),
}

impl<P: ConstPosition> Typed<P> {
    /// The type the shared rules see for this operand.
    fn checked_type(&self) -> CheckedType<Symbolic> {
        match self {
            Self::Int(_) => CheckedType::Int,
            Self::Real(_, dimension) => CheckedType::Quantity(dimension.clone()),
        }
    }

    /// The real value of this operand: an `Int` that meets a quantity is
    /// converted explicitly, labeled with the role it plays there.
    fn into_real(self, conversion: IntConversion) -> ConstReal<P> {
        match self {
            Self::Int(int) => {
                let span = int.span();
                ConstReal::new(
                    ConstRealKind::FromInt {
                        conversion,
                        operand: Box::new(int),
                    },
                    span,
                )
            }
            Self::Real(real, _) => real,
        }
    }
}

/// Lowering of one constant position `P`: the typing context, and the unit
/// resolver when `P` admits quantity literals.
struct Lowering<'a, 'r, P> {
    typing: ConstTyping<'a>,
    resolve_unit: Option<&'r UnitExprResolver<'r>>,
    position: PhantomData<P>,
}

impl<P: PositionErrors> Lowering<'_, '_, P> {
    /// Lower and type `expr`, reporting the first error in source order.
    fn lower(&self, expr: &ast::Expr) -> Result<Typed<P>, ConstLowerError> {
        let span = expr.span;
        let real = |kind| Typed::Real(ConstReal::new(kind, span), Dimension::dimensionless());
        Ok(match &expr.kind {
            ast::ExprKind::Number(value) => {
                let value = FiniteQuantity::try_new(*value).map_err(|_| {
                    ConstExprError::NonFiniteLiteral {
                        value: *value,
                        span,
                    }
                })?;
                real(ConstRealKind::Number(value))
            }
            ast::ExprKind::Integer(value) => {
                Typed::Int(ConstInt::new(ConstIntKind::Literal(*value), span))
            }
            ast::ExprKind::UnresolvedRef(UnresolvedRef::Path(path)) => {
                match path
                    .as_bare()
                    .and_then(|ident| BuiltinConst::parse(ident.name.as_str()))
                {
                    Some(constant) => real(ConstRealKind::Builtin(constant)),
                    None => {
                        return Err(
                            P::unknown_constant(path.clone().into_spanned_name_path()).into()
                        );
                    }
                }
            }
            ast::ExprKind::QuantityLiteral { value, unit } => {
                let (Some(admitted), Some(resolve)) = (P::QUANTITY, self.resolve_unit) else {
                    return Err(P::non_constant(span).into());
                };
                let value = FiniteQuantity::try_new(*value).map_err(|_| {
                    ConstExprError::NonFiniteLiteral {
                        value: *value,
                        span,
                    }
                })?;
                let (dimension, scale) = resolve(unit).map_err(|error| ConstExprError::Unit {
                    error,
                    span: unit.span,
                })?;
                let unit = ConstUnit::new(unit.clone(), dimension.clone(), scale);
                Typed::Real(
                    ConstReal::new(
                        ConstRealKind::Quantity {
                            value,
                            unit,
                            admitted,
                        },
                        span,
                    ),
                    dimension,
                )
            }
            ast::ExprKind::UnaryOp {
                op: op @ UnaryOp::Neg,
                operand,
            } => {
                let lowered = self.lower(operand)?;
                operator_typing::unary_rule(
                    *op,
                    &Operand {
                        ty: lowered.checked_type(),
                        span: operand.span,
                    },
                    self.typing.registry,
                    self.typing.src,
                )?;
                match lowered {
                    Typed::Int(int) => {
                        Typed::Int(ConstInt::new(ConstIntKind::Neg(Box::new(int)), span))
                    }
                    Typed::Real(real, dimension) => Typed::Real(
                        ConstReal::new(ConstRealKind::Neg(Box::new(real)), span),
                        dimension,
                    ),
                }
            }
            ast::ExprKind::BinOp { op, lhs, rhs } => self.binop(span, *op, lhs, rhs)?,
            ast::ExprKind::FnCall {
                callee,
                generic_args,
                args,
            } if generic_args.is_empty() => self.call(span, callee, args)?,
            _ => return Err(P::non_constant(span).into()),
        })
    }

    /// `to_float(n)` and `div_*(a, b)`, the built-ins that make an `Int`
    /// conversion or an `Int` division explicit. No other call is constant.
    fn call(
        &self,
        span: Span,
        callee: &IdentPath,
        args: &[ast::Expr],
    ) -> Result<Typed<P>, ConstLowerError> {
        let Some((ident, function)) = callee
            .as_bare()
            .and_then(|ident| Some((ident, BuiltinFn::parse(ident.name.as_str())?)))
        else {
            return Err(P::non_constant(span).into());
        };
        let wrong_arity = |expected| {
            Err(SemanticError::located(
                self.typing.src,
                ident.span,
                NameError::WrongArity {
                    name: CalledFunction::Builtin(function),
                    expected,
                    got: args.len(),
                },
            )
            .into())
        };
        let operand = |typed: &Typed<P>, arg: &ast::Expr| Operand {
            ty: typed.checked_type(),
            span: arg.span,
        };
        // The rules admit only `Int` arguments, so a real argument that
        // passed them would be a call the constant tree cannot represent;
        // it is reported as a non-constant construct.
        match (function, args) {
            (BuiltinFn::Conversion(ConversionFn::ToFloat), [arg]) => {
                let typed = self.lower(arg)?;
                operator_typing::to_float_rule(
                    &operand(&typed, arg),
                    self.typing.registry,
                    self.typing.src,
                )?;
                match typed {
                    Typed::Int(int) => Ok(Typed::Real(
                        ConstReal::new(
                            ConstRealKind::FromInt {
                                conversion: IntConversion::ToFloat,
                                operand: Box::new(int),
                            },
                            span,
                        ),
                        Dimension::dimensionless(),
                    )),
                    Typed::Real(..) => Err(P::non_constant(span).into()),
                }
            }
            (BuiltinFn::Conversion(ConversionFn::ToFloat), _) => wrong_arity(1),
            (BuiltinFn::IntDivision(function), [dividend, divisor]) => {
                let int_operand = |arg: &ast::Expr| {
                    let typed = self.lower(arg)?;
                    operator_typing::int_division_operand_rule(
                        function,
                        &operand(&typed, arg),
                        self.typing.registry,
                        self.typing.src,
                    )?;
                    match typed {
                        Typed::Int(int) => Ok(Box::new(int)),
                        Typed::Real(..) => Err(ConstLowerError::from(P::non_constant(arg.span))),
                    }
                };
                let dividend = int_operand(dividend)?;
                let divisor = int_operand(divisor)?;
                Ok(Typed::Int(ConstInt::new(
                    ConstIntKind::Division {
                        function,
                        dividend,
                        divisor,
                    },
                    span,
                )))
            }
            (BuiltinFn::IntDivision(_), _) => wrong_arity(2),
            _ => Err(P::non_constant(span).into()),
        }
    }

    /// Lower `lhs op rhs`: type it with the shared binary rule, then build
    /// the `Int` or real node the rule's result type selects.
    fn binop(
        &self,
        span: Span,
        op: BinOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
    ) -> Result<Typed<P>, ConstLowerError> {
        if !matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow(_)
        ) {
            return Err(P::non_constant(span).into());
        }
        let lhs_typed = self.lower(lhs)?;
        // An exact exponent is carried by the operator, not evaluated: it is
        // an `Int` when integral and a `Dimensionless` value otherwise, as in
        // a node body.
        let (rhs_typed, rhs_type) = if let BinOp::Pow(PowerExponent::Exact(exponent)) = op {
            let ty = if exponent.is_integer() {
                CheckedType::Int
            } else {
                CheckedType::Quantity(Dimension::dimensionless())
            };
            (None, ty)
        } else {
            let typed = self.lower(rhs)?;
            let ty = typed.checked_type();
            (Some(typed), ty)
        };
        let rhs_const_int = match (op, &rhs_typed) {
            (BinOp::Pow(_), Some(Typed::Int(exponent))) => exponent.evaluate().ok(),
            _ => None,
        };
        let result = operator_typing::binop_rule(
            span,
            op,
            &Operand {
                ty: lhs_typed.checked_type(),
                span: lhs.span,
            },
            &Operand {
                ty: rhs_type,
                span: rhs.span,
            },
            rhs_const_int,
            self.typing.registry,
            self.typing.src,
        )?;
        let built = match result {
            CheckedType::Int => Self::int_node(op, lhs_typed, rhs_typed, rhs.span)
                .map(|kind| Typed::Int(ConstInt::new(kind, span))),
            CheckedType::Quantity(dimension) => Self::real_node(op, lhs_typed, rhs_typed)
                .map(|kind| Typed::Real(ConstReal::new(kind, span), dimension)),
            _ => None,
        };
        // An operation the rules accept but the constant tree has no node for
        // is not part of the constant sublanguage; it fails loudly instead of
        // being evaluated with other semantics.
        built.ok_or_else(|| P::non_constant(span).into())
    }

    /// The `Int` node for an operation the shared rules typed as `Int`.
    fn int_node(
        op: BinOp,
        lhs: Typed<P>,
        rhs: Option<Typed<P>>,
        rhs_span: Span,
    ) -> Option<ConstIntKind> {
        let Typed::Int(lhs) = lhs else {
            return None;
        };
        let lhs = Box::new(lhs);
        let rhs = match (op, rhs) {
            (BinOp::Pow(PowerExponent::Exact(exponent)), None) => {
                ConstInt::new(ConstIntKind::Literal(exponent.num()), rhs_span)
            }
            (_, Some(Typed::Int(rhs))) => rhs,
            _ => return None,
        };
        let rhs = Box::new(rhs);
        let op = match op {
            BinOp::Add => IntArithOp::Add,
            BinOp::Sub => IntArithOp::Sub,
            BinOp::Mul => IntArithOp::Mul,
            BinOp::Mod => IntArithOp::Mod,
            BinOp::Pow(_) => {
                return Some(ConstIntKind::Power {
                    base: lhs,
                    exponent: rhs,
                });
            }
            _ => return None,
        };
        Some(ConstIntKind::Arith { op, lhs, rhs })
    }

    /// The real node for an operation the shared rules typed as a quantity.
    fn real_node(op: BinOp, lhs: Typed<P>, rhs: Option<Typed<P>>) -> Option<ConstRealKind<P>> {
        // The rules type `^` on an `Int` base as `Int`, so only `*` and `/`
        // reach here with an `Int` on the left.
        let lhs = Box::new(lhs.into_real(IntConversion::Scaling));
        if let BinOp::Pow(PowerExponent::Exact(exponent)) = op {
            return Some(ConstRealKind::ExactPower {
                base: lhs,
                exponent,
            });
        }
        let conversion = match op {
            BinOp::Pow(_) => IntConversion::Exponent,
            _ => IntConversion::Scaling,
        };
        let rhs = Box::new(rhs?.into_real(conversion));
        let op = match op {
            BinOp::Add => ArithOp::Add,
            BinOp::Sub => ArithOp::Sub,
            BinOp::Mul => ArithOp::Mul,
            BinOp::Div => ArithOp::Div,
            BinOp::Pow(_) => {
                return Some(ConstRealKind::Power {
                    base: lhs,
                    exponent: rhs,
                });
            }
            _ => return None,
        };
        Some(ConstRealKind::Arith { op, lhs, rhs })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::desugar::desugared_ast::{DeclKind, File, IndexDeclKind};
    use crate::hir::const_expr::{CoordinateAxisError, CoordinateAxisExpr};
    use crate::semantic_error::SemanticErrorKind;
    use crate::semantic_error::dimension::DimensionError;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> File {
        File::from(Parser::new(source).parse_file().unwrap())
    }

    fn with_typing<T>(f: impl FnOnce(ConstTyping<'_>) -> T) -> T {
        let registry = DimensionFormattingRegistry::new(BTreeMap::new(), []);
        f(ConstTyping {
            registry: &registry,
            src: SourceId::new(0, 0, 0),
        })
    }

    fn scale_expr(source: &str) -> ast::Expr {
        let file = parse(source);
        let Some(DeclKind::Unit(unit)) = file.declarations.first().map(|decl| &decl.kind) else {
            panic!("expected a unit declaration");
        };
        unit.definition().unwrap().scale_expr.clone()
    }

    fn scale(source: &str) -> Result<UnitScaleExpr, ConstLowerError> {
        match classify_unit_scale(&scale_expr(source)) {
            UnitScaleSource::Static(source) => with_typing(|typing| {
                source.lower(
                    &UnitRef::local(crate::syntax::dimension::UnitName::expect_valid("x")),
                    typing,
                )
            }),
            UnitScaleSource::Dynamic { .. } => panic!("expected a static scale"),
        }
    }

    fn scale_value(source: &str) -> f64 {
        scale(source).unwrap().evaluate().unwrap().get()
    }

    fn dimension_error(error: ConstLowerError) -> DimensionError {
        match error {
            ConstLowerError::Type(error) => match *error {
                SemanticError::Located(diagnostic) => match diagnostic.kind {
                    SemanticErrorKind::Dimension(error) => error,
                    other => panic!("expected a dimension error, got {other:?}"),
                },
                other @ SemanticError::Internal(_) => {
                    panic!("expected a located error, got {other:?}")
                }
            },
            ConstLowerError::Expr(error) => panic!("expected a type error, got {error:?}"),
        }
    }

    fn expr_error(error: ConstLowerError) -> ConstExprError {
        match error {
            ConstLowerError::Expr(error) => error,
            ConstLowerError::Type(error) => panic!("expected an expression error, got {error:?}"),
        }
    }

    fn prelude_unit(
        unit: &ast::UnitExpr,
    ) -> Result<(Dimension, PositiveFiniteScale), UnitResolveError> {
        let prelude = crate::ir::prelude_definitions::prelude_definitions().unwrap();
        crate::semantic::unit_scale::resolve_unit_expr_with(unit, |reference| {
            prelude
                .units()
                .find(|(identity, _)| {
                    !reference.is_qualified() && identity.atom() == reference.leaf().atom()
                })
                .map(|(_, info)| info)
        })
    }

    fn axis(source: &str) -> Result<CoordinateAxisExpr, ConstLowerError> {
        let file = parse(source);
        let resolve: &UnitExprResolver<'_> = &prelude_unit;
        let Some(DeclKind::Index(index)) = file.declarations.first().map(|decl| &decl.kind) else {
            panic!("expected an index declaration");
        };
        with_typing(|typing| match &index.kind {
            IndexDeclKind::Range { start, end, step } => Ok(CoordinateAxisExpr::Range {
                start: lower_coordinate_expr(start, resolve, typing)?,
                end: lower_coordinate_expr(end, resolve, typing)?,
                step: lower_coordinate_expr(step, resolve, typing)?,
            }),
            IndexDeclKind::Linspace { start, end, points } => Ok(CoordinateAxisExpr::Linspace {
                start: lower_coordinate_expr(start, resolve, typing)?,
                end: lower_coordinate_expr(end, resolve, typing)?,
                points: lower_static_nat_expr(points)?,
            }),
            _ => panic!("expected a coordinate index"),
        })
    }

    #[test]
    fn classify_reports_the_first_graph_ref() {
        let expr = scale_expr("unit EUR: Length = (@a * @b) m;");
        let UnitScaleSource::Dynamic { first_graph_ref } = classify_unit_scale(&expr) else {
            panic!("expected a dynamic scale");
        };
        assert_eq!(first_graph_ref.value.to_string(), "a");
    }

    #[test]
    fn unit_scales_follow_the_shared_rules() {
        assert!(matches!(
            scale("unit x: Length = 1000 m;"),
            Ok(UnitScaleExpr::Int(_))
        ));
        assert_eq!(scale_value("unit x: Length = 1000 m;"), 1000.0);
        assert_eq!(
            scale_value("unit x: Length = (PI / 180) m;"),
            std::f64::consts::PI / 180.0
        );
        assert_eq!(scale_value("unit x: Length = (2 ^ 3) m;"), 8.0);
        assert_eq!(scale_value("unit x: Length = (2.0 ^ 3) m;"), 8.0);
        assert_eq!(scale_value("unit x: Length = (2 ^ (1 + 2)) m;"), 8.0);
        assert_eq!(scale_value("unit x: Length = (-(-2.5)) m;"), 2.5);
        assert_eq!(scale_value("unit x: Length = (3 - 1 + 1) m;"), 3.0);
        assert_eq!(scale_value("unit x: Length = (7 % 4) m;"), 3.0);
        assert_eq!(scale_value("unit x: Length = (1.0 / 3) m;"), 1.0 / 3.0);
        assert_eq!(scale_value("unit x: Length = (3 * 0.5) m;"), 1.5);
    }

    #[test]
    fn unit_scales_reject_what_the_shared_rules_reject() {
        assert!(matches!(
            dimension_error(scale("unit x: Length = (1 / 3) m;").unwrap_err()),
            DimensionError::IntOperandDivision
        ));
        assert!(matches!(
            dimension_error(scale("unit x: Length = (1.5 + 1) m;").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
        assert!(matches!(
            dimension_error(scale("unit x: Length = (2 ^ (-1)) m;").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
    }

    #[test]
    fn unit_scales_reject_names_and_non_arithmetic_constructs() {
        assert!(matches!(
            expr_error(scale("unit x: Length = (FOO * 2) m;").unwrap_err()),
            ConstExprError::UnknownScaleConstant { .. }
        ));
        assert!(matches!(
            expr_error(scale("unit x: Length = (sqrt(4.0)) m;").unwrap_err()),
            ConstExprError::NonConstantScale { .. }
        ));
        assert!(matches!(
            expr_error(scale("unit x: Length = (1 < 2) m;").unwrap_err()),
            ConstExprError::NonConstantScale { .. }
        ));
    }

    #[test]
    fn unit_scales_accept_the_explicit_int_built_ins() {
        assert_eq!(
            scale_value("unit x: Length = (to_float(1) / 3) m;"),
            1.0 / 3.0
        );
        assert_eq!(scale_value("unit x: Length = (div_floor(7, 2)) m;"), 3.0);
        assert!(matches!(
            dimension_error(scale("unit x: Length = (to_float(1.0)) m;").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
        assert!(matches!(
            dimension_error(scale("unit x: Length = (div_trunc(7.0, 2)) m;").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
        assert!(matches!(
            scale("unit x: Length = (div_trunc(7)) m;").unwrap_err(),
            ConstLowerError::Type(error) if matches!(
                *error,
                SemanticError::Located(crate::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Name(crate::semantic_error::name::NameError::WrongArity { .. }),
                    ..
                })
            )
        ));
        assert!(matches!(
            scale("unit x: Length = (div_ceil(1, 0)) m;")
                .unwrap()
                .evaluate(),
            Err(ConstExprError::IntDivision { .. })
        ));
        assert!(matches!(
            scale("unit x: Length = (to_float(9007199254740993)) m;")
                .unwrap()
                .evaluate(),
            Err(ConstExprError::ToFloat { .. })
        ));
    }

    #[test]
    fn unit_scale_evaluation_is_checked() {
        let error = |source| scale(source).unwrap().evaluate().unwrap_err();
        assert!(matches!(
            error("unit x: Length = 9007199254740993 m;"),
            ConstExprError::IntUnitScale { .. }
        ));
        assert!(matches!(
            error("unit x: Length = (9007199254740993 * 1.0) m;"),
            ConstExprError::IntScalar { .. }
        ));
        assert!(matches!(
            error("unit x: Length = (9223372036854775807 + 1) m;"),
            ConstExprError::Int { .. }
        ));
        assert!(matches!(
            error("unit x: Length = (1e308 * 10.0 / 100.0) m;"),
            ConstExprError::Arithmetic { .. }
        ));
    }

    #[test]
    fn coordinates_resolve_units_and_reject_non_static_arguments() {
        let range = axis("index T = range(0.0 s, 1.0 min, step: 30.0 s);").unwrap();
        assert_eq!(range.evaluate().unwrap().cardinality(), 3);
        assert!(matches!(
            expr_error(axis("index T = range(0.0 s, 1.0 furlongs, step: 1.0 s);").unwrap_err()),
            ConstExprError::Unit { .. }
        ));
        assert!(matches!(
            axis("index T = range(0.0, 1e308 * 10.0, step: 1.0);")
                .unwrap()
                .evaluate(),
            Err(CoordinateAxisError::Expr(ConstExprError::Arithmetic { .. }))
        ));
        assert!(matches!(
            expr_error(axis("index T = range(0.0, FOO, step: 1.0);").unwrap_err()),
            ConstExprError::UnknownCoordinateConstant { .. }
        ));
        assert!(matches!(
            expr_error(axis("index T = range(0.0, abs(1.0), step: 1.0);").unwrap_err()),
            ConstExprError::NonStaticCoordinate { .. }
        ));
    }

    #[test]
    fn coordinates_follow_the_shared_rules() {
        let scaled = axis("index T = range(0.0 s, 2 * 30.0 s, step: 30.0 s);").unwrap();
        assert_eq!(scaled.evaluate().unwrap().cardinality(), 3);
        let power = axis("index T = range(0.0, 2.0 ^ 2, step: 1.0);").unwrap();
        assert_eq!(power.evaluate().unwrap().cardinality(), 5);
        assert!(matches!(
            dimension_error(axis("index T = range(0, 1.0, step: 1.0);").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
        assert!(matches!(
            dimension_error(axis("index T = range(0.0 s, 1.0 s + 1.0, step: 1.0 s);").unwrap_err()),
            DimensionError::DimensionMismatch { .. }
        ));
    }

    #[test]
    fn linspace_point_counts_must_be_closed() {
        let linspace = axis("index T = linspace(0.0, 1.0, points: 2 * 2 + 1);").unwrap();
        assert_eq!(linspace.evaluate().unwrap().cardinality(), 5);
        assert!(matches!(
            expr_error(axis("index T = linspace(0.0, 1.0, points: N + 1);").unwrap_err()),
            ConstExprError::NonConstantNat { .. }
        ));
    }
}

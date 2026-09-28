//! Lowering of constant-expression positions from the desugared AST into
//! [`crate::hir::const_expr`] trees.
//!
//! Constant positions are resolved against the static unit registry being
//! built for the declaring module, not against module-level Term
//! declarations: a name is either a [`BuiltinConst`] or an error, and a unit
//! literal must name a unit with a static scale.

use crate::builtin::BuiltinConst;
use crate::desugar::desugared_ast::{self as ast, BinOp, UnaryOp};
use crate::hir::const_expr::{
    ConstArithOp, ConstExponent, ConstExpr, ConstExprError, ConstExprKind, ConstUnit,
    CoordinateExpr, CoordinatePosition, UnitScaleExpr, UnitScalePosition,
};
use crate::hir::types::NatExpr;
use crate::registry::types::RegistryBuilder;
use crate::syntax::ast::{PowerExponent, UnresolvedRef};
use crate::syntax::module_name::ScopedName;
use crate::syntax::span::Spanned;
use crate::syntax::visitor::ExprVisitor;

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
                self.0 = Some(name.clone());
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

impl StaticScaleSource<'_> {
    /// Lower the scale expression into its constant HIR tree.
    ///
    /// # Errors
    ///
    /// Returns a [`ConstExprError`] for a name that is not a built-in
    /// constant, a non-arithmetic operator, or any other non-constant
    /// construct. Errors are reported in source order.
    pub fn lower(self) -> Result<UnitScaleExpr, ConstExprError> {
        lower_unit_scale_expr(self.0).map(UnitScaleExpr::new)
    }
}

fn builtin_constant(reference: &UnresolvedRef) -> Option<BuiltinConst> {
    match reference {
        UnresolvedRef::Path(path) => path
            .as_bare()
            .and_then(|ident| BuiltinConst::parse(ident.name.as_str())),
        UnresolvedRef::IndexLabel { .. } => None,
    }
}

fn lower_unit_scale_expr(expr: &ast::Expr) -> Result<ConstExpr<UnitScalePosition>, ConstExprError> {
    let kind = match &expr.kind {
        ast::ExprKind::Number(value) => ConstExprKind::Number(*value),
        ast::ExprKind::Integer(value) => ConstExprKind::Integer(*value, ()),
        ast::ExprKind::UnresolvedRef(reference @ UnresolvedRef::Path(path)) => {
            ConstExprKind::Builtin(builtin_constant(reference).ok_or_else(|| {
                ConstExprError::UnknownScaleConstant {
                    path: path.clone().into_spanned_name_path(),
                }
            })?)
        }
        ast::ExprKind::BinOp { op, lhs, rhs } => {
            let lhs = Box::new(lower_unit_scale_expr(lhs)?);
            if let BinOp::Pow(PowerExponent::Exact(exponent)) = op {
                ConstExprKind::Power {
                    base: lhs,
                    exponent: ConstExponent::Exact(*exponent),
                    admitted: (),
                }
            } else {
                let rhs = Box::new(lower_unit_scale_expr(rhs)?);
                match op {
                    BinOp::Add => arith(ConstArithOp::Add, lhs, rhs),
                    BinOp::Sub => arith(ConstArithOp::Sub, lhs, rhs),
                    BinOp::Mul => arith(ConstArithOp::Mul, lhs, rhs),
                    BinOp::Div => arith(ConstArithOp::Div, lhs, rhs),
                    BinOp::Pow(_) => ConstExprKind::Power {
                        base: lhs,
                        exponent: ConstExponent::Evaluated(rhs),
                        admitted: (),
                    },
                    BinOp::Mod
                    | BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Gt
                    | BinOp::Le
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => {
                        return Err(ConstExprError::UnsupportedScaleOperator {
                            op: *op,
                            span: expr.span,
                        });
                    }
                }
            }
        }
        ast::ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => ConstExprKind::Neg(Box::new(lower_unit_scale_expr(operand)?)),
        _ => return Err(ConstExprError::NonConstantScale { span: expr.span }),
    };
    Ok(ConstExpr::new(kind, expr.span))
}

const fn arith<P: crate::hir::const_expr::ConstPosition>(
    op: ConstArithOp,
    lhs: Box<ConstExpr<P>>,
    rhs: Box<ConstExpr<P>>,
) -> ConstExprKind<P> {
    ConstExprKind::Arith { op, lhs, rhs }
}

/// Lower a coordinate bound or step, resolving unit literals against the
/// static unit registry.
///
/// # Errors
///
/// Returns a [`ConstExprError`] for a non-finite literal, an unresolvable or
/// dynamic unit, a name that is not a built-in constant, an `Int` value, a
/// non-arithmetic operator, or any runtime construct. Errors are reported in
/// source order.
pub fn lower_coordinate_expr(
    expr: &ast::Expr,
    registry: &RegistryBuilder,
) -> Result<CoordinateExpr, ConstExprError> {
    let kind: ConstExprKind<CoordinatePosition> = match &expr.kind {
        ast::ExprKind::Number(value) if value.is_finite() => ConstExprKind::Number(*value),
        ast::ExprKind::Number(value) => {
            return Err(ConstExprError::NonFiniteCoordinate {
                value: *value,
                span: expr.span,
            });
        }
        ast::ExprKind::QuantityLiteral { value, unit } => {
            let (dimension, scale) =
                registry
                    .resolve_unit_expr(unit)
                    .map_err(|error| ConstExprError::Unit {
                        error,
                        span: unit.span,
                    })?;
            ConstExprKind::Quantity {
                value: *value,
                unit: ConstUnit::new(unit.clone(), dimension, scale),
                admitted: (),
            }
        }
        ast::ExprKind::UnresolvedRef(reference @ UnresolvedRef::Path(path)) => {
            ConstExprKind::Builtin(builtin_constant(reference).ok_or_else(|| {
                ConstExprError::UnknownCoordinateConstant {
                    path: path.clone().into_spanned_name_path(),
                }
            })?)
        }
        ast::ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => ConstExprKind::Neg(Box::new(lower_coordinate_expr(operand, registry)?)),
        ast::ExprKind::BinOp { op, lhs, rhs } => {
            let lhs = Box::new(lower_coordinate_expr(lhs, registry)?);
            let rhs = Box::new(lower_coordinate_expr(rhs, registry)?);
            match op {
                BinOp::Add => arith(ConstArithOp::Add, lhs, rhs),
                BinOp::Sub => arith(ConstArithOp::Sub, lhs, rhs),
                BinOp::Mul => arith(ConstArithOp::Mul, lhs, rhs),
                BinOp::Div => arith(ConstArithOp::Div, lhs, rhs),
                BinOp::Mod
                | BinOp::Pow(_)
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => {
                    return Err(ConstExprError::UnsupportedCoordinateOperator { span: expr.span });
                }
            }
        }
        _ => return Err(ConstExprError::NonStaticCoordinate { span: expr.span }),
    };
    Ok(ConstExpr::new(kind, expr.span))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desugar::desugared_ast::{DeclKind, File, IndexDeclKind};
    use crate::hir::const_expr::CoordinateAxisExpr;
    use crate::registry::prelude::load_prelude;
    use crate::registry::unit::PositiveFiniteScale;
    use crate::syntax::parser::Parser;

    fn parse(source: &str) -> File {
        File::from(Parser::new(source).parse_file().unwrap())
    }

    fn scale_expr(source: &str) -> ast::Expr {
        let file = parse(source);
        let Some(DeclKind::Unit(unit)) = file.declarations.first().map(|decl| &decl.kind) else {
            panic!("expected a unit declaration");
        };
        unit.definition.as_ref().unwrap().scale_expr.clone()
    }

    fn scale_value(source: &str) -> Result<f64, ConstExprError> {
        match classify_unit_scale(&scale_expr(source)) {
            UnitScaleSource::Static(source) => {
                source.lower()?.evaluate().map(PositiveFiniteScale::get)
            }
            UnitScaleSource::Dynamic { .. } => panic!("expected a static scale"),
        }
    }

    fn prelude() -> RegistryBuilder {
        let mut registry = RegistryBuilder::new();
        load_prelude(&mut registry).unwrap();
        registry
    }

    fn axis(source: &str) -> Result<CoordinateAxisExpr, ConstExprError> {
        let file = parse(source);
        let registry = prelude();
        let Some(DeclKind::Index(index)) = file.declarations.first().map(|decl| &decl.kind) else {
            panic!("expected an index declaration");
        };
        match &index.kind {
            IndexDeclKind::Range { start, end, step } => Ok(CoordinateAxisExpr::Range {
                start: lower_coordinate_expr(start, &registry)?,
                end: lower_coordinate_expr(end, &registry)?,
                step: lower_coordinate_expr(step, &registry)?,
            }),
            IndexDeclKind::Linspace { start, end, points } => Ok(CoordinateAxisExpr::Linspace {
                start: lower_coordinate_expr(start, &registry)?,
                end: lower_coordinate_expr(end, &registry)?,
                points: lower_static_nat_expr(points)?,
            }),
            _ => panic!("expected a coordinate index"),
        }
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
    fn unit_scales_accept_the_constant_sublanguage() {
        assert_eq!(scale_value("unit x: Length = 1000 m;"), Ok(1000.0));
        assert_eq!(
            scale_value("unit x: Length = (PI / 180) m;"),
            Ok(std::f64::consts::PI / 180.0)
        );
        assert_eq!(scale_value("unit x: Length = (2 ^ 3) m;"), Ok(8.0));
        assert_eq!(scale_value("unit x: Length = (-(-2.5)) m;"), Ok(2.5));
        assert_eq!(scale_value("unit x: Length = (3 - 1 + 1) m;"), Ok(3.0));
    }

    #[test]
    fn unit_scales_reject_names_operators_and_other_constructs() {
        assert!(matches!(
            scale_value("unit x: Length = (FOO * 2) m;"),
            Err(ConstExprError::UnknownScaleConstant { .. })
        ));
        assert!(matches!(
            scale_value("unit x: Length = (5 % 3) m;"),
            Err(ConstExprError::UnsupportedScaleOperator { op: BinOp::Mod, .. })
        ));
        assert!(matches!(
            scale_value("unit x: Length = (sqrt(4.0)) m;"),
            Err(ConstExprError::NonConstantScale { .. })
        ));
    }

    #[test]
    fn coordinates_resolve_units_and_reject_non_static_arguments() {
        let range = axis("index T = range(0.0 s, 1.0 min, step: 30.0 s);").unwrap();
        assert_eq!(range.evaluate().unwrap().cardinality(), 3);
        assert!(matches!(
            axis("index T = range(0.0 s, 1.0 furlongs, step: 1.0 s);"),
            Err(ConstExprError::Unit { .. })
        ));
        assert!(matches!(
            axis("index T = range(0, 1.0, step: 1.0);"),
            Err(ConstExprError::NonStaticCoordinate { .. })
        ));
        assert!(matches!(
            axis("index T = range(0.0, 1e308 * 10.0, step: 1.0);")
                .unwrap()
                .evaluate(),
            Err(crate::hir::const_expr::CoordinateAxisError::Expr(
                ConstExprError::NonFiniteCoordinate { .. }
            ))
        ));
        assert!(matches!(
            axis("index T = range(0.0, FOO, step: 1.0);"),
            Err(ConstExprError::UnknownCoordinateConstant { .. })
        ));
        assert!(matches!(
            axis("index T = range(0.0, 2.0 ^ 2.0, step: 1.0);"),
            Err(ConstExprError::UnsupportedCoordinateOperator { .. })
        ));
    }

    #[test]
    fn linspace_point_counts_must_be_closed() {
        let linspace = axis("index T = linspace(0.0, 1.0, points: 2 * 2 + 1);").unwrap();
        assert_eq!(linspace.evaluate().unwrap().cardinality(), 5);
        assert!(matches!(
            axis("index T = linspace(0.0, 1.0, points: N + 1);"),
            Err(ConstExprError::NonConstantNat { .. })
        ));
    }
}

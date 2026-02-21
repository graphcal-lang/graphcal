#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]
use super::*;
use crate::ast::{
    BinOp, DeclKind, ExprKind, FnBody, GenericConstraint, IndexDeclKind, MulDivOp, TypeExprKind,
    UnaryOp,
};

// --- Phase 1 declaration tests ---

#[test]
fn parse_param_with_type() {
    let file = Parser::new("param x: Dimensionless = 42.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Param(p) => {
            assert_eq!(p.name.value.as_str(), "x");
            assert!(matches!(p.type_ann.kind, TypeExprKind::Dimensionless));
            assert!(matches!(p.value.kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON));
        }
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_param_with_dim_type() {
    let file = Parser::new("param alt: Length = 400.0 km;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => {
            assert_eq!(p.name.value.as_str(), "alt");
            match &p.type_ann.kind {
                TypeExprKind::DimExpr(d) => {
                    assert_eq!(d.terms.len(), 1);
                    assert_eq!(d.terms[0].term.name.name, "Length");
                }
                other => panic!("expected DimExpr, got {other:?}"),
            }
            assert!(matches!(p.value.kind, ExprKind::UnitLiteral { .. }));
        }
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_node_with_compound_dim_type() {
    let file = Parser::new("node gm: Length^3 / Time^2 = 3.98e14 m^3/s^2;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => {
            assert_eq!(n.name.value.as_str(), "gm");
            match &n.type_ann.kind {
                TypeExprKind::DimExpr(d) => {
                    assert_eq!(d.terms.len(), 2);
                    assert_eq!(d.terms[0].term.name.name, "Length");
                    assert_eq!(d.terms[0].term.power, Some(3));
                    assert_eq!(d.terms[1].op, MulDivOp::Div);
                    assert_eq!(d.terms[1].term.name.name, "Time");
                    assert_eq!(d.terms[1].term.power, Some(2));
                }
                other => panic!("expected DimExpr, got {other:?}"),
            }
        }
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_const_with_type() {
    let file = Parser::new("const G0: Dimensionless = 9.80665;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Const(c) => {
            assert_eq!(c.name.value.as_str(), "G0");
            assert!(matches!(c.type_ann.kind, TypeExprKind::Dimensionless));
        }
        _ => panic!("expected const"),
    }
}

// --- Dimension declarations ---

#[test]
fn parse_base_dimension() {
    let file = Parser::new("dimension Length;").parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Dimension(d) => {
            assert_eq!(d.name.value.as_str(), "Length");
            assert!(d.definition.is_none());
        }
        _ => panic!("expected dimension"),
    }
}

#[test]
fn parse_derived_dimension() {
    let file = Parser::new("dimension Velocity = Length / Time;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Dimension(d) => {
            assert_eq!(d.name.value.as_str(), "Velocity");
            let def = d.definition.as_ref().unwrap();
            assert_eq!(def.terms.len(), 2);
            assert_eq!(def.terms[0].term.name.name, "Length");
            assert_eq!(def.terms[1].op, MulDivOp::Div);
            assert_eq!(def.terms[1].term.name.name, "Time");
        }
        _ => panic!("expected dimension"),
    }
}

// --- Unit declarations ---

#[test]
fn parse_base_unit() {
    let file = Parser::new("unit m: Length;").parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Unit(u) => {
            assert_eq!(u.name.value.as_str(), "m");
            assert_eq!(u.dim_type.terms[0].term.name.name, "Length");
            assert!(u.definition.is_none());
        }
        _ => panic!("expected unit"),
    }
}

#[test]
fn parse_derived_unit() {
    let file = Parser::new("unit km: Length = 1000.0 m;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Unit(u) => {
            assert_eq!(u.name.value.as_str(), "km");
            let def = u.definition.as_ref().unwrap();
            assert!((def.scale - 1000.0).abs() < f64::EPSILON);
            assert_eq!(def.unit_expr.terms.len(), 1);
            assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "m");
        }
        _ => panic!("expected unit"),
    }
}

#[test]
fn parse_compound_unit_decl() {
    let file = Parser::new("unit N: Force = 1.0 kg * m / s^2;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Unit(u) => {
            assert_eq!(u.name.value.as_str(), "N");
            let def = u.definition.as_ref().unwrap();
            assert!((def.scale - 1.0).abs() < f64::EPSILON);
            assert_eq!(def.unit_expr.terms.len(), 3);
            assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "kg");
            assert_eq!(def.unit_expr.terms[1].op, MulDivOp::Mul);
            assert_eq!(def.unit_expr.terms[1].name.value.as_str(), "m");
            assert_eq!(def.unit_expr.terms[2].op, MulDivOp::Div);
            assert_eq!(def.unit_expr.terms[2].name.value.as_str(), "s");
            assert_eq!(def.unit_expr.terms[2].power, Some(2));
        }
        _ => panic!("expected unit"),
    }
}

#[test]
fn parse_unit_decl_with_paren_expr() {
    let file = Parser::new("unit deg: Angle = (PI / 180) rad;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Unit(u) => {
            assert_eq!(u.name.value.as_str(), "deg");
            let def = u.definition.as_ref().unwrap();
            assert!(
                (def.scale - std::f64::consts::PI / 180.0).abs() < 1e-10,
                "scale = {}",
                def.scale
            );
            assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "rad");
        }
        _ => panic!("expected unit"),
    }
}

// --- Unit literals ---

#[test]
fn parse_unit_literal() {
    let file = Parser::new("param alt: Length = 400.0 km;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.value.kind {
            ExprKind::UnitLiteral { value, unit } => {
                assert!((value - 400.0).abs() < f64::EPSILON);
                assert_eq!(unit.terms.len(), 1);
                assert_eq!(unit.terms[0].name.value.as_str(), "km");
            }
            _ => panic!("expected UnitLiteral"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_compound_unit_literal() {
    let file = Parser::new("const G0: Acceleration = 9.80665 m/s^2;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Const(c) => match &c.value.kind {
            ExprKind::UnitLiteral { value, unit } => {
                assert!((value - 9.80665).abs() < f64::EPSILON);
                assert_eq!(unit.terms.len(), 2);
                assert_eq!(unit.terms[0].name.value.as_str(), "m");
                assert_eq!(unit.terms[1].op, MulDivOp::Div);
                assert_eq!(unit.terms[1].name.value.as_str(), "s");
                assert_eq!(unit.terms[1].power, Some(2));
            }
            _ => panic!("expected UnitLiteral"),
        },
        _ => panic!("expected const"),
    }
}

// --- Conversion ---

#[test]
fn parse_conversion() {
    let file = Parser::new("node speed_kmh: Velocity = @speed -> km/hour;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Convert { expr, target } => {
                assert!(
                    matches!(&expr.kind, ExprKind::GraphRef(id) if id.value.as_str() == "speed")
                );
                assert_eq!(target.terms.len(), 2);
                assert_eq!(target.terms[0].name.value.as_str(), "km");
                assert_eq!(target.terms[1].op, MulDivOp::Div);
                assert_eq!(target.terms[1].name.value.as_str(), "hour");
            }
            _ => panic!("expected Convert"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_convert_binds_loosely() {
    // @a + @b -> km should be (@a + @b) -> km
    let file = Parser::new("node x: Length = @a + @b -> km;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Convert { expr, target } => {
                assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                assert_eq!(target.terms[0].name.value.as_str(), "km");
            }
            _ => panic!("expected Convert"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_as_cast() {
    // @v as Vec3<Length, Eci> should parse as AsCast
    let source = r"
        type Eci {}
        type Vec3<D: Dim, F: Type> { x: D, y: D, z: D, }
        node x: Vec3<Length, Eci> = @v as Vec3<Length, Eci>;
    ";
    let file = Parser::new(source).parse_file().unwrap();
    // The node is the 3rd declaration (after type Eci, type Vec3)
    match &file.declarations[2].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::AsCast { expr, target_type } => {
                assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                match &target_type.kind {
                    TypeExprKind::TypeApplication { name, type_args } => {
                        assert_eq!(name.name.as_str(), "Vec3");
                        assert_eq!(type_args.len(), 2);
                    }
                    other => panic!("expected TypeApplication, got {other:?}"),
                }
            }
            other => panic!("expected AsCast, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_as_cast_binds_loosely() {
    // @a + @b as Vec3<Length, Eci> should be (@a + @b) as Vec3<Length, Eci>
    let source = r"
        type Eci {}
        type Vec3<D: Dim, F: Type> { x: D, y: D, z: D, }
        node x: Vec3<Length, Eci> = @a + @b as Vec3<Length, Eci>;
    ";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[2].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::AsCast { expr, target_type } => {
                assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
                match &target_type.kind {
                    TypeExprKind::TypeApplication { name, .. } => {
                        assert_eq!(name.name.as_str(), "Vec3");
                    }
                    other => panic!("expected TypeApplication, got {other:?}"),
                }
            }
            other => panic!("expected AsCast, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

// --- Expression parsing (preserved from Phase 0) ---

/// Helper: parse a single node declaration and return its expression.
fn parse_node_expr(input: &str) -> crate::ast::Expr {
    let full = format!("node x: Dimensionless = {input};");
    let file = Parser::new(&full).parse_file().unwrap();
    match file.declarations.into_iter().next().unwrap().kind {
        DeclKind::Node(n) => n.value,
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_arithmetic_precedence() {
    let expr = parse_node_expr("1.0 + 2.0 * 3.0");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
    if let ExprKind::BinOp { rhs, .. } = &expr.kind {
        assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Mul, .. }));
    }
}

#[test]
fn parse_left_associative_add() {
    let expr = parse_node_expr("1.0 - 2.0 - 3.0");
    if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
        assert_eq!(*op, BinOp::Sub);
        assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Sub, .. }));
    } else {
        panic!("expected BinOp");
    }
}

#[test]
fn parse_power_right_assoc() {
    let expr = parse_node_expr("2.0 ^ 3.0 ^ 2.0");
    if let ExprKind::BinOp { op, rhs, .. } = &expr.kind {
        assert_eq!(*op, BinOp::Pow);
        assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Pow, .. }));
    } else {
        panic!("expected Pow");
    }
}

#[test]
fn parse_neg_power_precedence() {
    let expr = parse_node_expr("-@x ^ 2.0");
    if let ExprKind::UnaryOp {
        op: UnaryOp::Neg,
        operand,
    } = &expr.kind
    {
        assert!(matches!(
            operand.kind,
            ExprKind::BinOp { op: BinOp::Pow, .. }
        ));
    } else {
        panic!("expected Neg(Pow(...))");
    }
}

#[test]
fn parse_graph_ref() {
    let expr = parse_node_expr("@x + 1.0");
    if let ExprKind::BinOp { lhs, .. } = &expr.kind {
        assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.as_str() == "x"));
    } else {
        panic!("expected BinOp");
    }
}

#[test]
fn parse_const_ref() {
    let expr = parse_node_expr("PI * 2.0");
    if let ExprKind::BinOp { lhs, .. } = &expr.kind {
        assert!(matches!(&lhs.kind, ExprKind::ConstRef(id) if id.value.as_str() == "PI"));
    } else {
        panic!("expected BinOp");
    }
}

#[test]
fn parse_function_call_one_arg() {
    let expr = parse_node_expr("sqrt(@x)");
    if let ExprKind::FnCall { name, args } = &expr.kind {
        assert_eq!(name.value.as_str(), "sqrt");
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0].kind, ExprKind::GraphRef(id) if id.value.as_str() == "x"));
    } else {
        panic!("expected FnCall");
    }
}

#[test]
fn parse_function_call_two_args() {
    let expr = parse_node_expr("atan2(@a, @b)");
    if let ExprKind::FnCall { name, args } = &expr.kind {
        assert_eq!(name.value.as_str(), "atan2");
        assert_eq!(args.len(), 2);
    } else {
        panic!("expected FnCall");
    }
}

#[test]
fn parse_function_call_zero_args() {
    let expr = parse_node_expr("foo()");
    if let ExprKind::FnCall { name, args } = &expr.kind {
        assert_eq!(name.value.as_str(), "foo");
        assert_eq!(args.len(), 0);
    } else {
        panic!("expected FnCall");
    }
}

#[test]
fn parse_if_else() {
    let expr = parse_node_expr("if @x > 0.0 { @x } else { 0.0 }");
    if let ExprKind::If {
        condition,
        then_branch,
        else_branch,
    } = &expr.kind
    {
        assert!(matches!(
            condition.kind,
            ExprKind::BinOp { op: BinOp::Gt, .. }
        ));
        assert!(matches!(
            &then_branch.kind,
            ExprKind::GraphRef(id) if id.value.as_str() == "x"
        ));
        assert!(matches!(else_branch.kind, ExprKind::Number(_)));
    } else {
        panic!("expected If");
    }
}

#[test]
fn parse_nested_parens() {
    let expr = parse_node_expr("(1.0 + 2.0) * 3.0");
    if let ExprKind::BinOp { op, lhs, .. } = &expr.kind {
        assert_eq!(*op, BinOp::Mul);
        assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Add, .. }));
    } else {
        panic!("expected Mul");
    }
}

#[test]
fn parse_boolean_and() {
    let expr = parse_node_expr("@a > 0.0 && @b > 0.0");
    if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
        assert_eq!(*op, BinOp::And);
        assert!(matches!(lhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
        assert!(matches!(rhs.kind, ExprKind::BinOp { op: BinOp::Gt, .. }));
    } else {
        panic!("expected And");
    }
}

#[test]
fn parse_boolean_or() {
    let expr = parse_node_expr("@a > 0.0 || @b > 0.0");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Or, .. }));
}

#[test]
fn parse_unary_neg() {
    let expr = parse_node_expr("-1.0");
    assert!(matches!(
        expr.kind,
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            ..
        }
    ));
}

#[test]
fn parse_unary_not() {
    let expr = parse_node_expr("!true");
    assert!(matches!(
        expr.kind,
        ExprKind::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}

#[test]
fn parse_complex_expression() {
    let expr = parse_node_expr("@v_exhaust * ln(@mass_ratio)");
    if let ExprKind::BinOp { op, lhs, rhs } = &expr.kind {
        assert_eq!(*op, BinOp::Mul);
        assert!(matches!(&lhs.kind, ExprKind::GraphRef(id) if id.value.as_str() == "v_exhaust"));
        assert!(matches!(&rhs.kind, ExprKind::FnCall { name, .. } if name.value.as_str() == "ln"));
    } else {
        panic!("expected Mul");
    }
}

#[test]
fn parse_comparison_eq() {
    let expr = parse_node_expr("@x == 1.0");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Eq, .. }));
}

#[test]
fn parse_comparison_ne() {
    let expr = parse_node_expr("@x != 1.0");
    assert!(matches!(expr.kind, ExprKind::BinOp { op: BinOp::Ne, .. }));
}

// --- Error tests ---

#[test]
fn parse_error_missing_semicolon() {
    let result = Parser::new("param x: Dimensionless = 1.0").parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_error_unexpected_token() {
    let result = Parser::new("+ 1.0;").parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_with_comments() {
    let input = "// this is a comment\nparam x: Dimensionless = 1.0;\n// another comment";
    let file = Parser::new(input).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
}

#[test]
fn parse_error_bad_param_casing() {
    let result = Parser::new("param BadName: Dimensionless = 1.0;").parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_error_bad_const_casing() {
    let result = Parser::new("const bad_name: Dimensionless = 42.0;").parse_file();
    assert!(result.is_err());
}

// --- Milestone: orbital.gcl syntax ---

#[test]
fn parse_orbital_milestone_syntax() {
    let source = r"
dimension Velocity = Length / Time;

param alt: Length = 400.0 km;
param period: Time = 90.0 min;
const R_EARTH: Length = 6371.0 km;

node circumference: Length = 2.0 * PI * (R_EARTH + @alt);
node speed: Velocity = @circumference / @period;
node speed_kmh: Velocity = @speed -> km/hour;
";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 7);

    let names: Vec<&str> = file
        .declarations
        .iter()
        .map(|d| match &d.kind {
            DeclKind::Param(p) => p.name.value.as_str(),
            DeclKind::Node(n) => n.name.value.as_str(),
            DeclKind::Const(c) => c.name.value.as_str(),
            DeclKind::Dimension(d) => d.name.value.as_str(),
            DeclKind::Unit(u) => u.name.value.as_str(),
            DeclKind::Type(t) => t.name.value.as_str(),
            DeclKind::Fn(f) => f.name.value.as_str(),
            DeclKind::Index(i) => i.name.value.as_str(),
            DeclKind::Use(_) => "<use>",
            DeclKind::Assert(a) => a.name.value.as_str(),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "Velocity",
            "alt",
            "period",
            "R_EARTH",
            "circumference",
            "speed",
            "speed_kmh"
        ]
    );
}

// --- Phase 2 type declaration tests ---

#[test]
fn parse_type_decl_single_field() {
    let source = "type Orbit { sma: Length }";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Orbit");
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].name.value.as_str(), "Orbit");
            assert_eq!(t.variants[0].fields.len(), 1);
            assert_eq!(t.variants[0].fields[0].name.value.as_str(), "sma");
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_multiple_fields() {
    let source = "type TransferResult { dv1: Velocity, dv2: Velocity }";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "TransferResult");
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].fields.len(), 2);
            assert_eq!(t.variants[0].fields[0].name.value.as_str(), "dv1");
            assert_eq!(t.variants[0].fields[1].name.value.as_str(), "dv2");
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_trailing_comma() {
    let source = "type TransferResult { dv1: Velocity, dv2: Velocity, }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].fields.len(), 2);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_empty_type() {
    let source = "type Eci {}";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Eci");
            assert_eq!(t.variants.len(), 0);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_uppercase_name_error() {
    let source = "type ORBIT { sma: Length }";
    let result = Parser::new(source).parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_type_decl_lowercase_name_error() {
    let source = "type orbit { sma: Length }";
    let result = Parser::new(source).parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_type_decl_with_dim_expr_field() {
    let source = "type TransferResult { dv: Length / Time }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].fields.len(), 1);
            assert_eq!(t.variants[0].fields[0].name.value.as_str(), "dv");
            match &t.variants[0].fields[0].type_ann.kind {
                TypeExprKind::DimExpr(_) => {}
                other => {
                    panic!("expected DimExpr, got {other:?}")
                }
            }
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_mixed_with_other_decls() {
    let source = r"
dimension Velocity = Length / Time;
type TransferResult { dv1: Velocity, dv2: Velocity }
param alt: Length = 400.0 km;
";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 3);
    assert!(matches!(&file.declarations[0].kind, DeclKind::Dimension(_)));
    assert!(matches!(&file.declarations[1].kind, DeclKind::Type(_)));
    assert!(matches!(&file.declarations[2].kind, DeclKind::Param(_)));
}

#[test]
fn parse_type_decl_generic_params() {
    let source = "type Vec3<D: Dim, F: Type> { x: D, y: D, z: D }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Vec3");
            assert_eq!(t.generic_params.len(), 2);
            assert_eq!(t.generic_params[0].name.value.as_str(), "D");
            assert_eq!(t.generic_params[0].constraint, GenericConstraint::Dim);
            assert_eq!(t.generic_params[1].name.value.as_str(), "F");
            assert_eq!(t.generic_params[1].constraint, GenericConstraint::Type);
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].fields.len(), 3);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_no_generics_empty() {
    let source = "type Eci {}";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Eci");
            assert!(t.generic_params.is_empty());
            assert_eq!(t.variants.len(), 0);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_generic_single_type_param() {
    let source = "type Timestamp<TZ: Type> { epoch_seconds: Time }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Timestamp");
            assert_eq!(t.generic_params.len(), 1);
            assert_eq!(t.generic_params[0].name.value.as_str(), "TZ");
            assert_eq!(t.generic_params[0].constraint, GenericConstraint::Type);
            assert_eq!(t.variants.len(), 1);
            assert_eq!(t.variants[0].fields.len(), 1);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_generic_tagged_union() {
    let source = "type Result<D: Dim, E: Type> { Ok { value: D } Err }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Result");
            assert_eq!(t.generic_params.len(), 2);
            assert_eq!(t.variants.len(), 2);
            assert_eq!(t.variants[0].name.value.as_str(), "Ok");
            assert_eq!(t.variants[0].fields.len(), 1);
            assert_eq!(t.variants[1].name.value.as_str(), "Err");
            assert_eq!(t.variants[1].fields.len(), 0);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_generic_default_type_param() {
    let source = "type Vec3<D: Dim, F: Type = Unframed> { x: D, y: D, z: D }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Vec3");
            assert_eq!(t.generic_params.len(), 2);
            assert_eq!(t.generic_params[0].name.value.as_str(), "D");
            assert_eq!(t.generic_params[0].constraint, GenericConstraint::Dim);
            assert!(t.generic_params[0].default.is_none());
            assert_eq!(t.generic_params[1].name.value.as_str(), "F");
            assert_eq!(t.generic_params[1].constraint, GenericConstraint::Type);
            let default = t.generic_params[1].default.as_ref().unwrap();
            assert_eq!(dim_expr_name(default), "Unframed");
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_generic_no_default() {
    let source = "type Pair<A: Dim, B: Dim> { a: A, b: B }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.generic_params.len(), 2);
            assert!(t.generic_params[0].default.is_none());
            assert!(t.generic_params[1].default.is_none());
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_derive_clause() {
    let source = "type Vec3<D: Dim, F: Type> derive(Add, Sub, Neg) { x: D, y: D, z: D }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Vec3");
            assert_eq!(t.generic_params.len(), 2);
            assert_eq!(t.derives.len(), 3);
            assert_eq!(t.derives[0].value, crate::ast::DeriveOp::Add);
            assert_eq!(t.derives[1].value, crate::ast::DeriveOp::Sub);
            assert_eq!(t.derives[2].value, crate::ast::DeriveOp::Neg);
            assert_eq!(t.variants.len(), 1);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_type_decl_no_derive() {
    let source = "type Eci {}";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert!(t.derives.is_empty());
        }
        _ => panic!("expected type declaration"),
    }
}

// --- TypeApplication and generic struct construction tests ---

/// Helper to extract the dimension name from a single-term `DimExpr` type expression.
fn dim_expr_name(te: &crate::ast::TypeExpr) -> &str {
    match &te.kind {
        TypeExprKind::DimExpr(dim) => {
            assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
            dim.terms[0].term.name.name.as_str()
        }
        other => panic!("expected DimExpr, got {other:?}"),
    }
}

#[test]
fn parse_type_application_in_annotation() {
    let source = "param v: Vec3<Length, ECI> = 1.0;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.type_ann.kind {
            TypeExprKind::TypeApplication { name, type_args } => {
                assert_eq!(name.name.as_str(), "Vec3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(dim_expr_name(&type_args[0]), "Length");
                assert_eq!(dim_expr_name(&type_args[1]), "ECI");
            }
            other => panic!("expected TypeApplication, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_type_application_single_arg() {
    let source = "param t: Timestamp<UTC> = 0.0;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.type_ann.kind {
            TypeExprKind::TypeApplication { name, type_args } => {
                assert_eq!(name.name.as_str(), "Timestamp");
                assert_eq!(type_args.len(), 1);
                assert_eq!(dim_expr_name(&type_args[0]), "UTC");
            }
            other => panic!("expected TypeApplication, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_non_generic_type_still_works() {
    let source = "param v: Length = 1.0;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => {
            assert!(matches!(&p.type_ann.kind, TypeExprKind::DimExpr(_)));
        }
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_generic_struct_construction() {
    let source = "node v: Vec3<Length, ECI> = Vec3<Length, ECI> { x: 1.0, y: 2.0, z: 3.0 };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            } => {
                assert_eq!(type_name.value.as_str(), "Vec3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(dim_expr_name(&type_args[0]), "Length");
                assert_eq!(dim_expr_name(&type_args[1]), "ECI");
                assert_eq!(fields.len(), 3);
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_non_generic_struct_construction_still_works() {
    let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0 };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::StructConstruction {
                type_name,
                type_args,
                fields,
            } => {
                assert_eq!(type_name.value.as_str(), "TransferResult");
                assert!(type_args.is_empty());
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn is_pascal_case_examples() {
    assert!(is_pascal_case("TransferResult"));
    assert!(is_pascal_case("Orbit"));
    assert!(is_pascal_case("Ab"));
    assert!(!is_pascal_case("ORBIT"));
    assert!(!is_pascal_case("UPPER_SNAKE"));
    assert!(!is_pascal_case("orbit"));
    assert!(!is_pascal_case("lower_snake"));
    assert!(!is_pascal_case(""));
}

// --- Phase 2 block / let / LocalRef tests ---

#[test]
fn parse_block_simple() {
    let source = "node x: Dimensionless = { let a = 1.0; a + 2.0 };";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { stmts, expr } => {
                assert_eq!(stmts.len(), 1);
                assert_eq!(stmts[0].name.name, "a");
                assert!(stmts[0].type_ann.is_none());
                assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
            }
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_block_multiple_lets() {
    let source = "node x: Dimensionless = { let r1 = @a + @b; let r2 = @c; r1 + r2 };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { stmts, expr } => {
                assert_eq!(stmts.len(), 2);
                assert_eq!(stmts[0].name.name, "r1");
                assert_eq!(stmts[1].name.name, "r2");
                assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
            }
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_block_let_with_type_ann() {
    let source = "node x: Dimensionless = { let a: Dimensionless = 1.0; a };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { stmts, .. } => {
                assert_eq!(stmts.len(), 1);
                assert!(stmts[0].type_ann.is_some());
            }
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_block_no_lets() {
    let source = "node x: Dimensionless = { 1.0 + 2.0 };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { stmts, .. } => {
                assert_eq!(stmts.len(), 0);
            }
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_local_ref() {
    let source = "node x: Dimensionless = { let a = 1.0; a };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { expr, .. } => {
                assert!(matches!(&expr.kind, ExprKind::LocalRef(ident) if ident.name == "a"));
            }
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

// --- Phase 2 struct construction and field access tests ---

#[test]
fn parse_struct_construction_explicit_fields() {
    let source = "node t: Dimensionless = TransferResult { dv1: @a + @b, dv2: @c };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::StructConstruction {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.value.as_str(), "TransferResult");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name.value.as_str(), "dv1");
                assert!(fields[0].value.is_some());
                assert_eq!(fields[1].name.value.as_str(), "dv2");
                assert!(fields[1].value.is_some());
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_struct_construction_shorthand() {
    let source =
        "node t: Dimensionless = { let dv1 = @a; let dv2 = @b; TransferResult { dv1, dv2 } };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Block { expr, .. } => match &expr.kind {
                ExprKind::StructConstruction {
                    type_name, fields, ..
                } => {
                    assert_eq!(type_name.value.as_str(), "TransferResult");
                    assert_eq!(fields.len(), 2);
                    assert!(fields[0].value.is_none());
                    assert!(fields[1].value.is_none());
                }
                other => panic!("expected StructConstruction, got {other:?}"),
            },
            other => panic!("expected Block, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_struct_construction_trailing_comma() {
    let source = "node t: Dimensionless = TransferResult { dv1: 1.0, dv2: 2.0, };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::StructConstruction { fields, .. } => {
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected StructConstruction, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_field_access() {
    let source = "node x: Dimensionless = @transfer.dv1;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::FieldAccess { expr, field } => {
                assert!(
                    matches!(&expr.kind, ExprKind::GraphRef(ident) if ident.value.as_str() == "transfer")
                );
                assert_eq!(field.value.as_str(), "dv1");
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_chained_field_access() {
    let source = "node x: Dimensionless = @mission.transfer.dv1;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::FieldAccess { expr, field } => {
                assert_eq!(field.value.as_str(), "dv1");
                match &expr.kind {
                    ExprKind::FieldAccess {
                        expr: inner,
                        field: mid_field,
                    } => {
                        assert_eq!(mid_field.value.as_str(), "transfer");
                        assert!(
                            matches!(&inner.kind, ExprKind::GraphRef(ident) if ident.value.as_str() == "mission")
                        );
                    }
                    other => panic!("expected inner FieldAccess, got {other:?}"),
                }
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_field_access_in_arithmetic() {
    let source = "node x: Dimensionless = @t.dv1 + @t.dv2;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::BinOp { op, lhs, rhs } => {
                assert!(matches!(op, BinOp::Add));
                assert!(matches!(&lhs.kind, ExprKind::FieldAccess { .. }));
                assert!(matches!(&rhs.kind, ExprKind::FieldAccess { .. }));
            }
            other => panic!("expected BinOp, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

// Phase 3: fn declaration tests

#[test]
fn parse_fn_short_form() {
    let source = "fn double(x: Dimensionless) -> Dimensionless = x * 2.0;";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.name.value.as_str(), "double");
            assert!(f.generic_params.is_empty());
            assert_eq!(f.params.len(), 1);
            assert_eq!(f.params[0].name.name, "x");
            assert!(matches!(f.body, FnBody::Short(_)));
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_block_form() {
    let source = "fn add_one(x: Dimensionless) -> Dimensionless { let one = 1.0; x + one }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.name.value.as_str(), "add_one");
            match &f.body {
                FnBody::Block { stmts, expr } => {
                    assert_eq!(stmts.len(), 1);
                    assert_eq!(stmts[0].name.name, "one");
                    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
                }
                FnBody::Short(_) => panic!("expected block body"),
            }
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_with_generics() {
    let source = "fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D = a + (b - a) * t;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.name.value.as_str(), "lerp");
            assert_eq!(f.generic_params.len(), 1);
            assert_eq!(f.generic_params[0].name.value.as_str(), "D");
            assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
            assert_eq!(f.params.len(), 3);
            assert_eq!(f.params[0].name.name, "a");
            assert_eq!(f.params[1].name.name, "b");
            assert_eq!(f.params[2].name.name, "t");
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_multiple_generics() {
    let source = "fn convert<A: Dim, B: Dim>(x: A, y: B) -> A = x;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.generic_params.len(), 2);
            assert_eq!(f.generic_params[0].name.value.as_str(), "A");
            assert_eq!(f.generic_params[1].name.value.as_str(), "B");
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_zero_args() {
    let source = "fn pi_val() -> Dimensionless = PI;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.name.value.as_str(), "pi_val");
            assert!(f.params.is_empty());
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_trailing_comma() {
    let source = "fn add(x: Dimensionless, y: Dimensionless,) -> Dimensionless = x + y;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.params.len(), 2);
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_dim_expr_type() {
    let source = "fn speed(d: Length, t: Time) -> Length / Time = d / t;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.params.len(), 2);
            // Return type is a compound dim expr
            assert!(matches!(f.return_type.kind, TypeExprKind::DimExpr(_)));
        }
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_block_no_lets() {
    let source = "fn identity(x: Dimensionless) -> Dimensionless { x }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => match &f.body {
            FnBody::Block { stmts, .. } => assert!(stmts.is_empty()),
            FnBody::Short(_) => panic!("expected block body"),
        },
        other => panic!("expected Fn, got {other:?}"),
    }
}

#[test]
fn parse_fn_mixed_with_other_decls() {
    let source = r"
        const TWO: Dimensionless = 2.0;
        fn double(x: Dimensionless) -> Dimensionless = x * TWO;
        param val: Dimensionless = 5.0;
        node result: Dimensionless = double(@val);
    ";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 4);
    assert!(matches!(file.declarations[0].kind, DeclKind::Const(_)));
    assert!(matches!(file.declarations[1].kind, DeclKind::Fn(_)));
    assert!(matches!(file.declarations[2].kind, DeclKind::Param(_)));
    assert!(matches!(file.declarations[3].kind, DeclKind::Node(_)));
}

// --- Phase 5: Indexed Values ---

#[test]
fn parse_index_decl() {
    let source = "index Maneuver = { Departure, Correction, Insertion }";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Maneuver");
            match &idx.kind {
                IndexDeclKind::Named { variants } => {
                    assert_eq!(variants.len(), 3);
                    assert_eq!(variants[0].value.as_str(), "Departure");
                    assert_eq!(variants[1].value.as_str(), "Correction");
                    assert_eq!(variants[2].value.as_str(), "Insertion");
                }
                IndexDeclKind::Range { .. } => panic!("expected named index"),
            }
        }
        _ => panic!("expected index declaration"),
    }
}

#[test]
fn parse_index_decl_trailing_comma() {
    let source = "index Phase = { Boost, Coast, }";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Phase");
            match &idx.kind {
                IndexDeclKind::Named { variants } => {
                    assert_eq!(variants.len(), 2);
                }
                IndexDeclKind::Range { .. } => panic!("expected named index"),
            }
        }
        _ => panic!("expected index declaration"),
    }
}

#[test]
fn parse_range_index_decl() {
    let source = "index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "TimeStep");
            assert!(matches!(idx.kind, IndexDeclKind::Range { .. }));
        }
        _ => panic!("expected index declaration"),
    }
}

#[test]
fn parse_indexed_type() {
    let source = "param dv: Velocity[Maneuver] = 1.0 m/s;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => {
            assert_eq!(p.name.value.as_str(), "dv");
            match &p.type_ann.kind {
                TypeExprKind::Indexed { base, indexes } => {
                    assert!(matches!(base.kind, TypeExprKind::DimExpr(_)));
                    assert_eq!(indexes.len(), 1);
                    assert_eq!(indexes[0].name, "Maneuver");
                }
                other => panic!("expected Indexed type, got {other:?}"),
            }
        }
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_multi_indexed_type() {
    let source = "param matrix: Dimensionless[Row, Col] = 0.0;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.type_ann.kind {
            TypeExprKind::Indexed { indexes, .. } => {
                assert_eq!(indexes.len(), 2);
                assert_eq!(indexes[0].name, "Row");
                assert_eq!(indexes[1].name, "Col");
            }
            other => panic!("expected Indexed type, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_for_comprehension() {
    let source = "node fuel: Mass[Maneuver] = for m: Maneuver { 1.0 kg };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::ForComp { bindings, body } => {
                assert_eq!(bindings.len(), 1);
                assert_eq!(bindings[0].var.name, "m");
                assert_eq!(bindings[0].index.value.as_str(), "Maneuver");
                assert!(matches!(body.kind, ExprKind::UnitLiteral { .. }));
            }
            other => panic!("expected ForComp, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_for_multi_binding() {
    let source = "node x: Dimensionless[Row, Col] = for r: Row, c: Col { 0.0 };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::ForComp { bindings, .. } => {
                assert_eq!(bindings.len(), 2);
                assert_eq!(bindings[0].var.name, "r");
                assert_eq!(bindings[0].index.value.as_str(), "Row");
                assert_eq!(bindings[1].var.name, "c");
                assert_eq!(bindings[1].index.value.as_str(), "Col");
            }
            other => panic!("expected ForComp, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_index_access_with_variant() {
    let source = "node x: Velocity = @dv[Maneuver::Departure];";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::IndexAccess { expr, args } => {
                assert!(matches!(expr.kind, ExprKind::GraphRef(_)));
                assert_eq!(args.len(), 1);
                match &args[0] {
                    crate::ast::IndexArg::Variant { index, variant } => {
                        assert_eq!(index.value.as_str(), "Maneuver");
                        assert_eq!(variant.value.as_str(), "Departure");
                    }
                    other @ crate::ast::IndexArg::Var(_) => {
                        panic!("expected Variant, got {other:?}")
                    }
                }
            }
            other => panic!("expected IndexAccess, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_index_access_with_loop_var() {
    let source = "node y: Velocity[Maneuver] = for m: Maneuver { @dv[m] };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::ForComp { body, .. } => match &body.kind {
                ExprKind::IndexAccess { args, .. } => {
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        crate::ast::IndexArg::Var(ident) => assert_eq!(ident.name, "m"),
                        other @ crate::ast::IndexArg::Variant { .. } => {
                            panic!("expected Var, got {other:?}")
                        }
                    }
                }
                other => panic!("expected IndexAccess, got {other:?}"),
            },
            other => panic!("expected ForComp, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_map_literal() {
    let source = "param dv: Velocity[Maneuver] = { Maneuver::Departure: 2.0 km/s, Maneuver::Correction: 0.05 km/s };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.value.kind {
            ExprKind::MapLiteral { entries } => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].keys[0].index.value.as_str(), "Maneuver");
                assert_eq!(entries[0].keys[0].variant.value.as_str(), "Departure");
                assert_eq!(entries[1].keys[0].index.value.as_str(), "Maneuver");
                assert_eq!(entries[1].keys[0].variant.value.as_str(), "Correction");
            }
            other => panic!("expected MapLiteral, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_table_1d() {
    let source = r"param v: Velocity[Maneuver] = table[Maneuver] {
        Departure: 2.46 km/s;
        Correction: 0.12 km/s;
        Insertion: 1.83 km/s;
    };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.value.kind {
            ExprKind::TableLiteral { indexes, entries } => {
                assert_eq!(indexes.len(), 1);
                assert_eq!(indexes[0].value.as_str(), "Maneuver");
                assert_eq!(entries.len(), 3);
                assert_eq!(entries[0].keys.len(), 1);
                assert_eq!(entries[0].keys[0].index.value.as_str(), "Maneuver");
                assert_eq!(entries[0].keys[0].variant.value.as_str(), "Departure");
                assert_eq!(entries[1].keys[0].variant.value.as_str(), "Correction");
                assert_eq!(entries[2].keys[0].variant.value.as_str(), "Insertion");
            }
            other => panic!("expected TableLiteral, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_table_2d() {
    let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
        Departure, Correction, Insertion;
        Launch:  5000.0 kg, 0.0 kg, 0.0 kg;
        Cruise:  0.0 kg, 4500.0 kg, 0.0 kg;
        Arrival: 0.0 kg, 0.0 kg, 4000.0 kg;
    };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.value.kind {
            ExprKind::TableLiteral { indexes, entries } => {
                assert_eq!(indexes.len(), 2);
                assert_eq!(indexes[0].value.as_str(), "Phase");
                assert_eq!(indexes[1].value.as_str(), "Maneuver");
                assert_eq!(entries.len(), 9);
                assert_eq!(entries[0].keys.len(), 2);
                assert_eq!(entries[0].keys[0].index.value.as_str(), "Phase");
                assert_eq!(entries[0].keys[0].variant.value.as_str(), "Launch");
                assert_eq!(entries[0].keys[1].index.value.as_str(), "Maneuver");
                assert_eq!(entries[0].keys[1].variant.value.as_str(), "Departure");
                assert_eq!(entries[1].keys[1].variant.value.as_str(), "Correction");
                assert_eq!(entries[8].keys[0].variant.value.as_str(), "Arrival");
                assert_eq!(entries[8].keys[1].variant.value.as_str(), "Insertion");
            }
            other => panic!("expected TableLiteral, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_table_3d() {
    let source = r"param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
        [Time::T1]
        Departure, Correction;
        Launch: 5000.0 kg, 0.0 kg;
        Cruise: 0.0 kg, 4500.0 kg;

        [Time::T2]
        Departure, Correction;
        Launch: 4800.0 kg, 0.0 kg;
        Cruise: 0.0 kg, 4300.0 kg;
    };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Param(p) => match &p.value.kind {
            ExprKind::TableLiteral { indexes, entries } => {
                assert_eq!(indexes.len(), 3);
                assert_eq!(indexes[0].value.as_str(), "Time");
                assert_eq!(indexes[1].value.as_str(), "Phase");
                assert_eq!(indexes[2].value.as_str(), "Maneuver");
                assert_eq!(entries.len(), 8);
                assert_eq!(entries[0].keys.len(), 3);
                assert_eq!(entries[0].keys[0].index.value.as_str(), "Time");
                assert_eq!(entries[0].keys[0].variant.value.as_str(), "T1");
                assert_eq!(entries[0].keys[1].index.value.as_str(), "Phase");
                assert_eq!(entries[0].keys[1].variant.value.as_str(), "Launch");
                assert_eq!(entries[0].keys[2].index.value.as_str(), "Maneuver");
                assert_eq!(entries[0].keys[2].variant.value.as_str(), "Departure");
                assert_eq!(entries[4].keys[0].variant.value.as_str(), "T2");
                assert_eq!(entries[4].keys[1].variant.value.as_str(), "Launch");
                assert_eq!(entries[4].keys[2].variant.value.as_str(), "Departure");
            }
            other => panic!("expected TableLiteral, got {other:?}"),
        },
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_table_row_length_mismatch() {
    let source = r"param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
        Departure, Correction, Insertion;
        Launch: 5000.0 kg, 0.0 kg;
    };";
    let err = Parser::new(source).parse_file().unwrap_err();
    assert!(matches!(
        err,
        ParseError::TableRowLengthMismatch {
            expected: 3,
            got: 2,
            ..
        }
    ));
}

#[test]
fn parse_scan_expression() {
    let source = "node cum: Velocity[Maneuver] = scan(@dv, 0.0 m/s, |acc, val| acc + val);";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Scan {
                acc_name, val_name, ..
            } => {
                assert_eq!(acc_name.name, "acc");
                assert_eq!(val_name.name, "val");
            }
            other => panic!("expected Scan, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_unfold_expression() {
    let source = "node x: Dimensionless[TimeStep] = unfold(1.0, |prev_t, t| { @x[prev_t] * 2.0 });";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Node(n) => match &n.value.kind {
            ExprKind::Unfold {
                prev_name,
                curr_name,
                ..
            } => {
                assert_eq!(prev_name.name, "prev_t");
                assert_eq!(curr_name.name, "t");
            }
            other => panic!("expected Unfold, got {other:?}"),
        },
        _ => panic!("expected node"),
    }
}

#[test]
fn parse_generic_fn_with_index_constraint() {
    let source = "fn total<D: Dim, I: Index>(values: D) -> D = values;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.generic_params.len(), 2);
            assert_eq!(f.generic_params[0].name.value.as_str(), "D");
            assert_eq!(f.generic_params[0].constraint, GenericConstraint::Dim);
            assert_eq!(f.generic_params[1].name.value.as_str(), "I");
            assert_eq!(f.generic_params[1].constraint, GenericConstraint::Index);
        }
        _ => panic!("expected fn"),
    }
}

// --- parse_single_expr tests ---

#[test]
fn single_expr_unit_literal() {
    let expr = Parser::new("450.0 s").parse_single_expr().unwrap();
    assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
}

#[test]
fn single_expr_integer_with_unit_errors() {
    let result = Parser::new("450 s").parse_single_expr();
    assert!(
        result.is_err(),
        "integer literal with unit should be an error"
    );
}

#[test]
fn single_expr_number() {
    let expr = Parser::new("3.0").parse_single_expr().unwrap();
    assert!(matches!(expr.kind, ExprKind::Number(n) if (n - 3.0).abs() < f64::EPSILON));
}

#[test]
fn single_expr_compound_unit() {
    let expr = Parser::new("9.80665 m/s^2").parse_single_expr().unwrap();
    assert!(matches!(expr.kind, ExprKind::UnitLiteral { .. }));
}

#[test]
fn single_expr_arithmetic_with_const() {
    let expr = Parser::new("2.0 * PI").parse_single_expr().unwrap();
    assert!(matches!(expr.kind, ExprKind::BinOp { .. }));
}

#[test]
fn single_expr_trailing_tokens_error() {
    let result = Parser::new("450.0 s; extra").parse_single_expr();
    assert!(result.is_err());
}

#[test]
fn parse_use_no_alias() {
    let file = Parser::new(r#"use "./helper.gcl" { x, Y };"#)
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    let DeclKind::Use(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path, "./helper.gcl");
    let crate::ast::UseKind::Selective(names) = &u.kind else {
        panic!("expected Selective");
    };
    assert_eq!(names.len(), 2);
    assert_eq!(names[0].name.name, "x");
    assert!(names[0].alias.is_none());
    assert_eq!(names[0].local_name(), "x");
    assert_eq!(names[1].name.name, "Y");
    assert!(names[1].alias.is_none());
    assert_eq!(names[1].local_name(), "Y");
}

#[test]
fn parse_use_with_alias() {
    let file = Parser::new(r#"use "./helper.gcl" { x as y };"#)
        .parse_file()
        .unwrap();
    let DeclKind::Use(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    let crate::ast::UseKind::Selective(names) = &u.kind else {
        panic!("expected Selective");
    };
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].name.name, "x");
    assert_eq!(names[0].alias.as_ref().unwrap().name, "y");
    assert_eq!(names[0].local_name(), "y");
}

#[test]
fn parse_use_mixed_alias() {
    let file = Parser::new(r#"use "./f.gcl" { x, Y as Z, w };"#)
        .parse_file()
        .unwrap();
    let DeclKind::Use(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    let crate::ast::UseKind::Selective(names) = &u.kind else {
        panic!("expected Selective");
    };
    assert_eq!(names.len(), 3);
    assert_eq!(names[0].name.name, "x");
    assert!(names[0].alias.is_none());
    assert_eq!(names[1].name.name, "Y");
    assert_eq!(names[1].alias.as_ref().unwrap().name, "Z");
    assert_eq!(names[1].local_name(), "Z");
    assert_eq!(names[2].name.name, "w");
    assert!(names[2].alias.is_none());
}

#[test]
fn parse_use_alias_missing_name_error() {
    let result = Parser::new(r#"use "./f.gcl" { x as };"#).parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_use_module_bare() {
    let file = Parser::new(r#"use "./constants.gcl";"#)
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    let DeclKind::Use(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path, "./constants.gcl");
    let crate::ast::UseKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert!(alias.is_none());
}

#[test]
fn parse_use_module_with_alias() {
    let file = Parser::new(r#"use "./constants.gcl" as consts;"#)
        .parse_file()
        .unwrap();
    let DeclKind::Use(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path, "./constants.gcl");
    let crate::ast::UseKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert_eq!(alias.as_ref().unwrap().name, "consts");
}

#[test]
fn parse_use_module_missing_alias_ident_error() {
    let result = Parser::new(r#"use "./f.gcl" as;"#).parse_file();
    assert!(result.is_err());
}

// --- Qualified reference tests ---

#[test]
fn parse_qualified_graph_ref() {
    let file = Parser::new("node x: Dimensionless = @params::dry_mass;")
        .parse_file()
        .unwrap();
    let decl = &file.declarations[0].kind;
    let DeclKind::Node(node) = decl else {
        panic!("expected Node");
    };
    match &node.value.kind {
        ExprKind::QualifiedGraphRef { module, name } => {
            assert_eq!(module.name, "params");
            assert_eq!(name.value.as_str(), "dry_mass");
        }
        other => panic!("expected QualifiedGraphRef, got {other:?}"),
    }
}

#[test]
fn parse_qualified_const_ref() {
    let file = Parser::new("node x: Dimensionless = constants::G0;")
        .parse_file()
        .unwrap();
    let decl = &file.declarations[0].kind;
    let DeclKind::Node(node) = decl else {
        panic!("expected Node");
    };
    match &node.value.kind {
        ExprKind::QualifiedConstRef { module, name } => {
            assert_eq!(module.name, "constants");
            assert_eq!(name.value.as_str(), "G0");
        }
        other => panic!("expected QualifiedConstRef, got {other:?}"),
    }
}

#[test]
fn parse_qualified_fn_call() {
    let file = Parser::new("node x: Dimensionless = lib::compute(1.0, 2.0);")
        .parse_file()
        .unwrap();
    let decl = &file.declarations[0].kind;
    let DeclKind::Node(node) = decl else {
        panic!("expected Node");
    };
    match &node.value.kind {
        ExprKind::QualifiedFnCall { module, name, args } => {
            assert_eq!(module.name, "lib");
            assert_eq!(name.value.as_str(), "compute");
            assert_eq!(args.len(), 2);
        }
        other => panic!("expected QualifiedFnCall, got {other:?}"),
    }
}

#[test]
fn parse_qualified_fn_call_no_args() {
    let file = Parser::new("node x: Dimensionless = lib::get_value();")
        .parse_file()
        .unwrap();
    let decl = &file.declarations[0].kind;
    let DeclKind::Node(node) = decl else {
        panic!("expected Node");
    };
    match &node.value.kind {
        ExprKind::QualifiedFnCall { module, name, args } => {
            assert_eq!(module.name, "lib");
            assert_eq!(name.value.as_str(), "get_value");
            assert_eq!(args.len(), 0);
        }
        other => panic!("expected QualifiedFnCall, got {other:?}"),
    }
}

// --- Attribute tests ---

#[test]
fn parse_attribute_no_args() {
    let file = Parser::new("#[lazy]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    assert_eq!(file.declarations[0].attributes.len(), 1);
    assert_eq!(file.declarations[0].attributes[0].name.name, "lazy");
    assert!(file.declarations[0].attributes[0].args.is_empty());
}

#[test]
fn parse_attribute_with_one_arg() {
    let file = Parser::new("#[assumes(pressure_safe)]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations[0].attributes.len(), 1);
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.name.name, "assumes");
    assert_eq!(attr.args.len(), 1);
    assert_eq!(attr.args[0].name, "pressure_safe");
}

#[test]
fn parse_attribute_with_multiple_args() {
    let file = Parser::new("#[assumes(pressure_safe, temp_bounded)]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.name.name, "assumes");
    assert_eq!(attr.args.len(), 2);
    assert_eq!(attr.args[0].name, "pressure_safe");
    assert_eq!(attr.args[1].name, "temp_bounded");
}

#[test]
fn parse_attribute_trailing_comma() {
    let file = Parser::new("#[assumes(pressure_safe,)]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.args.len(), 1);
}

#[test]
fn parse_multiple_attributes() {
    let file = Parser::new("#[lazy]\n#[assumes(x)]\nnode y: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations[0].attributes.len(), 2);
    assert_eq!(file.declarations[0].attributes[0].name.name, "lazy");
    assert_eq!(file.declarations[0].attributes[1].name.name, "assumes");
}

#[test]
fn parse_attribute_on_param() {
    let file = Parser::new("#[assumes(x)]\nparam y: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations[0].attributes.len(), 1);
    assert!(matches!(file.declarations[0].kind, DeclKind::Param(_)));
}

#[test]
fn parse_no_attributes_still_works() {
    let file = Parser::new("param x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    assert!(file.declarations[0].attributes.is_empty());
}

#[test]
fn parse_attribute_span_covers_hash_to_bracket() {
    let file = Parser::new("#[lazy]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    // Declaration span should start at '#' (offset 0), not at 'node'
    assert_eq!(file.declarations[0].span.offset(), 0);
}

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]

use crate::syntax::ast::{
    AttributeArg, DeclKind, ExprKind, GenericConstraint, ImportKind, IndexDeclKind, MulDivOp,
    TypeExprKind,
};
use crate::syntax::parser::Parser;

fn dim_expr_name(te: &crate::syntax::ast::TypeExpr) -> &str {
    match &te.kind {
        TypeExprKind::DimExpr(dim) => {
            assert_eq!(dim.terms.len(), 1, "expected single-term DimExpr");
            dim.terms[0].term.name.name.as_str()
        }
        other => panic!("expected DimExpr, got {other:?}"),
    }
}

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
            assert!(
                matches!(p.value.as_ref().unwrap().kind, ExprKind::Number(n) if (n - 42.0).abs() < f64::EPSILON)
            );
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
            assert!(matches!(
                p.value.as_ref().unwrap().kind,
                ExprKind::UnitLiteral { .. }
            ));
        }
        _ => panic!("expected param"),
    }
}

#[test]
fn parse_param_required() {
    let file = Parser::new("param dry_mass: Mass;").parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Param(p) => {
            assert_eq!(p.name.value.as_str(), "dry_mass");
            match &p.type_ann.kind {
                TypeExprKind::DimExpr(d) => {
                    assert_eq!(d.terms.len(), 1);
                    assert_eq!(d.terms[0].term.name.name, "Mass");
                }
                other => panic!("expected DimExpr, got {other:?}"),
            }
            assert!(p.value.is_none());
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
fn parse_const_node_with_type() {
    let file = Parser::new("const node G0: Dimensionless = 9.80665;")
        .parse_file()
        .unwrap();
    match &file.declarations[0].kind {
        DeclKind::ConstNode(c) => {
            assert_eq!(c.name.value.as_str(), "G0");
            assert!(matches!(c.type_ann.kind, TypeExprKind::Dimensionless));
        }
        _ => panic!("expected const node"),
    }
}

#[test]
fn parse_base_dimension() {
    let file = Parser::new("base dimension Length;").parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::BaseDimension(d) => {
            assert_eq!(d.name.value.as_str(), "Length");
        }
        _ => panic!("expected base dimension"),
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
            assert_eq!(d.definition.terms.len(), 2);
            assert_eq!(d.definition.terms[0].term.name.name, "Length");
            assert_eq!(d.definition.terms[1].op, MulDivOp::Div);
            assert_eq!(d.definition.terms[1].term.name.name, "Time");
        }
        _ => panic!("expected dimension"),
    }
}

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
            assert!(
                matches!(&def.scale_expr.kind, ExprKind::Number(n) if (*n - 1000.0).abs() < f64::EPSILON)
            );
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
            assert!(
                matches!(&def.scale_expr.kind, ExprKind::Number(n) if (*n - 1.0).abs() < f64::EPSILON)
            );
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
            // The parser now stores the expression tree instead of evaluating it.
            match &def.scale_expr.kind {
                ExprKind::BinOp { op, lhs, rhs } => {
                    assert!(matches!(op, crate::syntax::ast::BinOp::Div));
                    assert!(matches!(&lhs.kind, ExprKind::ConstRef(c) if c.value.as_str() == "PI"));
                    assert!(matches!(&rhs.kind, ExprKind::Integer(180)));
                }
                other => panic!("expected BinOp, got {other:?}"),
            }
            assert_eq!(def.unit_expr.terms[0].name.value.as_str(), "rad");
        }
        _ => panic!("expected unit"),
    }
}

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
fn parse_error_bad_const_node_casing() {
    let result = Parser::new("const node bad_name: Dimensionless = 42.0;").parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_error_standalone_const() {
    let result = Parser::new("const G0: Dimensionless = 9.80665;").parse_file();
    assert!(
        result.is_err(),
        "standalone `const` should be a parse error"
    );
}

#[test]
fn parse_orbital_milestone_syntax() {
    let source = r"
dimension Velocity = Length / Time;

param alt: Length = 400.0 km;
param period: Time = 90.0 min;
const node R_EARTH: Length = 6371.0 km;

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
            DeclKind::ConstNode(c) => c.name.value.as_str(),
            DeclKind::BaseDimension(d) => d.name.value.as_str(),
            DeclKind::Dimension(d) => d.name.value.as_str(),
            DeclKind::Unit(u) => u.name.value.as_str(),
            DeclKind::Type(t) => t.name.value.as_str(),
            DeclKind::Fn(f) => f.name.value.as_str(),
            DeclKind::Index(i) => i.name.value.as_str(),
            DeclKind::Import(_) => "<import>",
            DeclKind::Include(_) => "<include>",
            DeclKind::Dag(d) => d.name.value.as_str(),
            DeclKind::Assert(a) => a.name.value.as_str(),
            DeclKind::Plot(p) => p.name.value.as_str(),
            DeclKind::Figure(f) => f.name.value.as_str(),
            DeclKind::Layer(l) => l.name.value.as_str(),
            DeclKind::UnionType(u) => u.name.value.as_str(),
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

#[test]
fn parse_type_decl_single_field() {
    let source = "type Orbit { sma: Length }";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Orbit");
            assert_eq!(t.fields.len(), 1);
            assert_eq!(t.fields[0].name.value.as_str(), "sma");
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
            assert_eq!(t.fields.len(), 2);
            assert_eq!(t.fields[0].name.value.as_str(), "dv1");
            assert_eq!(t.fields[1].name.value.as_str(), "dv2");
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
            assert_eq!(t.fields.len(), 2);
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
            assert_eq!(t.fields.len(), 0);
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
            assert_eq!(t.fields.len(), 1);
            assert_eq!(t.fields[0].name.value.as_str(), "dv");
            match &t.fields[0].type_ann.kind {
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
            assert_eq!(t.fields.len(), 3);
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
            assert_eq!(t.fields.len(), 0);
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
            assert_eq!(t.fields.len(), 1);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_union_type_decl() {
    let source = "type ManeuverKind = Impulsive | Coasting;";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::UnionType(u) => {
            assert_eq!(u.name.value.as_str(), "ManeuverKind");
            assert_eq!(u.members.len(), 2);
        }
        _ => panic!("expected union type declaration"),
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
fn parse_type_decl_derive_attribute() {
    let source = "#[derive(Add, Sub, Neg)]\ntype Vec3<D: Dim, F: Type> { x: D, y: D, z: D }";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    // #[derive(...)] stays in attributes (validated by resolver, extracted at IR lowering)
    let attrs = &file.declarations[0].attributes;
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].name.name, "derive");
    assert_eq!(attrs[0].args.len(), 3);
    assert_eq!(attrs[0].args[0].as_single_ident().unwrap().name, "Add");
    assert_eq!(attrs[0].args[1].as_single_ident().unwrap().name, "Sub");
    assert_eq!(attrs[0].args[2].as_single_ident().unwrap().name, "Neg");
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert_eq!(t.name.value.as_str(), "Vec3");
            assert_eq!(t.generic_params.len(), 2);
            assert_eq!(t.fields.len(), 3);
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_derive_attribute_on_non_type_passes_parser() {
    // Parser no longer rejects #[derive] on non-type decls; validation is done by the resolver
    let source = "#[derive(Add)]\nparam x: Dimensionless = 1.0;";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations[0].attributes.len(), 1);
    assert_eq!(file.declarations[0].attributes[0].name.name, "derive");
}

#[test]
fn parse_type_decl_no_derive() {
    let source = "type Eci {}";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Type(t) => {
            assert!(t.fields.is_empty());
        }
        _ => panic!("expected type declaration"),
    }
}

#[test]
fn parse_index_named_decl() {
    let source = "index Maneuver = { Departure, Correction, Insertion };";
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
                other => panic!("expected named index, got {other:?}"),
            }
        }
        _ => panic!("expected index declaration"),
    }
}

#[test]
fn parse_index_named_trailing_comma() {
    let source = "index Phase = { Boost, Coast, };";
    let file = Parser::new(source).parse_file().unwrap();
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Phase");
            match &idx.kind {
                IndexDeclKind::Named { variants } => {
                    assert_eq!(variants.len(), 2);
                }
                other => panic!("expected named index, got {other:?}"),
            }
        }
        _ => panic!("expected index declaration"),
    }
}

#[test]
fn parse_index_linspace_decl() {
    let source = "index TimeStep = linspace(0.0 s, 100.0 s, step: 0.1 s);";
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
fn parse_import_no_alias() {
    let file = Parser::new(r#"import "./helper.gcl" { x, Y };"#)
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path.display_path(), "./helper.gcl");
    assert!(matches!(
        &u.path,
        crate::syntax::ast::ImportPath::FilePath { .. }
    ));
    let crate::syntax::ast::ImportKind::Selective(names) = &u.kind else {
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
fn parse_import_with_alias() {
    let file = Parser::new(r#"import "./helper.gcl" { x as y };"#)
        .parse_file()
        .unwrap();
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    let crate::syntax::ast::ImportKind::Selective(names) = &u.kind else {
        panic!("expected Selective");
    };
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].name.name, "x");
    assert_eq!(names[0].alias.as_ref().unwrap().name, "y");
    assert_eq!(names[0].local_name(), "y");
}

#[test]
fn parse_import_mixed_alias() {
    let file = Parser::new(r#"import "./f.gcl" { x, Y as Z, w };"#)
        .parse_file()
        .unwrap();
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    let crate::syntax::ast::ImportKind::Selective(names) = &u.kind else {
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
fn parse_import_alias_missing_name_error() {
    let result = Parser::new(r#"import "./f.gcl" { x as };"#).parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_import_module_bare() {
    let file = Parser::new(r#"import "./constants.gcl";"#)
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path.display_path(), "./constants.gcl");
    let crate::syntax::ast::ImportKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert!(alias.is_none());
}

#[test]
fn parse_import_module_with_alias() {
    let file = Parser::new(r#"import "./constants.gcl" as consts;"#)
        .parse_file()
        .unwrap();
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Use");
    };
    assert_eq!(u.path.display_path(), "./constants.gcl");
    let crate::syntax::ast::ImportKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert_eq!(alias.as_ref().unwrap().name, "consts");
}

#[test]
fn parse_import_module_missing_alias_ident_error() {
    let result = Parser::new(r#"import "./f.gcl" as;"#).parse_file();
    assert!(result.is_err());
}

// ---- Bare module path tests ----

#[test]
fn parse_import_bare_path_selective() {
    let file = Parser::new("import nasa/rocket { delta_v };")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations.len(), 1);
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Import");
    };
    let crate::syntax::ast::ImportPath::ModulePath { segments, .. } = &u.path else {
        panic!("expected ModulePath");
    };
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].name, "nasa");
    assert_eq!(segments[1].name, "rocket");
    assert_eq!(u.path.display_path(), "nasa/rocket");
    let crate::syntax::ast::ImportKind::Selective(names) = &u.kind else {
        panic!("expected Selective");
    };
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].name.name, "delta_v");
}

#[test]
fn parse_import_bare_path_nested() {
    let file = Parser::new("import a/b/c/d;").parse_file().unwrap();
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Import");
    };
    let crate::syntax::ast::ImportPath::ModulePath { segments, .. } = &u.path else {
        panic!("expected ModulePath");
    };
    assert_eq!(segments.len(), 4);
    assert_eq!(u.path.display_path(), "a/b/c/d");
}

#[test]
fn parse_import_bare_path_with_alias() {
    let file = Parser::new("import nasa/rocket as r;")
        .parse_file()
        .unwrap();
    let DeclKind::Import(u) = &file.declarations[0].kind else {
        panic!("expected Import");
    };
    assert!(matches!(
        &u.path,
        crate::syntax::ast::ImportPath::ModulePath { .. }
    ));
    assert_eq!(u.path.display_path(), "nasa/rocket");
    let crate::syntax::ast::ImportKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert_eq!(alias.as_ref().unwrap().name, "r");
}

#[test]
fn parse_include_bare_path_with_param_bindings() {
    let file = Parser::new("include nasa/rocket(dry_mass: 800.0 kg) as stage_1;")
        .parse_file()
        .unwrap();
    let DeclKind::Include(u) = &file.declarations[0].kind else {
        panic!("expected Include");
    };
    assert!(matches!(
        &u.path,
        crate::syntax::ast::ImportPath::ModulePath { .. }
    ));
    assert_eq!(u.path.display_path(), "nasa/rocket");
    assert_eq!(u.param_bindings.len(), 1);
    assert_eq!(u.param_bindings[0].name.name, "dry_mass");
    let crate::syntax::ast::ImportKind::Module { alias } = &u.kind else {
        panic!("expected Module");
    };
    assert_eq!(alias.as_ref().unwrap().name, "stage_1");
}

#[test]
fn parse_import_with_param_bindings_error() {
    // import with param bindings should fail — use include instead
    let result =
        Parser::new(r#"import "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };"#).parse_file();
    assert!(result.is_err());
}

#[test]
fn parse_import_bare_path_single_segment_error() {
    // Single-segment bare paths are ambiguous; require at least pkg/module
    let result = Parser::new("import foo;").parse_file();
    // This should parse as a module import with a bare identifier... actually
    // our parser requires at least one `/` for bare paths, so a single bare
    // identifier after `import` that isn't followed by `/` should error.
    assert!(result.is_err(), "single-segment bare import should fail");
}

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
    assert_eq!(
        attr.args[0].as_single_ident().unwrap().name,
        "pressure_safe"
    );
}

#[test]
fn parse_attribute_with_multiple_args() {
    let file = Parser::new("#[assumes(pressure_safe, temp_bounded)]\nnode x: Dimensionless = 1.0;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.name.name, "assumes");
    assert_eq!(attr.args.len(), 2);
    assert_eq!(
        attr.args[0].as_single_ident().unwrap().name,
        "pressure_safe"
    );
    assert_eq!(attr.args[1].as_single_ident().unwrap().name, "temp_bounded");
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
    assert_eq!(file.declarations[0].span.offset(), 0);
}

#[test]
fn parse_attribute_expected_fail_no_args() {
    let file = Parser::new("#[expected_fail]\nassert x = true;")
        .parse_file()
        .unwrap();
    assert_eq!(file.declarations[0].attributes.len(), 1);
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.name.name, "expected_fail");
    assert!(attr.args.is_empty());
}

#[test]
fn parse_attribute_qualified_path() {
    let file = Parser::new("#[expected_fail(Mode::Boost)]\nassert x = true;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.args.len(), 1);
    let AttributeArg::Path { segments, .. } = &attr.args[0] else {
        panic!("expected Path, got {:?}", attr.args[0]);
    };
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].name, "Mode");
    assert_eq!(segments[1].name, "Boost");
}

#[test]
fn parse_attribute_multiple_qualified_paths() {
    let file = Parser::new("#[expected_fail(Mode::Boost, Mode::Eco)]\nassert x = true;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.args.len(), 2);
    let AttributeArg::Path { segments: s0, .. } = &attr.args[0] else {
        panic!("expected Path, got {:?}", attr.args[0]);
    };
    assert_eq!(s0[0].name, "Mode");
    assert_eq!(s0[1].name, "Boost");
    let AttributeArg::Path { segments: s1, .. } = &attr.args[1] else {
        panic!("expected Path, got {:?}", attr.args[1]);
    };
    assert_eq!(s1[0].name, "Mode");
    assert_eq!(s1[1].name, "Eco");
}

#[test]
fn parse_attribute_group_arg() {
    let file = Parser::new("#[expected_fail((Mode::Boost, Phase::Launch))]\nassert x = true;")
        .parse_file()
        .unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.args.len(), 1);
    let AttributeArg::Group { elements, .. } = &attr.args[0] else {
        panic!("expected Group, got {:?}", attr.args[0]);
    };
    assert_eq!(elements.len(), 2);
    let AttributeArg::Path { segments: s0, .. } = &elements[0] else {
        panic!("expected Path, got {:?}", elements[0]);
    };
    assert_eq!(s0[0].name, "Mode");
    assert_eq!(s0[1].name, "Boost");
    let AttributeArg::Path { segments: s1, .. } = &elements[1] else {
        panic!("expected Path, got {:?}", elements[1]);
    };
    assert_eq!(s1[0].name, "Phase");
    assert_eq!(s1[1].name, "Launch");
}

#[test]
fn parse_attribute_multiple_groups() {
    let source = "#[expected_fail((Mode::Boost, Phase::Launch), (Mode::Eco, Phase::Cruise))]\nassert x = true;";
    let file = Parser::new(source).parse_file().unwrap();
    let attr = &file.declarations[0].attributes[0];
    assert_eq!(attr.args.len(), 2);
    assert!(matches!(&attr.args[0], AttributeArg::Group { elements, .. } if elements.len() == 2));
    assert!(matches!(&attr.args[1], AttributeArg::Group { elements, .. } if elements.len() == 2));
}

#[test]
fn parse_required_named_index() {
    let source = "index Foo;";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Foo");
            assert!(matches!(idx.kind, IndexDeclKind::RequiredNamed));
        }
        other => panic!("expected index declaration, got {other:?}"),
    }
}

#[test]
fn parse_required_range_simple() {
    let source = "index Foo: Time;";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Foo");
            match &idx.kind {
                IndexDeclKind::RequiredRange { dimension } => {
                    assert_eq!(dimension.terms.len(), 1);
                    assert_eq!(dimension.terms[0].term.name.name.as_str(), "Time");
                }
                other => panic!("expected required range, got {other:?}"),
            }
        }
        other => panic!("expected index declaration, got {other:?}"),
    }
}

#[test]
fn parse_include_item_with_expected_fail() {
    let source = r#"include "./lib.gcl"(Phase: MyPhase) {
    #[expected_fail(MyPhase::X)]
    my_assert,
};"#;
    let file = Parser::new(source).parse_file().unwrap();
    let DeclKind::Include(imp) = &file.declarations[0].kind else {
        panic!("expected include");
    };
    let ImportKind::Selective(items) = &imp.kind else {
        panic!("expected selective include");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name.name, "my_assert");
    assert_eq!(items[0].attributes.len(), 1);
    assert_eq!(items[0].attributes[0].name.name, "expected_fail");
    assert_eq!(items[0].attributes[0].args.len(), 1);
}

#[test]
fn parse_include_item_with_expected_fail_and_alias() {
    let source = r#"include "./lib.gcl"(Phase: MyPhase) {
    #[expected_fail]
    my_assert as local_assert,
};"#;
    let file = Parser::new(source).parse_file().unwrap();
    let DeclKind::Include(imp) = &file.declarations[0].kind else {
        panic!("expected include");
    };
    let ImportKind::Selective(items) = &imp.kind else {
        panic!("expected selective include");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name.name, "my_assert");
    assert_eq!(items[0].alias.as_ref().unwrap().name, "local_assert");
    assert_eq!(items[0].attributes.len(), 1);
    assert_eq!(items[0].attributes[0].name.name, "expected_fail");
}

#[test]
fn parse_import_item_no_attributes() {
    let source = r#"import "./lib.gcl" { x, y };"#;
    let file = Parser::new(source).parse_file().unwrap();
    let DeclKind::Import(imp) = &file.declarations[0].kind else {
        panic!("expected import");
    };
    let ImportKind::Selective(items) = &imp.kind else {
        panic!("expected selective import");
    };
    assert_eq!(items.len(), 2);
    assert!(items[0].attributes.is_empty());
    assert!(items[1].attributes.is_empty());
}

#[test]
fn parse_required_range_compound_dim() {
    let source = "index Foo: Mass * Length / Time^2;";
    let file = Parser::new(source).parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Index(idx) => {
            assert_eq!(idx.name.value.as_str(), "Foo");
            match &idx.kind {
                IndexDeclKind::RequiredRange { dimension } => {
                    assert_eq!(dimension.terms.len(), 3);
                    assert_eq!(dimension.terms[0].term.name.name.as_str(), "Mass");
                    assert_eq!(dimension.terms[1].term.name.name.as_str(), "Length");
                    assert_eq!(dimension.terms[2].term.name.name.as_str(), "Time");
                    assert_eq!(dimension.terms[2].term.power, Some(2));
                    assert_eq!(dimension.terms[2].op, MulDivOp::Div);
                }
                other => panic!("expected required range, got {other:?}"),
            }
        }
        other => panic!("expected index declaration, got {other:?}"),
    }
}

// --- dag declaration tests ---

#[test]
fn parse_dag_empty_body() {
    let file = Parser::new("dag my_pipeline {}").parse_file().unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Dag(d) => {
            assert_eq!(d.name.value.as_str(), "my_pipeline");
            assert!(d.body.is_empty());
        }
        other => panic!("expected dag, got {other:?}"),
    }
}

#[test]
fn parse_dag_with_declarations() {
    let file = Parser::new(
        "dag rocket {
            param thrust: Force;
            node accel: Acceleration = @thrust / 1000.0 kg;
        }",
    )
    .parse_file()
    .unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Dag(d) => {
            assert_eq!(d.name.value.as_str(), "rocket");
            assert_eq!(d.body.len(), 2);
            assert!(
                matches!(&d.body[0].kind, DeclKind::Param(p) if p.name.value.as_str() == "thrust")
            );
            assert!(
                matches!(&d.body[1].kind, DeclKind::Node(n) if n.name.value.as_str() == "accel")
            );
        }
        other => panic!("expected dag, got {other:?}"),
    }
}

#[test]
fn parse_dag_name_must_be_lower_snake_case() {
    let result = Parser::new("dag MyPipeline {}").parse_file();
    assert!(result.is_err(), "PascalCase dag name should be rejected");
}

#[test]
fn parse_dag_with_attributes() {
    let file = Parser::new(
        "#[hidden]
        dag my_dag {
            param x: Dimensionless;
        }",
    )
    .parse_file()
    .unwrap();
    assert_eq!(file.declarations.len(), 1);
    assert_eq!(file.declarations[0].attributes.len(), 1);
    assert_eq!(file.declarations[0].attributes[0].name.name, "hidden");
    assert!(matches!(&file.declarations[0].kind, DeclKind::Dag(_)));
}

#[test]
fn parse_nested_dag() {
    let file = Parser::new(
        "dag outer {
            dag inner {
                param x: Dimensionless;
            }
        }",
    )
    .parse_file()
    .unwrap();
    assert_eq!(file.declarations.len(), 1);
    match &file.declarations[0].kind {
        DeclKind::Dag(outer) => {
            assert_eq!(outer.name.value.as_str(), "outer");
            assert_eq!(outer.body.len(), 1);
            match &outer.body[0].kind {
                DeclKind::Dag(inner) => {
                    assert_eq!(inner.name.value.as_str(), "inner");
                    assert_eq!(inner.body.len(), 1);
                }
                other => panic!("expected inner dag, got {other:?}"),
            }
        }
        other => panic!("expected outer dag, got {other:?}"),
    }
}

//! Domain constraints on Int/Datetime, struct fields, and generic arguments.

use super::*;

#[test]
fn datetime_struct_field_bound_requires_the_field_scale() {
    let error = compile_and_eval(
        r#"
type EventSpec {
    EventSpec(at: Datetime<TT>(min: datetime("2024-01-01T00:00:00Z"))),
}
node event: EventSpec = EventSpec(at: epoch<TT>("2024-06-01T00:00:00"));
"#,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(
                    DomainError::DatetimeDomainBoundTypeMismatch { .. }
                ),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn domain_bounds_reject_runtime_graph_references() {
    let error = compile_to_tir(
        r#"
param start: Datetime = datetime("2024-01-01T00:00:00Z");
param event: Datetime(min: @start) = datetime("2024-06-01T00:00:00Z");
"#,
        "test.gcl",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Name(NameError::GraphRefInConst { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn struct_field_const_violation_is_compile_time() {
    let source = "
type Spec { Spec(mass: Mass(min: 100.0 kg, max: 2000.0 kg)) }
const node SAT: Spec = Spec(mass: 5000.0 kg);
";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind:
                    SemanticErrorKind::Domain(DomainError::DomainViolation {
                        name, violation, ..
                    }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainViolation, got {err:?}");
    };
    assert_eq!(name.to_string(), "Spec.mass");
    assert!(
        violation.to_string().contains("above maximum"),
        "violation = {violation}"
    );
}

#[test]
fn generic_field_dimension_mismatch_is_rejected_before_evaluation() {
    let error = compile_and_eval(
        r"
type Box<D: Dim> { Box(x: D(min: 0.5 m)) }
node bad: Box<Time> = Box<Time>(x: 1.0 s);
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn invalid_generic_static_fin_key_is_rejected_before_evaluation() {
    let error = compile_and_eval(
        r"
type T<N: Nat> { T(x: Int(min: to_int(key(Fin(N), 0)))) }
node bad: T<0> = T<0>(x: 0);
",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CompileError::Eval(RenderedSemanticError {
            error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Index(IndexError::InvalidFiniteIndexCardinality { .. }),
                ..
            }),
            ..
        })
    ));
}

#[test]
fn struct_field_min_exceeds_max_at_compile_time() {
    let source = "type Foo { Foo(x: Mass(min: 100.0 kg, max: 50.0 kg)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainMinExceedsMax { name, .. }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainMinExceedsMax, got {err:?}");
    };
    assert_eq!(name.to_string(), "Foo.x");
}

#[test]
fn struct_field_invalid_target_at_compile_time() {
    let source = "type Foo { Foo(x: Bool(min: 0.0)) }";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::InvalidDomainTarget { .. }),
                    ..
                }),
                ..
            })
        ),
        "expected InvalidDomainTarget, got {err:?}"
    );
}

#[test]
fn struct_field_dim_mismatch_at_compile_time() {
    let source = "type Foo { Foo(x: Length(min: 1.0 s)) }";
    let err = compile_and_eval(source).unwrap_err();
    let CompileError::Eval(RenderedSemanticError {
        error:
            SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { name, .. }),
                ..
            }),
        ..
    }) = err
    else {
        panic!("expected DomainDimensionMismatch, got {err:?}");
    };
    assert_eq!(name.to_string(), "Foo.x");
}

#[test]
fn generic_type_arg_constraint_rejected() {
    let source = "
pub type Eci { Eci }
pub type Vec3<D: Dim, F: Type> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m), Eci> = Vec3<Length, Eci>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::GenericTypeArgDomainConstraint),
                    ..
                }),
                ..
            })
        ),
        "expected GenericTypeArgDomainConstraint, got {err:?}"
    );
}

#[test]
fn included_dag_param_constraint_runtime_violation() {
    let source = "
dag bumper {
    param v: Velocity(max: 100.0 m/s);
    pub node out: Velocity = @v * 2.0;
}
param speed: Velocity = 1000.0 m/s;
include bumper(v: @speed)::{ out as doubled };
";
    let result = compile_and_eval(source).unwrap();
    let EvalEntry {
        outcome: v_result, ..
    } = result
        .entries
        .iter()
        .find(|EvalEntry { name: n, .. }| n.to_string() == "bumper::v")
        .expect("bumper::v not found");
    assert!(v_result.is_err(), "v should violate domain constraint");
}

#[test]
fn included_dag_param_constraint_dim_mismatch() {
    let source = "
dag bumper {
    param v: Velocity(min: 1.0 kg);
    pub node out: Velocity = @v;
}
include bumper(v: 5.0 m/s)::{ out };
";
    let err = compile_and_eval(source).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::Eval(RenderedSemanticError {
                error: SemanticError::Located(graphcal_compiler::diagnostic::Diagnostic {
                    kind: SemanticErrorKind::Domain(DomainError::DomainDimensionMismatch { .. }),
                    ..
                }),
                ..
            })
        ),
        "expected DomainDimensionMismatch, got {err:?}"
    );
}

#[test]
fn eval_inline_dag_call_const_node_in_body() {
    // `const node` inside a dag body should participate in the same
    // topological evaluation as runtime nodes.
    let source = "\
dag with_const {
    param v: Length;
    const node multiplier: Dimensionless = 3.0;
    pub node result: Length = @v * @multiplier;
}

param src: Length = 4.0 m;
node out: Length = @with_const(v: @src)::result;
";
    let result = compile_and_eval(source).unwrap();
    let out = find_value(&result, "out");
    assert!((out - 12.0).abs() < 1e-10, "expected 12.0, got {out}");
}

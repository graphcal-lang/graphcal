#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "test code"
)]
use super::*;
use crate::syntax::dimension::BaseDimId;
use crate::syntax::parser::Parser;

fn make_src(source: &str) -> NamedSource<Arc<String>> {
    NamedSource::new("test", Arc::new(source.to_string()))
}

fn check(source: &str) -> Result<HashMap<String, DeclaredType>, GraphcalError> {
    let file = Parser::new(source).parse_file().unwrap();
    let src = make_src(source);
    let ir = crate::ir::lower::lower(&file, &src)?;
    let tir = crate::tir::typed::type_resolve(ir, &src)?;
    check_dimensions_tir(&tir, &src)?;
    tir.build_declared_types(&src)
}

#[test]
fn check_dimensionless_const() {
    let types = check("const node g0: Dimensionless = 9.80665;").unwrap();
    assert_eq!(
        types["g0"],
        DeclaredType::Scalar(Dimension::dimensionless())
    );
}

#[test]
fn check_dimensionless_arithmetic() {
    let types = check("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 2.0;").unwrap();
    assert_eq!(types["y"], DeclaredType::Scalar(Dimension::dimensionless()));
}

#[test]
fn check_length_unit_literal() {
    let types = check("param alt: Length = 400.0 km;").unwrap();
    let length = Dimension::base(BaseDimId::Prelude("Length".to_string()));
    assert_eq!(types["alt"], DeclaredType::Scalar(length));
}

#[test]
fn check_velocity_from_division() {
    let source = "param dist: Length = 100.0 km;\nparam time: Time = 2.0 hour;\nnode speed: Velocity = @dist / @time;";
    let types = check(source).unwrap();
    let velocity = Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string()));
    assert_eq!(types["speed"], DeclaredType::Scalar(velocity));
}

#[test]
fn check_add_dimension_mismatch() {
    let source = "param x: Length = 1.0 m;\nparam y: Time = 1.0 s;\nnode z: Length = @x + @y;";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_annotation_mismatch() {
    let source = "param x: Length = 1.0 m;\nnode y: Time = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_conversion_same_dimension() {
    let source =
        "param speed: Velocity = 100.0 m / s;\nnode speed_kmh: Velocity = @speed -> km / hour;";
    let types = check(source).unwrap();
    let velocity = Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string()));
    assert_eq!(types["speed_kmh"], DeclaredType::Scalar(velocity));
}

#[test]
fn check_conversion_wrong_dimension() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = @x -> s;";
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::ConversionDimensionMismatch { .. }
    ));
}

#[test]
fn check_sqrt_dimension() {
    let source = "param area: Area = 100.0 m;\nnode side: Length = sqrt(@area);";
    // Note: area should be m^2, but we declared it with m (Length).
    // sqrt(Length) = Length^(1/2) which doesn't match Length.
    let err = check(source).unwrap_err();
    assert!(matches!(
        err,
        GraphcalError::DimensionMismatchInAnnotation { .. }
    ));
}

#[test]
fn check_builtin_sin_requires_angle() {
    let source = "param x: Length = 1.0 m;\nnode y: Dimensionless = sin(@x);";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_if_branches_same_dim() {
    let source =
        "param x: Dimensionless = 1.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };";
    check(source).unwrap();
}

#[test]
fn check_if_branches_different_dim() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = if true { @x } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::DimensionMismatch { .. }));
}

#[test]
fn check_multiplication_creates_new_dim() {
    let source = "param mass: Mass = 10.0 kg;\nparam accel: Acceleration = 9.8 m / s^2;\nnode force: Force = @mass * @accel;";
    check(source).unwrap();
}

#[test]
fn check_power_with_literal() {
    let source = "param r: Length = 5.0 m;\nnode area: Area = @r ^ 2.0;";
    // Area is Length^2, r^2 = Length^2
    // But we need PI * r^2 for circle area — just testing r^2 = Area
    check(source).unwrap();
}

#[test]
fn check_fn_unknown_function() {
    let source = "param x: Length = 1.0 m;\nnode y: Length = no_such_fn(@x);";
    let err = check(source).unwrap_err();
    assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
}

// --- Indexed type tests ---

#[test]
fn check_indexed_param_map_literal() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};";
    let types = check(source).unwrap();
    let velocity = Dimension::base(BaseDimId::Prelude("Length".to_string()))
        / Dimension::base(BaseDimId::Prelude("Time".to_string()));
    assert_eq!(
        types["dv"],
        DeclaredType::Indexed {
            element: Box::new(DeclaredType::Scalar(velocity)),
            index: IndexName::new("Maneuver"),
        }
    );
}

#[test]
fn check_for_comprehension() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node doubled: Velocity[Maneuver] = for m: Maneuver { @dv[m] + @dv[m] };";
    check(source).unwrap();
}

#[test]
fn check_for_comprehension_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node bad: Length[Maneuver] = for m: Maneuver { @dv[m] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_index_access_with_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
param first: Velocity = @dv[Maneuver::Departure];";
    check(source).unwrap();
}

#[test]
fn check_map_literal_missing_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::MissingVariants { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_map_literal_extra_variant() {
    let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExtraVariants { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_index_mismatch_in_for() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Phase] = for p: Phase { @dv[p] };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_sum_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node total_dv: Velocity = sum(@dv);";
    check(source).unwrap();
}

#[test]
fn check_count_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node n: Dimensionless = count(@dv);";
    check(source).unwrap();
}

#[test]
fn check_mean_aggregation() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node avg_dv: Velocity = mean(@dv);";
    check(source).unwrap();
}

#[test]
fn check_scan() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node cum_dv: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc + val);";
    check(source).unwrap();
}

#[test]
fn check_scan_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 m, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_unknown_index_in_type_annotation() {
    let source = "param x: Velocity[NoSuchIndex] = 1.0 m / s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownIndex { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_for_with_sum() {
    // sum over a for comprehension
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node total: Velocity = sum(for m: Maneuver { @dv[m] });";
    check(source).unwrap();
}

// --- Comparison dimension mismatch ---

#[test]
fn check_comparison_dimension_mismatch() {
    let source = "\
param x: Length = 1.0 m;
param t: Time = 1.0 s;
node bad: Dimensionless = if @x > @t { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Boolean operator dimension errors ---

#[test]
fn check_boolean_and_lhs_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = @x && true;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_boolean_or_rhs_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = true || @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Power / exponent edge cases ---

#[test]
fn check_power_half_exponent() {
    // x ^ 0.5 on dimensionless should work
    let source = "param x: Dimensionless = 4.0;\nnode y: Dimensionless = @x ^ 0.5;";
    check(source).unwrap();
}

#[test]
fn check_power_non_literal_exponent_dimensioned_base() {
    // dimensioned ^ non-literal → NonLiteralExponent
    let source = "\
param x: Length = 1.0 m;
param n: Dimensionless = 2.0;
node bad: Area = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_dimensionless_base_non_literal_exponent() {
    // dimensionless ^ dimensionless (non-literal) → ok
    let source = "\
param x: Dimensionless = 2.0;
param n: Dimensionless = 3.0;
node y: Dimensionless = @x ^ @n;";
    check(source).unwrap();
}

#[test]
fn check_power_bad_fractional_exponent() {
    // x ^ 0.3 → NonLiteralExponent (not 0.5 and not integer)
    let source = "param x: Length = 1.0 m;\nnode bad: Length = @x ^ 0.3;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_power_dimensioned_exponent() {
    // anything ^ dimensioned → NonLiteralExponent
    let source = "\
param x: Dimensionless = 2.0;
param n: Length = 1.0 m;
node bad: Dimensionless = @x ^ @n;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NonLiteralExponent { .. }),
        "got: {err:?}"
    );
}

// --- If condition must be dimensionless ---

#[test]
fn check_if_condition_dimensioned() {
    let source = "\
param x: Length = 1.0 m;
node bad: Dimensionless = if @x { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Unknown dimension in type annotation ---

#[test]
fn check_unknown_dimension_in_type() {
    let source = "param x: NoSuchDimension = 1.0;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownDimension { .. }),
        "got: {err:?}"
    );
}

// --- expect_scalar error: struct used where scalar expected ---

#[test]
fn check_struct_in_arithmetic() {
    let source = "\
pub type Orbit { altitude: Length, speed: Velocity }
param o: Orbit = Orbit { altitude: 400.0 km, speed: 7.6 km / s };
node bad: Length = @o + 1.0 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- FieldAccess on non-struct ---

#[test]
fn check_field_access_on_scalar() {
    let source = "\
param x: Length = 1.0 m;
node bad: Length = @x.foo;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::NotAStruct { .. }),
        "got: {err:?}"
    );
}

// --- Struct extra fields ---

#[test]
fn check_struct_extra_fields() {
    let source = "\
type Orbit { altitude: Length, speed: Velocity }
node o: Orbit = Orbit { altitude: 400.0 km, speed: 7.6 km / s, bonus: 1.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::ExtraFields { .. }),
        "got: {err:?}"
    );
}

// --- Block let-binding type annotation mismatch ---

// --- types_match wildcard: mismatched kinds ---

#[test]
fn check_types_match_struct_vs_scalar() {
    // Declared as a struct type but expression evaluates to scalar → mismatch
    let source = "\
type Orbit { altitude: Length, speed: Velocity }
param x: Dimensionless = 1.0;
node o: Orbit = @x;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatchInAnnotation { .. }),
        "got: {err:?}"
    );
}

// --- ForComp with unknown index ---

#[test]
fn check_for_comp_unknown_index() {
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = for m: NoSuchIndex { @x };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownIndex { .. }),
        "got: {err:?}"
    );
}

// --- Scan body type mismatch ---

#[test]
fn check_scan_body_type_mismatch() {
    let source = "\
pub index Maneuver = { Departure, Correction, Insertion };
param dv: Velocity[Maneuver] = {
Maneuver::Departure: 2.46 km / s,
Maneuver::Correction: 0.5 km / s,
Maneuver::Insertion: 1.8 km / s,
};
node bad: Velocity[Maneuver] = scan(@dv, 0.0 km / s, |acc, val| acc * val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Scan on non-indexed value ---

#[test]
fn check_scan_on_scalar() {
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = scan(@x, 0.0, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

// --- Map literal dimension inconsistency ---

#[test]
fn check_map_literal_inconsistent_element_dims() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless[Phase] = {
Phase::Coast: 1.0,
Phase::Burn: 2.0 m,
};";
    let err = check(source).unwrap_err();
    // The map entries have different dimensions: first is Dimensionless, second is Length
    assert!(
        matches!(
            err,
            GraphcalError::DimensionMismatchInAnnotation { .. }
                | GraphcalError::DimensionMismatch { .. }
        ),
        "got: {err:?}"
    );
}

// --- Index access with unknown variant ---

#[test]
fn check_index_access_unknown_variant() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless[Phase] = {
Phase::Coast: 1.0,
Phase::Burn: 2.0,
};
param bad: Dimensionless = @x[Phase::NoSuch];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownVariant { .. }),
        "got: {err:?}"
    );
}

// --- Indexing a non-indexed value ---

#[test]
fn check_index_access_on_scalar() {
    let source = "\
pub index Phase = { Coast, Burn };
param x: Dimensionless = 1.0;
param bad: Dimensionless = @x[Phase::Coast];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::EvalError { .. }),
        "got: {err:?}"
    );
}

// --- Index access with wrong index name ---

#[test]
fn check_index_access_wrong_index() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Stage = { First, Second };
param x: Dimensionless[Phase] = {
Phase::Coast: 1.0,
Phase::Burn: 2.0,
};
param bad: Dimensionless = @x[Stage::First];";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through if/else sub-expressions ---

#[test]
fn check_if_error_in_condition() {
    // Error inside condition sub-expression (unknown unit)
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if (1.0 foobar > 0.0) { 1.0 } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_if_error_in_then_branch() {
    // Error in then-branch sub-expression
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 1.0 foobar } else { 0.0 };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn check_if_error_in_else_branch() {
    // Error in else-branch sub-expression
    let source = "\
param x: Dimensionless = 1.0;
node bad: Dimensionless = if true { 0.0 } else { 1.0 foobar };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through convert sub-expression ---

#[test]
fn check_convert_error_in_inner() {
    // Error inside the inner expression of a convert
    let source = "\
node bad: Length = (1.0 foobar) -> m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through block binding ---

// --- Error propagation through field access inner expression ---

#[test]
fn check_field_access_error_in_inner() {
    let source = "\
type Orbit { altitude: Length, speed: Velocity }
node bad: Length = (1.0 foobar).altitude;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through struct construction field value ---

#[test]
fn check_struct_construction_error_in_field_value() {
    let source = "\
type Orbit { altitude: Length, speed: Velocity }
node o: Orbit = Orbit { altitude: 1.0 foobar, speed: 7.6 km / s };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through for comprehension body ---

#[test]
fn check_for_comp_error_in_body() {
    let source = "\
pub index Phase = { Coast, Burn };
node bad: Dimensionless[Phase] = for p: Phase { 1.0 foobar };";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through aggregation arg ---

#[test]
fn check_aggregation_error_in_arg() {
    let source = "\
node bad: Dimensionless = sum(1.0 foobar);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through scan source/init ---

#[test]
fn check_scan_error_in_source() {
    let source = "\
pub index Phase = { Coast, Burn };
node bad: Dimensionless[Phase] = scan(1.0 foobar, 0.0, |acc, val| acc + val);";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Error propagation through map literal entry ---

#[test]
fn check_map_literal_error_in_entry() {
    let source = "\
pub index Phase = { Coast, Burn };
param bad: Dimensionless[Phase] = {
Phase::Coast: 1.0 foobar,
Phase::Burn: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::UnknownUnit { .. }),
        "got: {err:?}"
    );
}

// --- Map literal with mixed index names ---

#[test]
fn check_map_literal_mixed_index_names() {
    let source = "\
pub index Phase = { Coast, Burn };
pub index Stage = { First, Second };
param x: Dimensionless[Phase] = {
Phase::Coast: 1.0,
Stage::Second: 2.0,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IndexMismatch { .. }),
        "got: {err:?}"
    );
}

// --- Block let-binding with valid type annotation ---

// -----------------------------------------------------------------------
// Fin(N) type: loop variable bounds checking
// -----------------------------------------------------------------------

#[test]
fn fin_same_size_indexing() {
    // i : Fin(3) indexing into D[3] — 3 <= 3 — safe
    let source = "\
param v: Dimensionless[3] = for i: range(3) { 1.0 };
node w: Dimensionless[3] = for i: range(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_smaller_bound_indexing() {
    // i : Fin(3) indexing into D[5] — 3 <= 5 — safe
    let source = "\
param v: Dimensionless[5] = for i: range(5) { 1.0 };
node w: Dimensionless[3] = for i: range(3) { @v[i] };";
    check(source).unwrap();
}

#[test]
fn fin_out_of_bounds() {
    // i : Fin(5) indexing into D[3] — 5 > 3 — compile error
    let source = "\
param v: Dimensionless[3] = for i: range(3) { 1.0 };
node w: Dimensionless[5] = for i: range(5) { @v[i] };";
    let err = check(source).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("index out of bounds"),
        "expected bounds error, got: {msg}"
    );
}

#[test]
fn fin_comparison_same_range() {
    // i : Fin(3), j : Fin(3) — i == j is valid
    let source = "\
node m: Dimensionless[3, 3] = for i: range(3), j: range(3) {
    if i == j { 1.0 } else { 0.0 }
};";
    check(source).unwrap();
}

#[test]
fn fin_arithmetic_with_int() {
    // Fin(N) * Dimensionless -> error (Fin is not Scalar)
    // Fin(N) + Fin(M) -> Int (arithmetic), then Int * Dimensionless -> error
    // to_float(i) -> Dimensionless (via Int coercion)
    let source = "\
node v: Dimensionless[3] = for i: range(3) { to_float(i) };";
    check(source).unwrap();
}

// -----------------------------------------------------------------------
// Domain constraint bound dimensions (#438)
// -----------------------------------------------------------------------

#[test]
fn domain_bound_unit_literal_matches() {
    let source = "param m: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;";
    check(source).unwrap();
}

#[test]
fn domain_bound_dimensionless_accepts_int() {
    // Bare Int literal is accepted as a Dimensionless bound (existing behavior).
    let source = "param r: Dimensionless(min: 0, max: 1) = 0.5;";
    check(source).unwrap();
}

#[test]
fn domain_bound_bare_number_on_dimensioned_rejected() {
    // Bare numbers infer as Dimensionless, mismatching Mass.
    // This is the implicit-unit-attachment case from #440.
    let source = "param m: Mass(min: 1.0, max: 100.0 kg) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_bare_int_on_dimensioned_rejected() {
    // Integer literal on a dimensioned scalar should also be rejected.
    let source = "param m: Mass(min: 1, max: 100.0 kg) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_creates_wrong_dimension() {
    // 1.0 m / 1.0 s is Velocity, but the constrained type is Length.
    let source = "param d: Length(min: 1.0 m / 1.0 s) = 5.0 m;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_division_inverse_dimension() {
    // 1.0 / 1.0 kg is 1/Mass, not Mass.
    let source = "param x: Mass(min: 1.0 / 1.0 kg) = 5.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_addition_unit_mismatch_in_bound() {
    // 5.0 m + 3.0 s is itself a dimension mismatch inside the bound expression.
    let source = "param t: Time(min: 5.0 m + 3.0 s) = 10.0 s;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn domain_bound_convert_preserves_dimension() {
    // Conversion between units of the same dimension is fine.
    let source = "param m: Mass(min: 1.0 kg -> g) = 5.0 kg;";
    check(source).unwrap();
}

#[test]
fn domain_bound_multiplication_creates_correct_dimension() {
    // 10.0 kg * 9.8 m / s^2 is Force; Force(min: ...) accepts it.
    let source = "param f: Force(min: 10.0 kg * 9.8 m / s^2) = 100.0 N;";
    check(source).unwrap();
}

#[test]
fn domain_bound_indexed_dimension_checked() {
    // Constraints on the base of an indexed type are also checked.
    let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity(min: 1.0 m)[Maneuver] = {
Maneuver::Departure: 1.0 m / s,
Maneuver::Correction: 0.5 m / s,
};";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

// -----------------------------------------------------------------------
// Int domain bound must be unitless (#439)
// -----------------------------------------------------------------------

#[test]
fn int_domain_bound_int_literal_accepted() {
    let source = "param n: Int(min: 1, max: 100) = 5;";
    check(source).unwrap();
}

#[test]
fn int_domain_bound_dimensionless_scalar_accepted() {
    let source = "param n: Int(min: 0.0, max: 100.0) = 5;";
    check(source).unwrap();
}

#[test]
fn int_domain_bound_with_unit_rejected() {
    let source = "param n: Int(min: 1.0 kg, max: 10.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

#[test]
fn int_domain_bound_arithmetic_with_unit_rejected() {
    // Arithmetic that produces a dimensioned result is also rejected.
    let source = "param n: Int(min: 1.0 m / 1.0 s) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

// -----------------------------------------------------------------------
// Domain bound dimension checks on const nodes (#441)
// -----------------------------------------------------------------------

#[test]
fn const_domain_bound_dimension_checked() {
    // Const nodes get the same compile-time bound dimension check as params/nodes.
    let source = "const node MAX_M: Mass(min: 1.0 m) = 50.0 kg;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::DomainDimensionMismatch { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_domain_bound_int_with_unit_rejected() {
    let source = "const node MAX_N: Int(min: 1.0 kg) = 5;";
    let err = check(source).unwrap_err();
    assert!(
        matches!(err, GraphcalError::IntDomainBoundNotUnitless { .. }),
        "got: {err:?}"
    );
}

#[test]
fn const_domain_bound_well_formed_passes_dim_check() {
    // Well-formed const constraint passes dim_check (value-vs-bound check is
    // in exec_plan, not dim_check).
    let source = "const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;";
    check(source).unwrap();
}

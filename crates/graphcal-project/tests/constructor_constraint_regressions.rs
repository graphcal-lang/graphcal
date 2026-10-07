//! Constructor field validation must cover temporary compile-time values (#1944).

use graphcal_project::prepare::compile_and_eval;

#[test]
fn const_projection_cannot_discard_a_field_constraint_violation() {
    let error = compile_and_eval(
        r"
type Spec { Spec(mass: Mass(max: 2000.0 kg)) }
const node BAD: Mass = Spec(mass: 5000.0 kg).mass;
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn declaration_bound_cannot_discard_a_field_constraint_violation() {
    let error = compile_and_eval(
        r"
type Spec { Spec(mass: Mass(max: 2000.0 kg)) }
param mass: Mass(max: Spec(mass: 5000.0 kg).mass) = 100.0 kg;
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn field_bound_cannot_discard_a_field_constraint_violation() {
    let error = compile_and_eval(
        r"
type Spec { Spec(mass: Mass(max: 2000.0 kg)) }
type Other { Other(mass: Mass(max: Spec(mass: 5000.0 kg).mass)) }
node good: Other = Other(mass: 100.0 kg);
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn deferred_checks_use_concrete_nat_arguments() {
    let error = compile_and_eval(
        r"
type Bounded<N: Nat> { Bounded(value: Int(max: to_int(key(Fin(N), 0)))) }
const node BAD: Int = Bounded<3>(value: 1).value;
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn valid_temporary_constructors_and_unselected_branches_are_accepted() {
    compile_and_eval(
        r"
type Spec { Spec(mass: Mass(min: 100.0 kg, max: 2000.0 kg)) }
const node LIMIT: Mass = Spec(mass: 1500.0 kg).mass;
const node GOOD: Mass = if true { Spec(mass: 100.0 kg).mass } else { Spec(mass: 5000.0 kg).mass };
param mass: Mass(max: @LIMIT) = @GOOD;
",
    )
    .unwrap();
}

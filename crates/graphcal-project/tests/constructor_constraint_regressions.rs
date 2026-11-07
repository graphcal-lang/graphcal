//! Constructor field validation must cover temporary compile-time values (#1944).

use std::collections::HashMap;

use graphcal_io::RealFileSystem;
use graphcal_project::prepare::{compile_and_eval, compile_and_eval_project};

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
fn match_cannot_discard_a_constrained_union_payload() {
    let error = compile_and_eval(
        r"
type Choice { Some(value: Int(max: 0)), None }
const node BAD: Int = match Some(value: 1) {
    Some(value: v) => v,
    None => 0,
};
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("Some.value"), "{error}");
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn nested_temporary_constructors_are_checked() {
    let error = compile_and_eval(
        r"
type Spec { Spec(mass: Mass(max: 2000.0 kg)) }
type Wrapper { Wrapper(spec: Spec) }
const node BAD: Mass = Wrapper(spec: Spec(mass: 5000.0 kg)).spec.mass;
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("Spec.mass"), "{error}");
}

#[test]
fn included_dag_constants_retain_constructor_checks() {
    let error = compile_and_eval(
        r"
dag bounded {
    type Spec { Spec(mass: Mass(max: 2000.0 kg)) }
    pub const node BAD: Mass = Spec(mass: 5000.0 kg).mass;
}
include bounded()::{ BAD };
",
    )
    .unwrap_err();
    assert!(error.to_string().contains("above maximum"), "{error}");
}

#[test]
fn imported_generic_constructor_uses_its_owners_constraints() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("src/constraints");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        directory.path().join("graphcal.toml"),
        "[package]\nname = \"constraints\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("schema.gcl"),
        r"
const node LIMIT: Int = 0;
pub type Bounded<N: Nat> { Bounded(value: Int(max: @LIMIT)) }
",
    )
    .unwrap();
    let root = package.join("main.gcl");
    std::fs::write(
        &root,
        r"
import constraints.schema::{ type Bounded, Bounded };
const node LIMIT: Int = 100;
const node BAD: Int = Bounded<3>(value: 1).value;
",
    )
    .unwrap();
    let error = compile_and_eval_project(&root, &HashMap::new(), None, &RealFileSystem::default())
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

/// Sealing reads every DAG's bounds from one map of canonically keyed
/// constants instead of a per-DAG copy (#2251): same-named constants of
/// different DAGs still bound only their own declarations.
#[test]
fn bounds_read_their_own_dags_constants_without_per_dag_copies() {
    let result = compile_and_eval(
        r"
dag tight {
    const node LIMIT: Dimensionless = 1.0;
    param x: Dimensionless(max: @LIMIT);
    pub node out: Dimensionless = @x;
}
dag loose {
    const node LIMIT: Dimensionless = 10.0;
    param x: Dimensionless(max: @LIMIT);
    pub node out: Dimensionless = @x;
}
const node LIMIT: Dimensionless = 100.0;
node from_tight: Dimensionless = @tight(x: 5.0)::out;
node from_loose: Dimensionless = @loose(x: 5.0)::out;
",
    )
    .unwrap();
    let outcome = |name: &str| {
        result
            .nodes()
            .find(|(candidate, _)| candidate.to_string() == name)
            .map(|(_, outcome)| outcome.is_ok())
    };
    assert_eq!(outcome("from_tight"), Some(false));
    assert_eq!(outcome("from_loose"), Some(true));
}

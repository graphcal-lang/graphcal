//! Generic arguments, sorts, and defaults.

use super::*;

#[test]
fn sorted_generic_defaults_are_substituted_in_type_and_constructor_positions() {
    let source = r"
pub index Component = { X, Y, Z };
pub type Marker { Marker }
pub type Fixed<
    D: Dim,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    Fixed(value: D),
}

pub type AllDefaults<
    D: Dim = Dimensionless,
    I: Index = Component,
    F: Type = Marker,
    N: Nat = 3,
> {
    AllDefaults(value: D),
}

param value: Fixed<Dimensionless> = Fixed<Dimensionless>(value: 1.0);
param all_defaults: AllDefaults = AllDefaults(value: 2.0);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_defaults_compose_across_all_generic_sorts() {
    let source = r"
pub index Phase = { A };
pub type Marker { Marker }
pub type Fixed<N: Nat> { Fixed(value: Dimensionless) }
pub type Bundle<
    D: Dim,
    I: Index,
    T: Type,
    N: Nat,
    E: Dim = D * D,
    J: Index = I,
    U: Type = T,
    M: Nat = N + 1,
> {
    Bundle(scalar: E, payload: U, series: E[J], fixed: Fixed<M>),
}

param value: Bundle<Length, Phase, Marker, 2> = Bundle<Length, Phase, Marker, 2>(
    scalar: 1.0 m^2,
    payload: Marker,
    series: { Phase#A: 2.0 m^2 },
    fixed: Fixed<3>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn dependent_nat_defaults_compose_through_nested_type_defaults() {
    let source = r"
pub type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

pub type Holder<N: Nat, M: Nat = N + 1, F: Type = Fixed<M>> {
    Holder(value: F),
}

param value: Holder<2> = Holder<2>(value: Fixed<3>(value: 1.0));
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_defaults_may_reference_only_earlier_parameters() {
    let error = compile_and_eval(
        "pub type Invalid<N: Nat = M, M: Nat = 3> { Invalid(value: Dimensionless) }",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("default for generic parameter `N` may reference only earlier generic parameters; `M` is not earlier"),
        "unexpected error: {error}"
    );
}

#[test]
fn generic_defaults_must_form_a_trailing_parameter_suffix() {
    let error =
        compile_and_eval("pub type Invalid<N: Nat = 3, M: Nat> { Invalid(value: Dimensionless) }")
            .unwrap_err();
    assert!(
        error.to_string().contains(
            "generic parameter `M` without a default cannot follow defaulted parameter `N`"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn nested_type_generic_arguments_apply_inner_sorted_defaults() {
    let source = r"
pub type Fixed<D: Dim, N: Nat = 3> {
    Fixed(value: D),
}

pub type Holder<F: Type> {
    Holder(value: F),
}

param value: Holder<Fixed<Dimensionless>> = Holder<Fixed<Dimensionless>>(
    value: Fixed<Dimensionless>(value: 1.0),
);
";

    compile_and_eval(source).unwrap();
}

#[test]
fn multi_declaration_supports_finite_shared_axes() {
    let source = r"
param x: Dimensionless[Fin(2)],
param y: Dimensionless[Fin(2)]
  = table[Fin(2), (_, _)] {
      : _, _;
      1.0, 2.0;
      3.0, 4.0;
  };
";
    let result = compile_and_eval(source).unwrap();
    for name in ["x", "y"] {
        let value = result
            .params()
            .find(|(candidate, _)| candidate.to_string() == name)
            .and_then(|(_, value)| value.as_ref().ok())
            .expect("finite multi-decl param");
        assert!(matches!(value, Value::Indexed { entries, .. } if entries.len() == 2));
    }
}

#[test]
fn finite_indexes_remain_distinct_from_nat_generic_arguments() {
    let source = r"
pub type Vector<N: Nat, D: Dim> {
    Vector(values: D[Fin(N)]),
}
pub type IndexedVector<I: Index, D: Dim> {
    IndexedVector(values: D[I]),
}
pub type DefaultIndexed<I: Index = Fin(2)> {
    DefaultIndexed(values: Dimensionless[I]),
}

param defaulted: DefaultIndexed = DefaultIndexed(
    values: table[Fin(2)] { 7.0; 8.0; },
);
param vector: Vector<3, Dimensionless> = Vector<3, Dimensionless>(
    values: table[Fin(3)] { 1.0; 2.0; 3.0; },
);
param indexed: IndexedVector<Fin(3), Dimensionless> =
    IndexedVector<Fin(3), Dimensionless>(
        values: table[Fin(3)] { 4.0; 5.0; 6.0; },
    );
";

    compile_and_eval(source).unwrap();
}

#[test]
fn generic_arguments_reject_sort_crossing() {
    let cases = [
        (
            "pub type ByIndex<I: Index> { ByIndex(value: Dimensionless) }\nparam value: ByIndex<3>;",
            "expected Index, found Nat `3`",
        ),
        (
            "pub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Fin(3)>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByNat<N: Nat> { ByNat(value: Dimensionless) }\nparam value: ByNat<Phase>;",
            "sort `Nat`",
        ),
        (
            "pub index Phase = { A };\npub type ByType<T: Type> { ByType(value: T) }\nparam value: ByType<Dimensionless[Phase]>;",
            "sort `Type`",
        ),
    ];

    for (source, expected) in cases {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn bare_nat_index_positions_are_rejected() {
    for source in [
        "param value: Dimensionless[3];",
        "pub type Sized<N: Nat> { Sized(values: Dimensionless[N]) }",
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains("expected Index, found Nat"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn finite_index_cardinality_is_validated_before_allocation() {
    for (source, expected) in [
        (
            "param value: Dimensionless[Fin(0)];",
            "Fin(0) is not allowed",
        ),
        (
            "param value: Dimensionless[Fin(1000001)];",
            "exceeds the practical limit",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<0>;",
            "Fin(0) is not allowed",
        ),
        (
            "pub type Vector<N: Nat> { Vector(values: Dimensionless[Fin(N)]) }\nparam value: Vector<1000001>;",
            "exceeds the practical limit",
        ),
        (
            "pub type InvalidDefault<I: Index = Fin(0)> { InvalidDefault(value: Dimensionless) }",
            "Fin(0) is not allowed",
        ),
    ] {
        let error = compile_and_eval(source).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn non_empty_function_generic_arguments_are_rejected() {
    let error = compile_and_eval("node value: Dimensionless = sqrt<3>(4.0);").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("function `sqrt` does not accept generic arguments"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_generic_struct_defaults_preserve_same_leaf_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("src/collide");
    std::fs::create_dir_all(&root_dir).unwrap();
    std::fs::write(
        dir.path().join("graphcal.toml"),
        "[package]\nname = \"collide\"\n",
    )
    .unwrap();
    let module_source = "pub type Marker { Marker }\n\
         pub type Wrap<D: Dim, F: Type = Marker> { Wrap(value: D) }\n";
    std::fs::write(root_dir.join("a.gcl"), module_source).unwrap();
    std::fs::write(root_dir.join("b.gcl"), module_source).unwrap();
    let root = root_dir.join("main.gcl");
    std::fs::write(
        &root,
        "import collide.a as a;\n\
         import collide.b as b;\n\
         node a_wrap: a::Wrap<Length> = a::Wrap<Length>(value: 1.0 m);\n\
         node b_wrap: b::Wrap<Time> = b::Wrap<Time>(value: 1.0 s);\n",
    )
    .unwrap();

    let (tir, project) = compile_to_tir_project(&root, None, &fs()).unwrap();
    let a_id = loaded_file_dag_id(&project, "a.gcl");
    let b_id = loaded_file_dag_id(&project, "b.gcl");
    let marker_owner = |decl: &str| {
        let graphcal_compiler::tir::typed::ResolvedDeclType::Value(
            graphcal_compiler::tir::typed::ResolvedValueType::Struct {
                name: wrap,
                generic_args,
                ..
            },
        ) = root_decl_type(&tir, decl).resolved()
        else {
            panic!("expected generic struct annotation for `{decl}`");
        };
        assert_eq!(wrap.as_str(), "Wrap");
        let graphcal_compiler::tir::typed::ResolvedGenericArg::Type(
            graphcal_compiler::tir::typed::ResolvedValueType::Struct {
                name: marker_resolved,
                ..
            },
        ) = &generic_args[1]
        else {
            panic!(
                "expected default marker type arg for `{decl}`, got {:?}",
                generic_args[1]
            );
        };
        assert_eq!(marker_resolved.as_str(), "Marker");
        marker_resolved.owner().clone()
    };

    assert_eq!(marker_owner("a_wrap"), a_id);
    assert_eq!(marker_owner("b_wrap"), b_id);
}

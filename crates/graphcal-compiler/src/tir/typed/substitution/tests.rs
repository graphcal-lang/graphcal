use super::*;
use crate::dag_id::DagId;
use crate::dimension::{BaseDimId, PreludeBaseDimension};
use crate::generic_param::GenericParamOwner;
use crate::generic_param::test_support::type_param;
use crate::registry::types::FiniteIndex;
use crate::resolved_name::{ResolvedIndexName, ResolvedStructTypeName};
use crate::syntax::index_name::IndexName;
use crate::syntax::type_name::{GenericParamName, StructTypeName};

fn span() -> Span {
    Span::new(0, 1)
}

fn length() -> Dimension {
    Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length))
}

fn time() -> Dimension {
    Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Time))
}

fn nat(param: &GenericParamId) -> NatPolyForm {
    NatPolyForm::from_var(param.clone())
}

fn dim_term(param: &GenericParamId, power: i16, op: MulDivOp) -> ResolvedDimTerm {
    ResolvedDimTerm::GenericParam {
        name: param.clone(),
        power: Rational::from(power),
        op,
        span: span(),
    }
}

#[test]
fn unbound_parameters_stay_symbolic() {
    let d = type_param("D");
    let generic = ResolvedTypeExpr::GenericDimParam(d, span());
    assert_eq!(Substitution::default().apply(&generic), Ok(generic));
}

#[test]
fn dimension_products_collapse_once_every_term_is_bound() {
    let d = type_param("D");
    let e = type_param("E");
    // D^2 / E
    let product = ResolvedTypeExpr::GenericDimExpr {
        terms: vec![
            dim_term(&d, 2, MulDivOp::Mul),
            dim_term(&e, 1, MulDivOp::Div),
        ],
        span: span(),
    };
    let mut partial = Substitution::default();
    partial.bind(
        d,
        ResolvedGenericArg::Dim(ResolvedDimArg::Concrete(length())),
    );
    let ResolvedTypeExpr::GenericDimExpr { terms, .. } = partial.apply(&product).unwrap() else {
        panic!("a product with an unbound term stays symbolic");
    };
    assert!(matches!(terms.as_slice(), [
        ResolvedDimTerm::Concrete { .. },
        ResolvedDimTerm::GenericParam { name, .. },
    ] if name == &e));

    let mut full = partial;
    full.bind(e, ResolvedGenericArg::Dim(ResolvedDimArg::Concrete(time())));
    let expected = length()
        .pow(2)
        .and_then(|squared| squared.checked_div(&time()))
        .unwrap();
    assert_eq!(
        full.apply(&product).unwrap(),
        ResolvedTypeExpr::Quantity(expected)
    );
}

#[test]
fn dimension_parameters_bound_to_products_are_expanded_with_their_power() {
    let d = type_param("D");
    let e = type_param("E");
    // D / Time with D := E^2 gives E^2 / Time, still symbolic in E.
    let quotient = ResolvedDimArg::Expr {
        terms: vec![
            dim_term(&d, 1, MulDivOp::Mul),
            ResolvedDimTerm::Concrete {
                dim: time(),
                power: Rational::ONE,
                op: MulDivOp::Div,
            },
        ],
        span: span(),
    };
    let mut substitution = Substitution::default();
    substitution.bind(
        d,
        ResolvedGenericArg::Dim(ResolvedDimArg::Expr {
            terms: vec![dim_term(&e, 2, MulDivOp::Mul)],
            span: span(),
        }),
    );
    let ResolvedGenericArg::Dim(ResolvedDimArg::Expr { terms, .. }) = substitution
        .apply_generic_arg(&ResolvedGenericArg::Dim(quotient))
        .unwrap()
    else {
        panic!("the product stays symbolic in E");
    };
    assert!(matches!(terms.as_slice(), [
        ResolvedDimTerm::GenericParam { name, power, op: MulDivOp::Mul, .. },
        ResolvedDimTerm::Concrete { op: MulDivOp::Div, .. },
    ] if name == &e && *power == Rational::from(2)));
}

#[test]
fn index_type_and_nat_parameters_are_replaced_everywhere() {
    let owner = DagId::root_in_package("test", "main");
    let phase = ResolvedIndexName::from_def(owner.clone(), IndexName::expect_valid("Phase"));
    let wrapper = ResolvedStructTypeName::from_def(owner, StructTypeName::expect_valid("Wrap"));
    let i = type_param("I");
    let n = type_param("N");
    let t = type_param("T");
    // Wrap<T, N + 1>[I, Fin(2 * N)]
    let doubled = NatPolyForm::from_constant(2).mul(&nat(&n)).unwrap();
    let successor = nat(&n).add(&NatPolyForm::from_constant(1)).unwrap();
    let symbolic = ResolvedTypeExpr::Indexed {
        base: Box::new(ResolvedTypeExpr::GenericStruct {
            name: wrapper.clone(),
            generic_args: vec![
                ResolvedGenericArg::Type(ResolvedTypeExpr::GenericTypeParam(t.clone(), span())),
                ResolvedGenericArg::Nat(successor, span()),
            ],
            span: span(),
        }),
        indexes: vec![
            ResolvedIndex::GenericParam(i.clone(), span()),
            ResolvedIndex::Finite(doubled, span()),
        ],
    };
    let mut substitution = Substitution::for_nats([(&n, &3)]);
    substitution.bind(
        i,
        ResolvedGenericArg::Index(ResolvedIndex::Concrete(phase.clone(), span())),
    );
    substitution.bind(t, ResolvedGenericArg::Type(ResolvedTypeExpr::Bool));
    assert_eq!(
        substitution.apply(&symbolic).unwrap(),
        ResolvedTypeExpr::Indexed {
            base: Box::new(ResolvedTypeExpr::GenericStruct {
                name: wrapper,
                generic_args: vec![
                    ResolvedGenericArg::Type(ResolvedTypeExpr::Bool),
                    ResolvedGenericArg::Nat(NatPolyForm::from_constant(4), span()),
                ],
                span: span(),
            }),
            indexes: vec![
                ResolvedIndex::Concrete(phase, span()),
                ResolvedIndex::Finite(NatPolyForm::from_constant(6), span()),
            ],
        }
    );
}

#[test]
fn for_params_binds_owner_qualified_parameters_pairwise() {
    let owner = DagId::root_in_package("test", "main");
    let make_owner = |name| {
        GenericParamOwner::Type(ResolvedStructTypeName::from_def(
            owner.clone(),
            StructTypeName::expect_valid(name),
        ))
    };
    let local = GenericParamId::new(make_owner("Local"), GenericParamName::expect_valid("N"));
    let foreign = GenericParamId::new(make_owner("Foreign"), GenericParamName::expect_valid("N"));
    let params = [crate::hir::nominal::NominalGenericParam::new(
        local.clone(),
        crate::syntax::ast::GenericConstraint::Nat,
        None,
        span(),
    )];
    let substitution = Substitution::for_params(
        &params,
        &[ResolvedGenericArg::Nat(
            NatPolyForm::from_constant(5),
            span(),
        )],
    );
    let form = |param: &GenericParamId| ResolvedIndex::Finite(nat(param), span());
    assert_eq!(
        substitution.apply_index(&form(&local)).unwrap(),
        ResolvedIndex::Finite(NatPolyForm::from_constant(5), span())
    );
    // A same-spelled parameter of another owner is a different variable.
    assert_eq!(
        substitution.apply_index(&form(&foreign)).unwrap(),
        form(&foreign)
    );
}

#[test]
fn declared_types_close_only_under_complete_nat_bindings() {
    let n = type_param("N");
    let other = type_param("M");
    let symbolic_axis = |param: &GenericParamId| {
        IndexTypeRef::from_finite_index_form(
            nat(param).add(&NatPolyForm::from_constant(1)).unwrap(),
        )
        .unwrap()
    };
    let declared = CheckedType::Indexed {
        element: Box::new(CheckedType::Bool),
        index: symbolic_axis(&n),
    };
    let substitution = Substitution::for_nats([(&n, &2)]);
    assert_eq!(
        substitution.apply_declared(&declared, span()).unwrap(),
        CheckedType::Indexed {
            element: Box::new(CheckedType::Bool),
            index: IndexTypeRef::from_finite_index(FiniteIndex::try_from_u64(3).unwrap()),
        }
    );
    let unbound = CheckedType::Key(symbolic_axis(&other));
    assert_eq!(
        substitution.apply_declared(&unbound, span()),
        Err(SubstitutionError::UnboundNat {
            param: other,
            span: span(),
        })
    );
    let overflowing = CheckedType::Key(
        IndexTypeRef::from_finite_index_form(nat(&n).mul(&nat(&n)).unwrap()).unwrap(),
    );
    assert_eq!(
        Substitution::for_nats([(&n, &u64::MAX)]).apply_declared(&overflowing, span()),
        Err(SubstitutionError::NatOverflow { span: span() })
    );
}

#[test]
fn errors_render_at_their_span() {
    let src = NamedSource::new("test.gcl", Arc::new(String::new()));
    assert!(matches!(
        SubstitutionError::DimensionOverflow { span: span() }.into_graphcal(&src),
        GraphcalError::DimensionOverflow { .. }
    ));
    assert!(matches!(
        SubstitutionError::NatOverflow { span: span() }.into_graphcal(&src),
        GraphcalError::EvalError { message, .. } if message.contains("Nat arithmetic overflow")
    ));
    assert!(matches!(
        SubstitutionError::InvalidFiniteIndex {
            error: crate::registry::types::IndexCardinalityError::Empty,
            span: span(),
        }
        .into_graphcal(&src),
        GraphcalError::EvalError { .. }
    ));
}

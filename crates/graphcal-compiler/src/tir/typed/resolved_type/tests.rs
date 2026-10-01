use super::*;
use crate::dag_id::DagId;
use crate::dimension::{BaseDimId, PreludeBaseDimension};
use crate::semantic_error::SemanticErrorKind;
use crate::semantic_error::evaluation::EvaluationError;
use crate::syntax::index_name::IndexName;
use crate::syntax::type_name::StructTypeName;

fn src() -> crate::source_id::SourceId {
    crate::source_registry::SourceRegistry::new()
        .register("test.gcl", std::sync::Arc::new(String::new()))
}

fn registry() -> FormattingRegistry {
    FormattingRegistry::graphcal_prelude().unwrap()
}

fn length() -> Dimension {
    Dimension::base(BaseDimId::Prelude(PreludeBaseDimension::Length))
}

fn param_term(name: &str, power: Rational, op: MulDivOp) -> ResolvedDimTerm {
    ResolvedDimTerm::GenericParam {
        name: crate::generic_param::test_support::type_param(name),
        power,
        op,
        span: Span::new(1, 2),
    }
}

fn symbolic(terms: Vec<ResolvedDimTerm>) -> ResolvedDim {
    ResolvedDim::Symbolic {
        terms,
        span: Span::new(0, 5),
    }
}

#[test]
fn lone_generic_param_recognizes_only_a_bare_parameter() {
    let lone = symbolic(vec![param_term("D", Rational::ONE, MulDivOp::Mul)]);
    assert_eq!(
        lone.lone_generic_param(),
        Some((
            &crate::generic_param::test_support::type_param("D"),
            Span::new(1, 2)
        ))
    );
    for not_lone in [
        symbolic(vec![param_term("D", Rational::from(2), MulDivOp::Mul)]),
        symbolic(vec![param_term("D", Rational::ONE, MulDivOp::Div)]),
        symbolic(vec![
            param_term("D", Rational::ONE, MulDivOp::Mul),
            ResolvedDimTerm::Concrete {
                dim: length(),
                power: Rational::ONE,
                op: MulDivOp::Mul,
            },
        ]),
        ResolvedDim::Concrete(length()),
    ] {
        assert_eq!(not_lone.lone_generic_param(), None);
    }
}

#[test]
fn dimensionless_has_one_spelling() {
    let registry = registry();
    assert_eq!(
        ResolvedDim::dimensionless(),
        ResolvedDim::Concrete(Dimension::dimensionless())
    );
    assert_eq!(
        ResolvedValueType::Quantity(ResolvedDim::dimensionless()).format(&registry),
        "Dimensionless"
    );
    assert_eq!(
        ResolvedValueType::Complex {
            dimension: ResolvedDim::dimensionless(),
            span: Span::new(0, 0),
        }
        .format(&registry),
        "Complex<Dimensionless>"
    );
}

#[test]
fn struct_format_omits_empty_argument_list() {
    let owner = DagId::root_in_package("test", "main");
    let name = ResolvedStructTypeName::for_test(owner, StructTypeName::expect_valid("Orbit"));
    let registry = registry();
    let plain = ResolvedValueType::Struct {
        name: name.clone(),
        generic_args: Vec::new(),
        span: Span::new(0, 0),
    };
    assert_eq!(plain.format(&registry), "Orbit");
    let applied = ResolvedValueType::Struct {
        name,
        generic_args: vec![ResolvedGenericArg::Nat(
            NatPolyForm::from_constant(3),
            Span::new(0, 0),
        )],
        span: Span::new(0, 0),
    };
    assert_eq!(applied.format(&registry), "Orbit<3>");
}

#[test]
fn decl_type_exposes_element_and_axes() {
    let owner = DagId::root_in_package("test", "main");
    let axis = ResolvedIndex::Concrete(
        ResolvedIndexName::for_test(owner, IndexName::expect_valid("Phase")),
        Span::new(0, 0),
    );
    let element = ResolvedValueType::Quantity(ResolvedDim::Concrete(length()));
    let scalar = ResolvedDeclType::Value(element.clone());
    assert_eq!(scalar.element(), &element);
    assert!(scalar.indexes().is_empty());

    let indexed = ResolvedDeclType::Indexed {
        element: element.clone(),
        indexes: NonEmpty::new(axis.clone(), vec![axis]),
    };
    assert_eq!(indexed.element(), &element);
    assert_eq!(indexed.indexes().len(), 2);
    assert_eq!(indexed.format(&registry()), "Length[Phase, Phase]");
    let CheckedType::Indexed { element: outer, .. } = indexed.to_checked_type(src()).unwrap()
    else {
        panic!("expected an indexed checked type");
    };
    assert!(matches!(*outer, CheckedType::Indexed { .. }));
}

#[test]
fn symbolic_complex_and_generic_args_have_no_checked_type() {
    let lone = symbolic(vec![param_term("D", Rational::ONE, MulDivOp::Mul)]);
    let squared = symbolic(vec![param_term("D", Rational::from(2), MulDivOp::Mul)]);
    let message = |ty: &ResolvedValueType| match ty.to_checked_type(src()) {
        Err(GraphcalError::Located(crate::diagnostic::Diagnostic {
            kind: SemanticErrorKind::Evaluation(EvaluationError::Failed { message, .. }),
            ..
        })) => message,
        other => panic!("expected an evaluation error, got {other:?}"),
    };
    let complex = |dimension| ResolvedValueType::Complex {
        dimension,
        span: Span::new(0, 0),
    };
    assert_eq!(
        message(&complex(lone.clone())),
        "complex dimension parameter `D` is not bound"
    );
    assert_eq!(
        message(&complex(squared.clone())),
        "complex dimension expression is not concrete"
    );
    let owner = DagId::root_in_package("test", "main");
    let applied = |dimension| ResolvedValueType::Struct {
        name: ResolvedStructTypeName::for_test(owner.clone(), StructTypeName::expect_valid("Vec3")),
        generic_args: vec![ResolvedGenericArg::Dim(dimension)],
        span: Span::new(0, 0),
    };
    assert_eq!(
        message(&applied(lone)),
        "generic dimension parameter `D` is not bound"
    );
    assert_eq!(
        message(&applied(squared)),
        "generic dimension expression is not concrete"
    );
    assert_eq!(
        message(&ResolvedValueType::GenericTypeParam(
            crate::generic_param::test_support::type_param("T"),
            Span::new(0, 0),
        )),
        "cannot use generic type parameter `T` as a concrete type"
    );
}

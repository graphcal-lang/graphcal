use super::*;
use crate::dag_id::DagId;
use crate::hir::types::{GenericParamId, GenericParamOwner};
use crate::syntax::type_name::{GenericParamName, ResolvedStructTypeName, StructTypeName};

#[test]
fn bound_nat_resolution_is_lexical_and_preserves_missing_and_overflow_errors() {
    let dag = DagId::from_virtual_relative_path(std::path::Path::new("facts.gcl")).unwrap();
    let name = GenericParamName::expect_valid("N");
    let parameter = |owner| {
        GenericParamId::new(
            GenericParamOwner::Type(ResolvedStructTypeName::from_def(
                dag.clone(),
                StructTypeName::expect_valid(owner),
            )),
            name.clone(),
        )
    };
    let local = parameter("Local");
    let foreign = parameter("Foreign");
    let scope = HashMap::from([(name.clone(), local.clone())]);
    let form = crate::nat::NatPolyForm::from_var(name);
    let foreign_only = HashMap::from([(foreign.clone(), 99)]);
    assert!(
        matches!(evaluate_bound_nat(&form, &scope, &foreign_only), Err(BoundNatError::MissingBinding(id)) if id == local)
    );
    assert!(matches!(
        evaluate_bound_nat(&form, &HashMap::new(), &foreign_only),
        Err(BoundNatError::MissingScope(_))
    ));
    let bindings = HashMap::from([(foreign, 99), (local.clone(), 3)]);
    assert_eq!(evaluate_bound_nat(&form, &scope, &bindings).unwrap(), 3);
    let square = form.mul(&form).unwrap();
    assert!(matches!(
        evaluate_bound_nat(&square, &scope, &HashMap::from([(local, u64::MAX)])),
        Err(BoundNatError::Overflow(_))
    ));
}

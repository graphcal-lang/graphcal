use super::*;
use crate::dag_id::DagId;
use crate::hir::types::{GenericParamId, GenericParamOwner};
use crate::resolved_name::ResolvedStructTypeName;
use crate::syntax::type_name::{GenericParamName, StructTypeName};

#[test]
fn bound_nat_resolution_is_owner_qualified_and_preserves_missing_and_overflow_errors() {
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
    // The form names the owner-qualified parameter, so a same-spelled binding
    // of another owner never satisfies it.
    let form = crate::nat::NatPolyForm::from_var(local.clone());
    let foreign_only = HashMap::from([(foreign.clone(), 99)]);
    assert!(
        matches!(evaluate_bound_nat(&form, &foreign_only), Err(BoundNatError::MissingBinding(id)) if id == local)
    );
    let bindings = HashMap::from([(foreign, 99), (local.clone(), 3)]);
    assert_eq!(evaluate_bound_nat(&form, &bindings).unwrap(), 3);
    let square = form.mul(&form).unwrap();
    assert!(matches!(
        evaluate_bound_nat(&square, &HashMap::from([(local, u64::MAX)])),
        Err(BoundNatError::Overflow(_))
    ));
}

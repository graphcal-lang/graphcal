//! Runtime execution-plan selection from retained checked facts.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::tir::typed::{StructFieldConstraintKey, TIR};

use crate::decl_key::RuntimeDeclKey;
use crate::domain_check::ResolvedDomainConstraint;
use crate::execution_facts::{CheckedExecutionFacts, RuntimeValueMap};

/// A compiled execution plan ready for runtime evaluation.
#[derive(Debug)]
pub struct ExecPlan {
    /// Evaluated const values (in base SI units).
    /// Key-lookup only, order irrelevant.
    pub(crate) const_values: Arc<RuntimeValueMap>,
    /// Compile-time constants imported from dependency module artifacts.
    /// These are injected directly into the evaluation environment.
    /// Iterated once during env setup; feeds into `HashMap` (key-lookup only).
    pub(crate) imported_values: RuntimeValueMap,
    /// Topologically sorted names for runtime evaluation (params + nodes).
    pub(crate) topo_order: Vec<RuntimeDeclKey>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Key-lookup only, order irrelevant.
    pub(crate) assumes_map: HashMap<ScopedName, Vec<ScopedName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Key-lookup only, order irrelevant.
    pub(crate) expected_fail: HashMap<ScopedName, graphcal_compiler::ir::resolve::ExpectedFail>,
    /// Resolved domain constraints for runtime validation, keyed by declaration name.
    /// Key-lookup only, order irrelevant.
    pub(crate) domain_constraints: Arc<HashMap<RuntimeDeclKey, ResolvedDomainConstraint>>,
    /// Resolved domain constraints for struct/union member fields, keyed by
    /// owner-qualified struct/constructor/field identity. Looked up at every
    /// `ExprKind::ConstructorCall` evaluation to validate field values.
    pub(crate) struct_field_constraints:
        Arc<HashMap<StructFieldConstraintKey, ResolvedDomainConstraint>>,
    /// Per-DAG checked facts required by nested callable evaluation.
    pub(crate) checked_execution_facts: CheckedExecutionFacts,
}

/// Check a TIR and select its root execution plan.
///
/// This test convenience mirrors the production check-then-prepare pipeline.
///
/// # Errors
///
/// Returns a [`GraphcalError`] when static execution-fact checking or plan
/// selection fails.
#[cfg(test)]
pub fn compile(tir: &TIR, src: &NamedSource<Arc<String>>) -> Result<ExecPlan, GraphcalError> {
    compile_with_cancellation(
        tir,
        src,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
}

/// Compile a TIR into an execution plan with cooperative cancellation.
///
/// # Errors
///
/// Returns a [`GraphcalError`] for an invalid plan or cancellation.
#[cfg(test)]
pub fn compile_with_cancellation(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    let facts = crate::project_compiler::check_execution_facts_with_cancellation(
        tir,
        src,
        cancellation,
    )?;
    compile_checked_with_cancellation(tir, &facts, src, cancellation)
}

/// Build a runtime schedule from facts retained by the checked project.
pub fn compile_checked_with_cancellation(
    tir: &TIR,
    facts: &CheckedExecutionFacts,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<ExecPlan, GraphcalError> {
    cancellation.checkpoint()?;
    let root_facts = facts.for_dag(tir.root_dag_id()).ok_or_else(|| {
        GraphcalError::internal_error(
            format!(
                "checked execution facts are missing root DAG `{}`",
                tir.root_dag_id()
            ),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })?;
    debug_assert_eq!(&root_facts.dag_id, tir.root_dag_id());

    Ok(ExecPlan {
        const_values: Arc::clone(&root_facts.const_values),
        imported_values: tir
            .root()
            .imported_bindings()
            .values()
            .filter_map(|binding| {
                binding.value().map(|value| {
                    (
                        RuntimeDeclKey::resolved(binding.target().clone()),
                        value.clone(),
                    )
                })
            })
            .collect(),
        topo_order: root_facts.topo_order.as_ref().clone(),
        assumes_map: tir.root().assumes_map().clone(),
        expected_fail: tir
            .root()
            .expected_fail_entries()
            .map(|(name, expected)| (name.clone(), expected.clone()))
            .collect(),
        domain_constraints: Arc::clone(&root_facts.domain_constraints),
        struct_field_constraints: Arc::clone(&facts.struct_field_constraints),
        checked_execution_facts: facts.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphcal_compiler::ir::lower::lower;
    use graphcal_compiler::registry::runtime_value::RuntimeValue;
    use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
    use graphcal_compiler::syntax::module_resolve::ModuleResolver;
    use graphcal_compiler::syntax::parser::Parser;
    use graphcal_compiler::tir::typed::{ProjectTypeStore, type_resolve_with_modules};

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test.gcl", Arc::new(source.to_string()))
    }

    fn compile_source(source: &str) -> Result<ExecPlan, GraphcalError> {
        let (tir, src) = tir_from_source(source);
        compile(&tir, &src)
    }

    fn tir_from_source(
        source: &str,
    ) -> (graphcal_compiler::tir::typed::TIR, NamedSource<Arc<String>>) {
        let raw_file = Parser::new(source).parse_file().unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_file);
        let file = desugared;
        let src = make_src(source);
        let ir = lower(&file, &src).unwrap();
        let mut resolver = ModuleResolver::default();
        resolver
            .add_module(ir.dag_id().clone(), &file.declarations)
            .unwrap();
        let mut project_types = ProjectTypeStore::default();
        project_types.insert_graphcal_prelude().unwrap();
        project_types.insert_local_hir(&ir).unwrap();
        let tir = type_resolve_with_modules(ir, &src, &resolver, &project_types).unwrap();
        (tir, src)
    }

    fn quantity(rv: &RuntimeValue) -> f64 {
        match rv {
            RuntimeValue::Quantity(v) => *v,
            other => panic!("expected quantity, got {other:?}"),
        }
    }

    fn test_dag_id() -> graphcal_compiler::dag_id::DagId {
        graphcal_compiler::dag_id::DagId::from_virtual_relative_path(std::path::Path::new(
            "test.gcl",
        ))
        .unwrap()
    }

    fn resolved_key(name: &str) -> RuntimeDeclKey {
        RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
            test_dag_id(),
            DeclName::expect_valid(name),
        ))
    }

    #[test]
    fn compile_simple_const() {
        let plan = compile_source("const node g0: Dimensionless = 9.80665;").unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("g0")]) - 9.80665).abs() < f64::EPSILON);
        assert!(plan.topo_order.is_empty());
    }

    #[test]
    fn compile_const_chain() {
        let plan = compile_source(
            "const node g0: Dimensionless = 9.80665;\nconst node two_g0: Dimensionless = 2.0 * @g0;",
        )
        .unwrap();
        assert!((quantity(&plan.const_values[&resolved_key("two_g0")]) - 19.6133).abs() < 1e-10);
    }

    #[test]
    fn checked_fact_stores_are_reused_by_runtime_planning() {
        let (tir, src) = tir_from_source(
            "const node lower: Dimensionless = 1.0;\n\
             param x: Dimensionless(min: @lower, max: 3.0) = 2.0;",
        );
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let facts = crate::project_compiler::check_execution_facts_with_cancellation(
            &tir,
            &src,
            &cancellation,
        )
        .unwrap();
        let plan = compile_checked_with_cancellation(&tir, &facts, &src, &cancellation).unwrap();
        let root_facts = facts.for_dag(tir.root_dag_id()).unwrap();

        assert!(Arc::ptr_eq(&root_facts.const_values, &plan.const_values));
        assert!(Arc::ptr_eq(
            &root_facts.domain_constraints,
            &plan.domain_constraints
        ));
        assert!(Arc::ptr_eq(
            &facts.struct_field_constraints,
            &plan.struct_field_constraints
        ));
    }

    #[test]
    fn compile_runtime_dag() {
        let plan = compile_source(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;\nnode z: Dimensionless = @y * 2.0;",
        )
        .unwrap();
        let x_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "x")
            .unwrap();
        let y_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "y")
            .unwrap();
        let z_pos = plan
            .topo_order
            .iter()
            .position(|n| n.member() == "z")
            .unwrap();
        assert!(x_pos < y_pos);
        assert!(y_pos < z_pos);
    }

    #[test]
    fn compile_const_cycle() {
        let err = compile_source(
            "const node a: Dimensionless = @b + 1.0;\nconst node b: Dimensionless = @a + 1.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_runtime_cycle() {
        let err =
            compile_source("node a: Dimensionless = @b + 1.0;\nnode b: Dimensionless = @a + 1.0;")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::CyclicDependency { .. }));
    }

    #[test]
    fn compile_uses_collected_semantic_const_deps() {
        let (tir, src) = tir_from_source(
            "const node a: Dimensionless = 1.0;\n\
             const node b: Dimensionless = @a + 1.0;",
        );
        let plan = compile(&tir, &src).unwrap();
        assert!(
            (quantity(
                &plan.const_values[&RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("b")
                ))]
            ) - 2.0)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn compile_uses_collected_semantic_runtime_deps() {
        let (tir, src) = tir_from_source(
            "node a: Dimensionless = 1.0;\n\
             node b: Dimensionless = @a + 1.0;",
        );
        let plan = compile(&tir, &src).unwrap();
        let a_pos = plan
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("a"),
                ))
            })
            .unwrap();
        let b_pos = plan
            .topo_order
            .iter()
            .position(|name| {
                name == &RuntimeDeclKey::resolved(ResolvedDeclName::from_def(
                    tir.root_dag_id().clone(),
                    DeclName::expect_valid("b"),
                ))
            })
            .unwrap();
        assert!(a_pos < b_pos);
    }

    // -----------------------------------------------------------------------
    // Domain constraints on const nodes (#441)
    // -----------------------------------------------------------------------

    #[test]
    fn const_domain_value_within_bounds_passes() {
        compile_source("const node MAX_M: Mass(min: 1.0 kg, max: 100.0 kg) = 50.0 kg;").unwrap();
    }

    #[test]
    fn const_domain_value_below_min_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_value_above_max_rejected() {
        let err = compile_source("const node X: Mass(max: 10.0 kg) = 50.0 kg;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_min_exceeds_max_rejected() {
        let err = compile_source("const node X: Mass(min: 100.0 kg, max: 50.0 kg) = 75.0 kg;")
            .unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainMinExceedsMax { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_invalid_target_rejected() {
        // `Bool` is not a valid constraint target; this should now fire on consts too.
        let err = compile_source("const node FLAG: Bool(min: 0.0) = true;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::InvalidDomainTarget { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_value_within_bounds() {
        compile_source("const node N: Int(min: 1, max: 100) = 5;").unwrap();
    }

    #[test]
    fn const_domain_int_value_out_of_bounds_rejected() {
        let err = compile_source("const node N: Int(min: 1, max: 10) = 100;").unwrap_err();
        assert!(
            matches!(err, GraphcalError::DomainViolation { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn const_domain_int_preserves_full_range_bounds() {
        compile_source(
            "const node MIN_I: Int(\
             min: -9223372036854775807 - 1, \
             max: 9223372036854775807) = -9223372036854775807 - 1;",
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_bounds_are_inclusive() {
        compile_source(
            r#"
const node START: Datetime<TT> = epoch<TT>("2024-01-01T00:00:00");
const node EVENT: Datetime<TT>(
    min: @START,
    max: epoch<TT>("2024-12-31T23:59:59"),
) = @START;
"#,
        )
        .unwrap();
    }

    #[test]
    fn const_datetime_domain_violation_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime(
    min: datetime("2024-01-01T00:00:00Z"),
    max: datetime("2024-12-31T23:59:59Z"),
) = datetime("2025-01-01T00:00:00Z");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainViolation { .. }));
    }

    #[test]
    fn datetime_domain_min_exceeds_max_is_rejected() {
        let error = compile_source(
            r#"
const node EVENT: Datetime<TT>(
    min: epoch<TT>("2025-01-01T00:00:00"),
    max: epoch<TT>("2024-01-01T00:00:00"),
) = epoch<TT>("2024-06-01T00:00:00");
"#,
        )
        .unwrap_err();
        assert!(matches!(error, GraphcalError::DomainMinExceedsMax { .. }));
    }
}

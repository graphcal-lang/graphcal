//! Constant pools are built in the checker's schedule and sealed with their
//! own checked TIR.

use std::collections::HashMap;
use std::sync::Arc;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::ir::imported_binding::{ImportedBinding, ImportedValueKind};
use graphcal_compiler::resolved_name::ResolvedDeclName;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::module_name::ScopedName;
use graphcal_compiler::tir::typed::CheckedTir;
use graphcal_eval::runtime_value::RuntimeValue;

use crate::project_compiler::compile_to_tir;
use graphcal_eval::checked_program::{
    CheckedProgram, EvaluatedTir, ExecutionFacts, ScheduledChecks, SealError,
    resolve_imported_constant,
};
use graphcal_eval::constant_pools::{ConstPool, ConstPoolBuildError, ConstantPoolError};
use graphcal_eval::runtime_presentation::PendingPresentedMap;

fn key(tir: &CheckedTir, name: &str) -> ResolvedDeclName {
    ResolvedDeclName::for_test(tir.root_dag_id().clone(), DeclName::expect_valid(name))
}

fn one() -> impl FnMut(
    graphcal_eval::constant_pools::ConstStep<'_>,
) -> Result<RuntimeValue, std::convert::Infallible> {
    |_| Ok(RuntimeValue::quantity(1.0).unwrap())
}

fn sealed(source: &str) -> CheckedProgram {
    let tir = compile_to_tir(source, "test.gcl").unwrap();
    let mut sources = graphcal_compiler::source_registry::SourceRegistry::new();
    let src = sources.register("test.gcl", Arc::new(source.to_owned()));
    graphcal_eval::execution_check::seal_checked_program_with_cancellation(
        tir,
        src,
        &sources,
        &graphcal_compiler::cancellation::CancellationToken::unbounded(),
    )
    .unwrap()
}

#[test]
fn const_pools_evaluate_every_constant_once_after_the_constants_it_reads() {
    let tir = compile_to_tir(
        "const node b: Dimensionless = @a + 1.0;\n\
         const node a: Dimensionless = 1.0;\n\
         const node c: Dimensionless = @b * 2.0;\n\
         param x: Dimensionless = 1.0;",
        "test.gcl",
    )
    .unwrap();
    let mut evaluated = Vec::new();
    let pool = ConstPool::build(&tir, &ConstPool::default(), |step| {
        assert!(std::ptr::eq(step.tir, &raw const tir));
        assert!(std::ptr::eq(
            step.expression.get(),
            tir.root().body_for_test().const_expr(step.key).unwrap()
        ));
        assert!(
            evaluated.iter().all(|done| step.visible.contains_key(done)),
            "every earlier constant is visible"
        );
        evaluated.push(step.key.clone());
        Ok::<_, std::convert::Infallible>(
            RuntimeValue::quantity(f64::from(u8::try_from(evaluated.len()).unwrap())).unwrap(),
        )
    })
    .unwrap();
    let names = evaluated
        .iter()
        .map(ResolvedDeclName::as_str)
        .collect::<Vec<_>>();
    assert_eq!(names, ["a", "b", "c"]);
    let root = pool.for_dag(tir.root_dag_id()).unwrap();
    assert_eq!(root.len(), 3);
    assert_eq!(pool.dags().collect::<Vec<_>>(), [tir.root_dag_id()]);

    let reference = pool.reference(&key(&tir, "c")).unwrap();
    assert_eq!(reference.key(), &key(&tir, "c"));
    assert!(std::ptr::eq(
        reference.value(),
        root.get(&key(&tir, "c")).unwrap()
    ));
    assert!(pool.reference(&key(&tir, "x")).is_none());
    assert!(ConstPool::default().reference(&key(&tir, "c")).is_none());
}

#[test]
fn const_pools_stop_at_the_first_failed_constant() {
    let tir = compile_to_tir(
        "const node a: Dimensionless = 1.0;\nconst node b: Dimensionless = @a + 1.0;",
        "test.gcl",
    )
    .unwrap();
    let mut calls = 0;
    let result = ConstPool::build(&tir, &ConstPool::default(), |_| {
        calls += 1;
        Err::<RuntimeValue, _>("failed")
    });
    assert!(matches!(
        result,
        Err(ConstPoolBuildError::Evaluation("failed"))
    ));
    assert_eq!(calls, 1);
}

#[test]
fn const_pools_reject_inherited_pools_for_scheduled_dags() {
    let tir = compile_to_tir("const node a: Dimensionless = 1.0;", "test.gcl").unwrap();
    let pool = ConstPool::build(&tir, &ConstPool::default(), one()).unwrap();
    assert!(matches!(
        ConstPool::build(&tir, &pool, one()),
        Err(ConstPoolBuildError::Invalid(ConstantPoolError::Rescheduled(dag))) if &dag == tir.root_dag_id()
    ));
}

#[test]
fn imported_constants_resolve_once_and_runtime_imports_resolve_to_none() {
    let tir = compile_to_tir(
        "const node C: Dimensionless = 2.0; param x: Dimensionless;",
        "test.gcl",
    )
    .unwrap();
    let evaluated = EvaluatedTir::evaluate(tir, &ExecutionFacts::default(), one()).unwrap();
    let (tir, consts) = (evaluated.tir(), evaluated.consts());
    let binding = |name: &str, kind| {
        let target = key(tir, name);
        ImportedBinding::new(
            target.clone(),
            tir.decl_type(&target).unwrap().declared().clone(),
            kind,
        )
    };
    let local = ScopedName::local(DeclName::expect_valid("local"));
    let constant = resolve_imported_constant(
        tir,
        consts,
        &local,
        &binding("C", ImportedValueKind::Constant),
    )
    .unwrap()
    .unwrap();
    assert_eq!(constant.name(), &local);
    assert_eq!(constant.value().key(), &key(tir, "C"));
    assert!(std::ptr::eq(
        constant.value().value(),
        consts
            .for_dag(tir.root_dag_id())
            .unwrap()
            .get(&key(tir, "C"))
            .unwrap()
    ));
    assert!(
        resolve_imported_constant(
            tir,
            consts,
            &local,
            &binding("x", ImportedValueKind::Runtime)
        )
        .unwrap()
        .is_none()
    );
    for wrong in [
        binding("C", ImportedValueKind::Runtime),
        binding("x", ImportedValueKind::Constant),
    ] {
        assert!(matches!(
            resolve_imported_constant(tir, consts, &local, &wrong),
            Err(SealError::WrongImportedKind { .. })
        ));
    }
    let absent = ImportedBinding::new(
        key(tir, "absent"),
        binding("C", ImportedValueKind::Constant)
            .declared_type()
            .clone(),
        ImportedValueKind::Constant,
    );
    assert!(matches!(
        resolve_imported_constant(tir, consts, &local, &absent),
        Err(SealError::MissingDeclaration(_))
    ));
    assert!(matches!(
        resolve_imported_constant(
            tir,
            &ConstPool::default(),
            &local,
            &binding("C", ImportedValueKind::Constant)
        ),
        Err(SealError::MissingConstant(_))
    ));
}

#[test]
fn sealing_pairs_every_dag_with_its_own_facts() {
    let program = sealed(
        "dag lib { const node K: Dimensionless = 3.0; pub node out: Dimensionless = @K; }\n\
         include lib() as inst;\n\
         const node lower: Dimensionless = 1.0;\n\
         param x: Dimensionless(min: @lower) = 2.0;\n\
         node result: Dimensionless = @inst::out + @x;",
    );
    let tir = program.tir();
    assert!(tir.dag_registry().len() > 1);
    let file_source = program.dag(tir.root_dag_id()).unwrap().source();
    for (dag_id, dag) in tir.dag_registry().iter() {
        let sealed = program.dag(dag_id).unwrap();
        assert!(std::ptr::eq(sealed.dag(), dag));
        assert_eq!(sealed.source(), file_source);
        assert_eq!(
            sealed.const_values().len(),
            dag.body_for_test().consts().count(),
            "every constant of `{dag_id}` is evaluated exactly once"
        );
    }
    let root = program.dag(tir.root_dag_id()).unwrap();
    assert!(root.domain_constraints().contains_key(&key(tir, "x")));
    let foreign = DagId::from_virtual_relative_path(std::path::Path::new("other.gcl")).unwrap();
    assert!(program.dag(&foreign).is_none());
    let root_id = tir.root_dag_id().clone();
    let (_, facts) = program.into_parts();
    assert_eq!(facts.source(&root_id), Some(file_source));
    assert!(facts.source(&foreign).is_none());
}

#[test]
fn sealing_requires_constraints_for_every_scheduled_dag() {
    let source = "const node C: Dimensionless = 2.0;";
    let tir = compile_to_tir(source, "test.gcl").unwrap();
    let evaluated = EvaluatedTir::evaluate(tir, &ExecutionFacts::default(), one()).unwrap();
    let result = evaluated.seal(ScheduledChecks {
        source: graphcal_compiler::source_registry::SourceRegistry::new()
            .register("test.gcl", Arc::new(source.to_owned())),
        const_presentations: PendingPresentedMap::new(),
        domain_constraints: HashMap::new(),
        struct_field_constraints: HashMap::new(),
    });
    assert!(matches!(
        result,
        Err(SealError::MissingDomainConstraints(_))
    ));
}

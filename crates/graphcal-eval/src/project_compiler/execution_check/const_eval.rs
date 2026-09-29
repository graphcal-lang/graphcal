//! Constant evaluation in the checker's constant schedule.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::tir::typed::CheckedTir;

use crate::eval_expr::{EvalContext, HirLocalValueMap, eval_hir_expr_with_presentation};
use crate::execution_facts::RuntimeValueMap;
use crate::presentation_evidence::PresentationInstanceMap;

pub(super) fn eval_const_pools_for_dags(
    tir: &CheckedTir,
    dag_ids: &HashSet<graphcal_compiler::dag_id::DagId>,
    mut visible_values: RuntimeValueMap,
    mut presentations: PresentationInstanceMap,
    src: &NamedSource<Arc<String>>,
    cancellation: &graphcal_compiler::cancellation::CancellationToken,
) -> Result<
    (
        HashMap<graphcal_compiler::dag_id::DagId, RuntimeValueMap>,
        PresentationInstanceMap,
    ),
    GraphcalError,
> {
    cancellation.checkpoint()?;
    let invalid =
        |message: String| GraphcalError::internal_error(message, src, DiagnosticAnchor::WholeFile);
    let schedule = tir
        .const_schedule()
        .ok_or_else(|| invalid("checked TIR has no constant schedule".to_owned()))?;
    if schedule.dags().len() != dag_ids.len()
        || !schedule
            .dags()
            .iter()
            .all(|dag_id| dag_ids.contains(dag_id))
    {
        return Err(invalid(
            "constant schedule does not cover exactly the DAGs being checked".to_owned(),
        ));
    }

    let empty_hir_locals = HirLocalValueMap::root();
    let mut const_pools = dag_ids
        .iter()
        .cloned()
        .map(|dag_id| (dag_id, RuntimeValueMap::new()))
        .collect::<HashMap<_, _>>();
    for key in schedule.order() {
        cancellation.checkpoint()?;
        let dag_id = key.owner();
        let dag = tir
            .dag_registry()
            .get(dag_id)
            .ok_or_else(|| invalid(format!("scheduled constant `{key}` has no checked DAG")))?;
        let ctx = EvalContext::provisional_constants(tir, dag.dag_id(), src, cancellation.clone())?
            .with_roots(&visible_values, None)
            .for_decl(key);
        let hir_expr = dag.const_expr(key).ok_or_else(|| {
            invalid(format!(
                "constant schedule references missing declaration `{key}`"
            ))
        })?;
        reject_constant_call(hir_expr, src)?;
        let (value, presentation) = eval_hir_expr_with_presentation(
            hir_expr,
            &visible_values,
            &presentations,
            &empty_hir_locals,
            &ctx,
        )?
        .into_parts();
        let runtime_key = key.clone();
        if !presentation.is_none() {
            presentations.insert(runtime_key.clone(), presentation);
        }
        visible_values.insert(runtime_key.clone(), value.clone());
        let pool = const_pools.get_mut(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked DAG `{dag_id}` has no initialized const pool"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        pool.insert(runtime_key, value);
    }
    Ok((const_pools, presentations))
}

fn reject_constant_call(
    expr: &graphcal_compiler::hir::expr::Expr,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match graphcal_compiler::hir::find_dag_call(expr) {
        Some((target, span)) => Err(GraphcalError::DagCallInCompileTime {
            name: target.to_string(),
            src: src.clone(),
            span: span.into(),
        }),
        None => Ok(()),
    }
}

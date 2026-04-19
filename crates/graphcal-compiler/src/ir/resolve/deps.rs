use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{classify_special_fn, is_aggregation_fn, is_time_scale_name};
use crate::syntax::ast::{Expr, ExprKind};
use crate::syntax::visitor::ExprVisitor;
use miette::NamedSource;

/// Extract const references from a const expression.
pub(super) fn extract_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<String>, GraphcalError> {
    let mut deps = HashSet::new();
    collect_const_refs(
        expr,
        all_const_names,
        builtin_consts,
        builtin_fns,
        user_fn_names,
        src,
        &mut deps,
    )?;
    Ok(deps)
}

#[expect(
    clippy::too_many_lines,
    reason = "recursive reference collector for all expression types"
)]
fn collect_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    deps: &mut HashSet<String>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. }
        | ExprKind::NameRef(_)
        | ExprKind::QualifiedNameRef { .. } => Ok(()),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            // In const expressions, @name can reference other const nodes but not runtime names.
            // Runtime refs are already rejected by check_no_runtime_graph_refs before we get here.
            if all_const_names.contains(ident.value.as_str()) {
                deps.insert(ident.value.to_string());
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => {
            // Bare UPPER_SNAKE_CASE identifiers: built-in constants only.
            if builtin_consts.contains_key(ident.value.as_str())
                || is_time_scale_name(ident.value.as_str())
            {
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                collect_const_refs(
                    &binding.value,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    deps,
                )?;
            }
            Ok(())
        }
        ExprKind::FnCall { name, args, .. } => {
            let name_str = name.value.as_str();
            if !builtin_fns.contains_key(name_str)
                && !user_fn_names.contains(name_str)
                && classify_special_fn(name_str).is_none()
            {
                return Err(GraphcalError::UnknownFunction {
                    name: name.value.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            // Only check arity for builtins (user fn arity checked later in dim_check).
            // Skip arity check for aggregation/conversion functions.
            if let Some(builtin) = builtin_fns.get(name_str)
                && args.len() != builtin.arity()
                && !is_aggregation_fn(name_str)
            {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
                    expected: builtin.arity(),
                    got: args.len(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            for arg in args {
                collect_const_refs(
                    arg,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    deps,
                )?;
            }
            Ok(())
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_const_refs(
                lhs,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                rhs,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
        ExprKind::UnaryOp { operand, .. } => collect_const_refs(
            operand,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            deps,
        ),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_const_refs(
                condition,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                then_branch,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                else_branch,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => collect_const_refs(
            inner,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            deps,
        ),
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            collect_const_refs(
                expr,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_const_refs(
                        val,
                        all_const_names,
                        builtin_consts,
                        builtin_fns,
                        user_fn_names,
                        src,
                        deps,
                    )?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                collect_const_refs(
                    &entry.value,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    deps,
                )?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => collect_const_refs(
            body,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            deps,
        ),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_const_refs(
                source,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                init,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                body,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_const_refs(
                init,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            collect_const_refs(
                body,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_const_refs(
                scrutinee,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )?;
            for arm in arms {
                collect_const_refs(
                    &arm.body,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    deps,
                )?;
            }
            Ok(())
        }
        // TupleMatch is desugared before resolution.
        #[expect(clippy::unreachable, reason = "invariant: desugared before resolution")]
        ExprKind::TupleMatch { .. } => {
            unreachable!("TupleMatch should be desugared before resolution")
        }
    }
}

/// Extract all graph and const references from an expression.
///
/// When `self_name` is `Some` and the expression is an `Unfold`, the self-reference
/// is excluded from the returned `graph_refs`. Unfold self-references (e.g.
/// `@my_node[prev]`) are temporal — they access the previous iteration, not a
/// true cyclic dependency.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through resolution context; self_name adds one beyond the existing set"
)]
pub(super) fn extract_all_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    self_name: Option<&str>,
) -> Result<(HashSet<String>, HashSet<String>), GraphcalError> {
    let mut graph_refs = HashSet::new();
    let mut const_refs = HashSet::new();
    collect_all_refs(
        expr,
        all_runtime_names,
        all_const_names,
        builtin_consts,
        builtin_fns,
        user_fn_names,
        src,
        &mut graph_refs,
        &mut const_refs,
    )?;
    // Unfold self-references (@self[prev_i]) are not true cyclic dependencies —
    // they access the previous step. Remove the self-edge so the DAG stays acyclic.
    if let Some(name) = self_name
        && matches!(expr.kind, ExprKind::Unfold { .. })
    {
        graph_refs.remove(name);
    }
    Ok((graph_refs, const_refs))
}

#[expect(
    clippy::too_many_arguments,
    reason = "passes through resolution context to recursive calls"
)]
#[expect(
    clippy::too_many_lines,
    reason = "recursive reference collector for all expression types"
)]
fn collect_all_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    graph_refs: &mut HashSet<String>,
    const_refs: &mut HashSet<String>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. }
        | ExprKind::NameRef(_)
        | ExprKind::QualifiedNameRef { .. } => Ok(()),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            if all_runtime_names.contains(ident.value.as_str()) {
                graph_refs.insert(ident.value.to_string());
                Ok(())
            } else if all_const_names.contains(ident.value.as_str()) {
                // @const_node_name in a node expression — track as const dependency
                const_refs.insert(ident.value.to_string());
                Ok(())
            } else {
                Err(GraphcalError::UnknownGraphRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => {
            // Bare UPPER_SNAKE_CASE: built-in constants only.
            if builtin_consts.contains_key(ident.value.as_str())
                || is_time_scale_name(ident.value.as_str())
            {
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::InlineDagRef { args, .. } => {
            for binding in args {
                collect_all_refs(
                    &binding.value,
                    all_runtime_names,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    graph_refs,
                    const_refs,
                )?;
            }
            Ok(())
        }
        ExprKind::FnCall { name, args, .. } => {
            let name_str = name.value.as_str();
            if !builtin_fns.contains_key(name_str)
                && !user_fn_names.contains(name_str)
                && classify_special_fn(name_str).is_none()
            {
                return Err(GraphcalError::UnknownFunction {
                    name: name.value.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            if let Some(builtin) = builtin_fns.get(name_str)
                && args.len() != builtin.arity()
                && !is_aggregation_fn(name_str)
            {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
                    expected: builtin.arity(),
                    got: args.len(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            for arg in args {
                collect_all_refs(
                    arg,
                    all_runtime_names,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    graph_refs,
                    const_refs,
                )?;
            }
            Ok(())
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_all_refs(
                lhs,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                rhs,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )
        }
        ExprKind::UnaryOp { operand, .. } => collect_all_refs(
            operand,
            all_runtime_names,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            graph_refs,
            const_refs,
        ),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_all_refs(
                condition,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                then_branch,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                else_branch,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )
        }
        ExprKind::Convert { expr: inner, .. }
        | ExprKind::DisplayTimezone { expr: inner, .. }
        | ExprKind::AsCast { expr: inner, .. } => collect_all_refs(
            inner,
            all_runtime_names,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            graph_refs,
            const_refs,
        ),
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            collect_all_refs(
                expr,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_all_refs(
                        val,
                        all_runtime_names,
                        all_const_names,
                        builtin_consts,
                        builtin_fns,
                        user_fn_names,
                        src,
                        graph_refs,
                        const_refs,
                    )?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                collect_all_refs(
                    &entry.value,
                    all_runtime_names,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    graph_refs,
                    const_refs,
                )?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => collect_all_refs(
            body,
            all_runtime_names,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            graph_refs,
            const_refs,
        ),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_all_refs(
                source,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                init,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                body,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_all_refs(
                init,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            collect_all_refs(
                body,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_all_refs(
                scrutinee,
                all_runtime_names,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                graph_refs,
                const_refs,
            )?;
            for arm in arms {
                collect_all_refs(
                    &arm.body,
                    all_runtime_names,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    graph_refs,
                    const_refs,
                )?;
            }
            Ok(())
        }
        // TupleMatch is desugared before resolution.
        #[expect(clippy::unreachable, reason = "invariant: desugared before resolution")]
        ExprKind::TupleMatch { .. } => {
            unreachable!("TupleMatch should be desugared before resolution")
        }
    }
}

/// Visitor that collects known graph references.
struct KnownGraphRefCollector<'a> {
    all_runtime_names: &'a HashSet<&'a str>,
    refs: &'a mut HashSet<String>,
}

impl ExprVisitor for KnownGraphRefCollector<'_> {
    type Error = std::convert::Infallible;

    fn visit_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::GraphRef(ident) = &expr.kind
            && self.all_runtime_names.contains(ident.value.as_str())
        {
            self.refs.insert(ident.value.to_string());
        }
        Ok(())
    }

    fn visit_qualified_graph_ref(&mut self, expr: &Expr) -> Result<(), Self::Error> {
        if let ExprKind::QualifiedGraphRef { name: ident, .. } = &expr.kind
            && self.all_runtime_names.contains(ident.value.as_str())
        {
            self.refs.insert(ident.value.to_string());
        }
        Ok(())
    }
}

/// Collect `@`-references (graph refs) from an expression.
///
/// This is a lightweight version of `collect_all_refs` used for re-extracting
/// runtime dependencies after an override expression replaces a param's default.
/// Only collects names that exist in `all_runtime_names`.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API always uses default hasher"
)]
pub fn collect_graph_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    refs: &mut HashSet<String>,
) {
    let mut collector = KnownGraphRefCollector {
        all_runtime_names,
        refs,
    };
    let _ = collector.visit_expr(expr);
}

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{DeclKind, Expr, ExprKind, File, FnBody, FnDecl};
use graphcal_syntax::span::Span;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::error::GraphcalError;

/// Aggregation functions recognized as special forms (not registered as builtins).
const AGGREGATION_FNS: &[&str] = &["sum", "min", "max", "mean", "count"];
const CONVERSION_FNS: &[&str] = &["to_float", "to_int"];

/// Declarations imported from other files, to be injected into the resolve scope.
///
/// These are treated as if they were declared locally, appearing before local declarations.
#[derive(Debug, Default)]
pub struct ImportedNames {
    pub consts: Vec<(String, Expr, Span)>,
    pub params: Vec<(String, Expr, Span)>,
    pub nodes: Vec<(String, Expr, Span)>,
    pub functions: Vec<(String, FnDecl, Span)>,
}

/// The kind of a declaration (used for source-order tracking).
#[derive(Debug, Clone, Copy)]
pub enum DeclCategory {
    Const,
    Param,
    Node,
}

/// The result of name resolution: declarations separated by category with dependency info.
#[derive(Debug)]
pub struct ResolvedFile {
    /// Const declarations in source order: (name, expr, span).
    pub consts: Vec<(String, Expr, Span)>,
    /// Param declarations in source order: (name, expr, span).
    pub params: Vec<(String, Expr, Span)>,
    /// Node declarations in source order: (name, expr, span).
    pub nodes: Vec<(String, Expr, Span)>,
    /// For each node/param, the set of `@`-references (graph deps).
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of `CONST_REF` references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
}

/// Resolve names, check casing, detect duplicates, and extract dependencies.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, casing
/// violations, or arity mismatches are found.
pub fn resolve(file: &File, src: &NamedSource<Arc<String>>) -> Result<ResolvedFile, GraphcalError> {
    resolve_with_imports(file, src, &ImportedNames::default())
}

/// Resolve names with imported declarations injected into scope.
///
/// Imported declarations are prepended to the local declarations, so they appear
/// first in eval order. The downstream pipeline (`dim_check`, `const_eval`, DAG, evaluate)
/// works without changes because imported params/nodes become part of the DAG.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, casing
/// violations, or arity mismatches are found.
#[expect(
    clippy::too_many_lines,
    reason = "complex resolution logic with multiple passes"
)]
pub fn resolve_with_imports(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<ResolvedFile, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<String, Span> = HashMap::new();
    let mut consts = Vec::new();
    let mut params = Vec::new();
    let mut nodes = Vec::new();
    let mut functions = Vec::new();
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();
    let mut user_fn_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    for (name, _, span) in &imported.consts {
        names.insert(name.clone(), *span);
    }
    for (name, _, span) in &imported.params {
        names.insert(name.clone(), *span);
    }
    for (name, _, span) in &imported.nodes {
        names.insert(name.clone(), *span);
    }
    for (name, _, span) in &imported.functions {
        names.insert(name.clone(), *span);
        user_fn_names.insert(name.clone());
    }

    // First pass: collect all declarations and check for duplicates + casing
    for decl in &file.declarations {
        // Dimension and Unit declarations are handled by the registry, not the resolver
        let (name, name_span, is_const) = match &decl.kind {
            DeclKind::Param(p) => (p.name.name.clone(), p.name.span, false),
            DeclKind::Node(n) => (n.name.name.clone(), n.name.span, false),
            DeclKind::Const(c) => (c.name.name.clone(), c.name.span, true),
            DeclKind::Fn(f) => {
                // Check fn name for duplicates (same namespace as param/node/const)
                if let Some(first_span) = names.get(&f.name.name) {
                    return Err(GraphcalError::DuplicateName {
                        name: f.name.name.clone(),
                        src: src.clone(),
                        duplicate: f.name.span.into(),
                        first: (*first_span).into(),
                    });
                }
                names.insert(f.name.name.clone(), f.name.span);
                user_fn_names.insert(f.name.name.clone());
                continue;
            }
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Use(_) => {
                continue;
            }
        };

        // Check for duplicates
        if let Some(first_span) = names.get(&name) {
            return Err(GraphcalError::DuplicateName {
                name,
                src: src.clone(),
                duplicate: name_span.into(),
                first: (*first_span).into(),
            });
        }
        names.insert(name.clone(), name_span);

        // Track source order
        let category = match &decl.kind {
            DeclKind::Const(_) => DeclCategory::Const,
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Fn(_)
            | DeclKind::Index(_)
            | DeclKind::Use(_) => {
                unreachable!()
            }
        };
        source_order.push((name.clone(), category));

        // Check casing (defensive -- parser should enforce this already)
        #[expect(
            clippy::else_if_without_else,
            reason = "no action needed in the else case"
        )]
        if is_const {
            if !is_upper_snake_case(&name) {
                return Err(GraphcalError::EvalError {
                    message: format!("const name `{name}` must be UPPER_SNAKE_CASE"),
                    src: src.clone(),
                    span: name_span.into(),
                });
            }
        } else if !is_lower_snake_case(&name) {
            return Err(GraphcalError::EvalError {
                message: format!("param/node name `{name}` must be lower_snake_case"),
                src: src.clone(),
                span: name_span.into(),
            });
        }
    }

    // Build the set of all known names for reference checking
    let all_const_names: HashSet<&str> = names
        .keys()
        .filter(|n| is_upper_snake_case(n))
        .map(String::as_str)
        .collect();
    let all_runtime_names: HashSet<&str> = names
        .keys()
        .filter(|n| is_lower_snake_case(n))
        .map(String::as_str)
        .collect();
    // Second pass: resolve references and extract dependencies
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Use(_) => {}
            DeclKind::Fn(f) => {
                // Enforce @ prohibition in function bodies
                check_no_graph_refs_in_fn(f, src)?;
                functions.push((f.name.name.clone(), f.clone(), decl.span));
            }
            DeclKind::Const(c) => {
                check_no_graph_refs(&c.value, src)?;
                let deps = extract_const_refs(
                    &c.value,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                const_deps.insert(c.name.name.clone(), deps);
                consts.push((c.name.name.clone(), c.value.clone(), decl.span));
            }
            DeclKind::Param(p) => {
                let (graph_refs, _const_refs) = extract_all_refs(
                    &p.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                runtime_deps.insert(p.name.name.clone(), graph_refs);
                params.push((p.name.name.clone(), p.value.clone(), decl.span));
            }
            DeclKind::Node(n) => {
                let (graph_refs, _const_refs) = extract_all_refs(
                    &n.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                runtime_deps.insert(n.name.name.clone(), graph_refs);
                nodes.push((n.name.name.clone(), n.value.clone(), decl.span));
            }
        }
    }

    // Prepend imported declarations so they appear before local ones in eval order.
    let mut all_consts = imported.consts.clone();
    all_consts.extend(consts);
    let mut all_params = imported.params.clone();
    all_params.extend(params);
    let mut all_nodes = imported.nodes.clone();
    all_nodes.extend(nodes);
    let mut all_functions = imported.functions.clone();
    all_functions.extend(functions);

    // Prepend imported source_order entries
    let mut all_source_order: Vec<(String, DeclCategory)> = Vec::new();
    for (name, _, _) in &imported.consts {
        all_source_order.push((name.clone(), DeclCategory::Const));
    }
    for (name, _, _) in &imported.params {
        all_source_order.push((name.clone(), DeclCategory::Param));
    }
    for (name, _, _) in &imported.nodes {
        all_source_order.push((name.clone(), DeclCategory::Node));
    }
    all_source_order.extend(source_order);

    Ok(ResolvedFile {
        consts: all_consts,
        params: all_params,
        nodes: all_nodes,
        runtime_deps,
        const_deps,
        source_order: all_source_order,
        functions: all_functions,
    })
}

fn is_upper_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_lower_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Check that an expression contains no `@` references (for const expressions).
fn check_no_graph_refs(expr: &Expr, src: &NamedSource<Arc<String>>) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) => Err(GraphcalError::GraphRefInConst {
            name: ident.name.clone(),
            src: src.clone(),
            span: expr.span.into(),
        }),
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::ConstRef(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs(lhs, src)?;
            check_no_graph_refs(rhs, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs(operand, src),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                check_no_graph_refs(arg, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_graph_refs(condition, src)?;
            check_no_graph_refs(then_branch, src)?;
            check_no_graph_refs(else_branch, src)
        }
        ExprKind::Convert { expr: inner, .. } => check_no_graph_refs(inner, src),
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_graph_refs(&stmt.value, src)?;
            }
            check_no_graph_refs(expr, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_graph_refs(expr, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs(val, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                check_no_graph_refs(&entry.value, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_graph_refs(body, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_graph_refs(source, src)?;
            check_no_graph_refs(init, src)?;
            check_no_graph_refs(body, src)
        }
    }
}

/// Check that a function body contains no `@` references (purity enforcement).
fn check_no_graph_refs_in_fn(
    fn_decl: &FnDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let check = |expr: &Expr| -> Result<(), GraphcalError> {
        check_no_graph_refs_in_fn_expr(expr, &fn_decl.name.name, src)
    };
    match &fn_decl.body {
        FnBody::Short(expr) => check(expr),
        FnBody::Block { stmts, expr } => {
            for stmt in stmts {
                check(&stmt.value)?;
            }
            check(expr)
        }
    }
}

fn check_no_graph_refs_in_fn_expr(
    expr: &Expr,
    fn_name: &str,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) => Err(GraphcalError::GraphRefInFn {
            name: ident.name.clone(),
            src: src.clone(),
            span: expr.span.into(),
            help: format!("pass `{fn_name}` as a function parameter instead"),
        }),
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::ConstRef(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs_in_fn_expr(lhs, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(rhs, fn_name, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs_in_fn_expr(operand, fn_name, src),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                check_no_graph_refs_in_fn_expr(arg, fn_name, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_graph_refs_in_fn_expr(condition, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(then_branch, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(else_branch, fn_name, src)
        }
        ExprKind::Convert { expr: inner, .. } => {
            check_no_graph_refs_in_fn_expr(inner, fn_name, src)
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_graph_refs_in_fn_expr(&stmt.value, fn_name, src)?;
            }
            check_no_graph_refs_in_fn_expr(expr, fn_name, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_graph_refs_in_fn_expr(expr, fn_name, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs_in_fn_expr(val, fn_name, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                check_no_graph_refs_in_fn_expr(&entry.value, fn_name, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_graph_refs_in_fn_expr(body, fn_name, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_graph_refs_in_fn_expr(source, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(init, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(body, fn_name, src)
        }
    }
}

/// Extract const references from a const expression.
fn extract_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
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
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    deps: &mut HashSet<String>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => Ok(()),
        ExprKind::GraphRef(_) => unreachable!("should be caught by check_no_graph_refs"),
        ExprKind::ConstRef(ident) => {
            if builtin_consts.contains_key(ident.name.as_str()) {
                Ok(())
            } else if all_const_names.contains(ident.name.as_str()) {
                deps.insert(ident.name.clone());
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::FnCall { name, args } => {
            if !builtin_fns.contains_key(name.name.as_str())
                && !user_fn_names.contains(&name.name)
                && !AGGREGATION_FNS.contains(&name.name.as_str())
                && !CONVERSION_FNS.contains(&name.name.as_str())
            {
                return Err(GraphcalError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            // Only check arity for builtins (user fn arity checked later in dim_check).
            // Skip arity check for aggregation/conversion functions.
            if let Some(builtin) = builtin_fns.get(name.name.as_str())
                && args.len() != builtin.arity
                && !AGGREGATION_FNS.contains(&name.name.as_str())
            {
                return Err(GraphcalError::WrongArity {
                    name: name.name.clone(),
                    expected: builtin.arity,
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
        ExprKind::Convert { expr: inner, .. } => collect_const_refs(
            inner,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            deps,
        ),
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_const_refs(
                    &stmt.value,
                    all_const_names,
                    builtin_consts,
                    builtin_fns,
                    user_fn_names,
                    src,
                    deps,
                )?;
            }
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
        ExprKind::MapLiteral { entries } => {
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
    }
}

/// Extract graph refs and const refs from a runtime expression (param/node value).
fn extract_all_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
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
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    graph_refs: &mut HashSet<String>,
    const_refs: &mut HashSet<String>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => Ok(()),
        ExprKind::GraphRef(ident) => {
            if all_runtime_names.contains(ident.name.as_str()) {
                graph_refs.insert(ident.name.clone());
                Ok(())
            } else {
                Err(GraphcalError::UnknownGraphRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::ConstRef(ident) => {
            if builtin_consts.contains_key(ident.name.as_str()) {
                Ok(())
            } else if all_const_names.contains(ident.name.as_str()) {
                const_refs.insert(ident.name.clone());
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::FnCall { name, args } => {
            if !builtin_fns.contains_key(name.name.as_str())
                && !user_fn_names.contains(&name.name)
                && !AGGREGATION_FNS.contains(&name.name.as_str())
                && !CONVERSION_FNS.contains(&name.name.as_str())
            {
                return Err(GraphcalError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            if let Some(builtin) = builtin_fns.get(name.name.as_str())
                && args.len() != builtin.arity
                && !AGGREGATION_FNS.contains(&name.name.as_str())
            {
                return Err(GraphcalError::WrongArity {
                    name: name.name.clone(),
                    expected: builtin.arity,
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
        ExprKind::Convert { expr: inner, .. } => collect_all_refs(
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
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_all_refs(
                    &stmt.value,
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
        ExprKind::MapLiteral { entries } => {
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
    }
}

/// Collect `@`-references (graph refs) from an expression.
///
/// This is a lightweight version of `collect_all_refs` used for re-extracting
/// runtime dependencies after an override expression replaces a param's default.
/// Only collects names that exist in `all_runtime_names`.
pub fn collect_graph_refs(
    expr: &Expr,
    all_runtime_names: &HashSet<&str>,
    refs: &mut HashSet<String>,
) {
    match &expr.kind {
        ExprKind::GraphRef(ident) => {
            if all_runtime_names.contains(ident.name.as_str()) {
                refs.insert(ident.name.clone());
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_graph_refs(lhs, all_runtime_names, refs);
            collect_graph_refs(rhs, all_runtime_names, refs);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_graph_refs(operand, all_runtime_names, refs);
        }
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_graph_refs(arg, all_runtime_names, refs);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_graph_refs(condition, all_runtime_names, refs);
            collect_graph_refs(then_branch, all_runtime_names, refs);
            collect_graph_refs(else_branch, all_runtime_names, refs);
        }
        ExprKind::Convert { expr: inner, .. } => {
            collect_graph_refs(inner, all_runtime_names, refs);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_graph_refs(&stmt.value, all_runtime_names, refs);
            }
            collect_graph_refs(expr, all_runtime_names, refs);
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            collect_graph_refs(expr, all_runtime_names, refs);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_graph_refs(val, all_runtime_names, refs);
                }
            }
        }
        ExprKind::MapLiteral { entries } => {
            for entry in entries {
                collect_graph_refs(&entry.value, all_runtime_names, refs);
            }
        }
        ExprKind::ForComp { body, .. } => {
            collect_graph_refs(body, all_runtime_names, refs);
        }
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_graph_refs(source, all_runtime_names, refs);
            collect_graph_refs(init, all_runtime_names, refs);
            collect_graph_refs(body, all_runtime_names, refs);
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_) => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use graphcal_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_and_resolve(source: &str) -> Result<ResolvedFile, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        resolve(&file, &make_src(source))
    }

    #[test]
    fn resolve_rocket_ksr() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = resolve(&file, &make_src(source)).unwrap();
        assert_eq!(resolved.consts.len(), 1);
        assert_eq!(resolved.params.len(), 3);
        assert_eq!(resolved.nodes.len(), 3);
    }

    #[test]
    fn resolve_constants_ksr() {
        let source = include_str!("../../../tests/fixtures/constants.gcl");
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = resolve(&file, &make_src(source)).unwrap();
        assert_eq!(resolved.consts.len(), 4);
        assert_eq!(resolved.params.len(), 1);
        assert_eq!(resolved.nodes.len(), 2);
    }

    #[test]
    fn resolve_duplicate_name() {
        let err = parse_and_resolve("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_unknown_graph_ref() {
        let err = parse_and_resolve("node x: Dimensionless = @nonexistent + 1.0;").unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
    }

    #[test]
    fn resolve_unknown_const_ref() {
        let err = parse_and_resolve("node x: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownConstRef { .. }));
    }

    #[test]
    fn resolve_at_in_const() {
        let err = parse_and_resolve(
            "param p: Dimensionless = 1.0;\nconst BAD: Dimensionless = @p * 2.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::GraphRefInConst { .. }));
    }

    #[test]
    fn parser_rejects_bad_const_casing() {
        let result = Parser::new("const bad_name: Dimensionless = 42.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parser_rejects_bad_param_casing() {
        let result = Parser::new("param BAD: Dimensionless = 42.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn resolve_builtin_const_recognized() {
        let resolved = parse_and_resolve("node x: Dimensionless = PI * 2.0;").unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_builtin_function_recognized() {
        let resolved =
            parse_and_resolve("param x: Dimensionless = 4.0;\nnode y: Dimensionless = sqrt(@x);")
                .unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_unknown_function() {
        let err = parse_and_resolve("node x: Dimensionless = unknown_fn(1.0);").unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
    }

    #[test]
    fn resolve_wrong_arity() {
        let err = parse_and_resolve("node x: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
        assert!(matches!(err, GraphcalError::WrongArity { .. }));
    }

    #[test]
    fn resolve_const_deps_extracted() {
        let resolved =
            parse_and_resolve("const A: Dimensionless = 1.0;\nconst B: Dimensionless = A + 2.0;")
                .unwrap();
        let b_deps = &resolved.const_deps["B"];
        assert!(b_deps.contains("A"));
        assert_eq!(b_deps.len(), 1);
    }

    #[test]
    fn resolve_runtime_deps_extracted() {
        let resolved =
            parse_and_resolve("param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;").unwrap();
        let c_deps = &resolved.runtime_deps["c"];
        assert!(c_deps.contains("a"));
        assert!(c_deps.contains("b"));
        assert_eq!(c_deps.len(), 2);
    }

    // Phase 3: function resolution tests

    #[test]
    fn resolve_fn_collected() {
        let source = r"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            param val: Dimensionless = 1.0;
            node result: Dimensionless = double(@val);
        ";
        let resolved = parse_and_resolve(source).unwrap();
        assert_eq!(resolved.functions.len(), 1);
        assert_eq!(resolved.functions[0].0, "double");
    }

    #[test]
    fn resolve_fn_duplicate_name_with_param() {
        let source = r"
            param x: Dimensionless = 1.0;
            fn x(a: Dimensionless) -> Dimensionless = a;
        ";
        let err = parse_and_resolve(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_fn_duplicate_name_with_const() {
        let source = r"
            const X: Dimensionless = 1.0;
            fn X(a: Dimensionless) -> Dimensionless = a;
        ";
        // This should fail at parse time (fn name must be lower_snake_case)
        let result = Parser::new(source).parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn resolve_at_in_fn_body() {
        let source = r"
            param val: Dimensionless = 1.0;
            fn bad(x: Dimensionless) -> Dimensionless = x + @val;
        ";
        let err = parse_and_resolve(source).unwrap_err();
        assert!(matches!(err, GraphcalError::GraphRefInFn { .. }));
    }

    #[test]
    fn resolve_user_fn_call_in_node() {
        let source = r"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            param val: Dimensionless = 5.0;
            node result: Dimensionless = double(@val);
        ";
        let resolved = parse_and_resolve(source).unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_user_fn_call_in_const() {
        let source = r"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            const FOUR: Dimensionless = double(2.0);
        ";
        let resolved = parse_and_resolve(source).unwrap();
        assert_eq!(resolved.consts.len(), 1);
    }

    #[test]
    fn resolve_fn_not_in_source_order() {
        let source = r"
            fn double(x: Dimensionless) -> Dimensionless = x * 2.0;
            param val: Dimensionless = 5.0;
            node result: Dimensionless = double(@val);
        ";
        let resolved = parse_and_resolve(source).unwrap();
        // Functions should NOT appear in source_order
        assert_eq!(resolved.source_order.len(), 2); // param + node only
    }

    // --- Additional error path tests ---

    #[test]
    fn resolve_duplicate_param_name() {
        let err = parse_and_resolve("param x: Dimensionless = 1.0;\nparam x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_duplicate_const_name() {
        let err = parse_and_resolve("const A: Dimensionless = 1.0;\nconst A: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_duplicate_node_name() {
        let err = parse_and_resolve(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;\nnode y: Dimensionless = @x + 1.0;",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_const_collision_with_param() {
        // const uses UPPER, param uses lower — no collision
        let resolved =
            parse_and_resolve("const A: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;")
                .unwrap();
        assert_eq!(resolved.consts.len(), 1);
        assert_eq!(resolved.params.len(), 1);
    }

    #[test]
    fn resolve_unknown_const_ref_in_const() {
        let err = parse_and_resolve("const A: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownConstRef { .. }));
    }

    #[test]
    fn resolve_unknown_function_in_const() {
        let err = parse_and_resolve("const A: Dimensionless = unknown_fn(1.0);").unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
    }

    #[test]
    fn resolve_wrong_arity_in_const() {
        let err = parse_and_resolve("const A: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
        assert!(matches!(err, GraphcalError::WrongArity { .. }));
    }

    #[test]
    fn resolve_unknown_graph_ref_in_node() {
        let err =
            parse_and_resolve("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @z + 1.0;")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownGraphRef { .. }));
    }

    #[test]
    fn resolve_unknown_function_in_node() {
        let err =
            parse_and_resolve("param x: Dimensionless = 1.0;\nnode y: Dimensionless = bad_fn(@x);")
                .unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownFunction { .. }));
    }

    #[test]
    fn resolve_wrong_arity_in_node() {
        let err = parse_and_resolve(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = sqrt(@x, @x);",
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::WrongArity { .. }));
    }

    #[test]
    fn resolve_const_with_block_expr() {
        let resolved =
            parse_and_resolve("const A: Dimensionless = { let x = 1.0; let y = 2.0; x + y };")
                .unwrap();
        assert_eq!(resolved.consts.len(), 1);
        let a_deps = &resolved.const_deps["A"];
        assert!(a_deps.is_empty());
    }

    #[test]
    fn resolve_const_with_if_else() {
        let resolved =
            parse_and_resolve("const A: Dimensionless = if 1.0 > 0.0 { 1.0 } else { 0.0 };")
                .unwrap();
        assert_eq!(resolved.consts.len(), 1);
    }

    #[test]
    fn resolve_const_with_unary_op() {
        let resolved = parse_and_resolve("const A: Dimensionless = -42.0;").unwrap();
        assert_eq!(resolved.consts.len(), 1);
    }

    #[test]
    fn resolve_node_with_block() {
        let resolved = parse_and_resolve(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = { let a = @x; a + 1.0 };",
        )
        .unwrap();
        assert_eq!(resolved.nodes.len(), 1);
        let y_deps = &resolved.runtime_deps["y"];
        assert!(y_deps.contains("x"));
    }

    #[test]
    fn resolve_node_with_struct() {
        let resolved = parse_and_resolve(
            r"
            type Pair { a: Dimensionless, b: Dimensionless }
            param x: Dimensionless = 1.0;
            node p: Pair = Pair { a: @x, b: @x + 1.0 };
        ",
        )
        .unwrap();
        assert_eq!(resolved.nodes.len(), 1);
        let p_deps = &resolved.runtime_deps["p"];
        assert!(p_deps.contains("x"));
    }

    #[test]
    fn resolve_node_with_field_access() {
        let resolved = parse_and_resolve(
            r"
            type Pair { a: Dimensionless, b: Dimensionless }
            param x: Dimensionless = 1.0;
            node p: Pair = Pair { a: @x, b: @x + 1.0 };
            node val: Dimensionless = @p.a;
        ",
        )
        .unwrap();
        assert_eq!(resolved.nodes.len(), 2);
    }

    #[test]
    fn resolve_node_with_convert() {
        let resolved =
            parse_and_resolve("param x: Length = 1000.0 m;\nnode y: Length = @x -> km;").unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_use_decl_skipped() {
        // use declarations should not be treated as param/node/const
        let source = r#"use "./helper.gcl" { something };"#;
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = resolve(&file, &make_src(source)).unwrap();
        assert!(resolved.params.is_empty());
        assert!(resolved.nodes.is_empty());
        assert!(resolved.consts.is_empty());
    }

    #[test]
    fn resolve_indexed_param() {
        let resolved = parse_and_resolve(
            r"
            index Color = { Red, Green, Blue }
            param values: Dimensionless[Color] = {
                Color::Red: 1.0,
                Color::Green: 2.0,
                Color::Blue: 3.0,
            };
        ",
        )
        .unwrap();
        assert_eq!(resolved.params.len(), 1);
    }

    #[test]
    fn resolve_for_comprehension() {
        let resolved = parse_and_resolve(
            r"
            index Color = { Red, Green, Blue }
            param values: Dimensionless[Color] = {
                Color::Red: 1.0,
                Color::Green: 2.0,
                Color::Blue: 3.0,
            };
            node doubled: Dimensionless[Color] = for c: Color { @values[c] * 2.0 };
        ",
        )
        .unwrap();
        assert_eq!(resolved.nodes.len(), 1);
        let deps = &resolved.runtime_deps["doubled"];
        assert!(deps.contains("values"));
    }

    #[test]
    fn resolve_scan_expression() {
        let resolved = parse_and_resolve(
            r"
            index Step = { First, Second, Third }
            param vals: Dimensionless[Step] = {
                Step::First: 1.0,
                Step::Second: 2.0,
                Step::Third: 3.0,
            };
            node cumul: Dimensionless[Step] = scan(@vals, 0.0, |acc, val| acc + val);
        ",
        )
        .unwrap();
        assert_eq!(resolved.nodes.len(), 1);
        let deps = &resolved.runtime_deps["cumul"];
        assert!(deps.contains("vals"));
    }

    #[test]
    fn resolve_fn_with_block_body() {
        let resolved = parse_and_resolve(
            r"
            fn compute(x: Dimensionless) -> Dimensionless {
                let a = x * 2.0;
                let b = a + 1.0;
                b
            }
            param val: Dimensionless = 5.0;
            node result: Dimensionless = compute(@val);
        ",
        )
        .unwrap();
        assert_eq!(resolved.functions.len(), 1);
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_duplicate_fn_name() {
        let source = r"
            fn foo(x: Dimensionless) -> Dimensionless = x;
            fn foo(x: Dimensionless) -> Dimensionless = x * 2.0;
        ";
        let err = parse_and_resolve(source).unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }
}

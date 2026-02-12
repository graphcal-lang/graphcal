use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::{DeclKind, Expr, ExprKind, File, FnBody, FnDecl};
use kasuri_syntax::span::Span;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::error::KasuriError;

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
/// Returns a [`KasuriError`] if duplicate names, unknown references, casing
/// violations, or arity mismatches are found.
#[expect(clippy::too_many_lines)] // Complex resolution logic with multiple passes
pub fn resolve(file: &File, src: &NamedSource<Arc<String>>) -> Result<ResolvedFile, KasuriError> {
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
                    return Err(KasuriError::DuplicateName {
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
            DeclKind::Dimension(_) | DeclKind::Unit(_) | DeclKind::Type(_) => {
                continue;
            }
        };

        // Check for duplicates
        if let Some(first_span) = names.get(&name) {
            return Err(KasuriError::DuplicateName {
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
            DeclKind::Dimension(_) | DeclKind::Unit(_) | DeclKind::Type(_) | DeclKind::Fn(_) => {
                unreachable!()
            }
        };
        source_order.push((name.clone(), category));

        // Check casing (defensive -- parser should enforce this already)
        #[expect(clippy::else_if_without_else)] // No action needed in the else case
        if is_const {
            if !is_upper_snake_case(&name) {
                return Err(KasuriError::EvalError {
                    message: format!("const name `{name}` must be UPPER_SNAKE_CASE"),
                    src: src.clone(),
                    span: name_span.into(),
                });
            }
        } else if !is_lower_snake_case(&name) {
            return Err(KasuriError::EvalError {
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
            DeclKind::Dimension(_) | DeclKind::Unit(_) | DeclKind::Type(_) => {}
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

    Ok(ResolvedFile {
        consts,
        params,
        nodes,
        runtime_deps,
        const_deps,
        source_order,
        functions,
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
fn check_no_graph_refs(expr: &Expr, src: &NamedSource<Arc<String>>) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) => Err(KasuriError::GraphRefInConst {
            name: ident.name.clone(),
            src: src.clone(),
            span: expr.span.into(),
        }),
        ExprKind::Number(_)
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
        ExprKind::FieldAccess { expr, .. } => check_no_graph_refs(expr, src),
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs(val, src)?;
                }
            }
            Ok(())
        }
    }
}

/// Check that a function body contains no `@` references (purity enforcement).
fn check_no_graph_refs_in_fn(
    fn_decl: &FnDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), KasuriError> {
    let check = |expr: &Expr| -> Result<(), KasuriError> {
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
) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) => Err(KasuriError::GraphRefInFn {
            name: ident.name.clone(),
            src: src.clone(),
            span: expr.span.into(),
            help: format!("pass `{fn_name}` as a function parameter instead"),
        }),
        ExprKind::Number(_)
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
        ExprKind::FieldAccess { expr, .. } => check_no_graph_refs_in_fn_expr(expr, fn_name, src),
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_graph_refs_in_fn_expr(val, fn_name, src)?;
                }
            }
            Ok(())
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
) -> Result<HashSet<String>, KasuriError> {
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

#[expect(clippy::too_many_lines)]
fn collect_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    user_fn_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
    deps: &mut HashSet<String>,
) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::Number(_)
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
                Err(KasuriError::UnknownConstRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::FnCall { name, args } => {
            if !builtin_fns.contains_key(name.name.as_str()) && !user_fn_names.contains(&name.name)
            {
                return Err(KasuriError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            // Only check arity for builtins (user fn arity checked later in dim_check)
            if let Some(builtin) = builtin_fns.get(name.name.as_str())
                && args.len() != builtin.arity
            {
                return Err(KasuriError::WrongArity {
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
        ExprKind::FieldAccess { expr, .. } => collect_const_refs(
            expr,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            deps,
        ),
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
) -> Result<(HashSet<String>, HashSet<String>), KasuriError> {
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

#[expect(clippy::too_many_arguments)]
#[expect(clippy::too_many_lines)] // Recursive reference collector for all expression types
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
) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::Number(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_) => Ok(()),
        ExprKind::GraphRef(ident) => {
            if all_runtime_names.contains(ident.name.as_str()) {
                graph_refs.insert(ident.name.clone());
                Ok(())
            } else {
                Err(KasuriError::UnknownGraphRef {
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
                Err(KasuriError::UnknownConstRef {
                    name: ident.name.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::FnCall { name, args } => {
            if !builtin_fns.contains_key(name.name.as_str()) && !user_fn_names.contains(&name.name)
            {
                return Err(KasuriError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            if let Some(builtin) = builtin_fns.get(name.name.as_str())
                && args.len() != builtin.arity
            {
                return Err(KasuriError::WrongArity {
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
        ExprKind::FieldAccess { expr, .. } => collect_all_refs(
            expr,
            all_runtime_names,
            all_const_names,
            builtin_consts,
            builtin_fns,
            user_fn_names,
            src,
            graph_refs,
            const_refs,
        ),
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
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use kasuri_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_and_resolve(source: &str) -> Result<ResolvedFile, KasuriError> {
        let file = Parser::new(source).parse_file().unwrap();
        resolve(&file, &make_src(source))
    }

    #[test]
    fn resolve_rocket_ksr() {
        let source = include_str!("../../../tests/fixtures/rocket.ksr");
        let file = Parser::new(source).parse_file().unwrap();
        let resolved = resolve(&file, &make_src(source)).unwrap();
        assert_eq!(resolved.consts.len(), 1);
        assert_eq!(resolved.params.len(), 3);
        assert_eq!(resolved.nodes.len(), 3);
    }

    #[test]
    fn resolve_constants_ksr() {
        let source = include_str!("../../../tests/fixtures/constants.ksr");
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
        assert!(matches!(err, KasuriError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_unknown_graph_ref() {
        let err = parse_and_resolve("node x: Dimensionless = @nonexistent + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::UnknownGraphRef { .. }));
    }

    #[test]
    fn resolve_unknown_const_ref() {
        let err = parse_and_resolve("node x: Dimensionless = NONEXISTENT + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::UnknownConstRef { .. }));
    }

    #[test]
    fn resolve_at_in_const() {
        let err = parse_and_resolve(
            "param p: Dimensionless = 1.0;\nconst BAD: Dimensionless = @p * 2.0;",
        )
        .unwrap_err();
        assert!(matches!(err, KasuriError::GraphRefInConst { .. }));
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
        assert!(matches!(err, KasuriError::UnknownFunction { .. }));
    }

    #[test]
    fn resolve_wrong_arity() {
        let err = parse_and_resolve("node x: Dimensionless = sqrt(1.0, 2.0);").unwrap_err();
        assert!(matches!(err, KasuriError::WrongArity { .. }));
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
        assert!(matches!(err, KasuriError::DuplicateName { .. }));
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
        assert!(matches!(err, KasuriError::GraphRefInFn { .. }));
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
}

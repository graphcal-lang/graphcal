use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use kasuri_syntax::ast::{DeclKind, Expr, ExprKind, File};
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
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();

    // First pass: collect all declarations and check for duplicates + casing
    for decl in &file.declarations {
        let (name, name_span, is_const) = match &decl.kind {
            DeclKind::Param(p) => (p.name.name.clone(), p.name.span, false),
            DeclKind::Node(n) => (n.name.name.clone(), n.name.span, false),
            DeclKind::Const(c) => (c.name.name.clone(), c.name.span, true),
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
            DeclKind::Const(c) => {
                check_no_graph_refs(&c.value, src)?;
                let deps = extract_const_refs(
                    &c.value,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
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
        ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::ConstRef(_) => Ok(()),
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
    }
}

/// Extract const references from a const expression.
fn extract_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
) -> Result<HashSet<String>, KasuriError> {
    let mut deps = HashSet::new();
    collect_const_refs(
        expr,
        all_const_names,
        builtin_consts,
        builtin_fns,
        src,
        &mut deps,
    )?;
    Ok(deps)
}

fn collect_const_refs(
    expr: &Expr,
    all_const_names: &HashSet<&str>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    src: &NamedSource<Arc<String>>,
    deps: &mut HashSet<String>,
) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Bool(_) => Ok(()),
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
            if !builtin_fns.contains_key(name.name.as_str()) {
                return Err(KasuriError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            let expected_arity = builtin_fns[name.name.as_str()].arity;
            if args.len() != expected_arity {
                return Err(KasuriError::WrongArity {
                    name: name.name.clone(),
                    expected: expected_arity,
                    got: args.len(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            for arg in args {
                collect_const_refs(arg, all_const_names, builtin_consts, builtin_fns, src, deps)?;
            }
            Ok(())
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_const_refs(lhs, all_const_names, builtin_consts, builtin_fns, src, deps)?;
            collect_const_refs(rhs, all_const_names, builtin_consts, builtin_fns, src, deps)
        }
        ExprKind::UnaryOp { operand, .. } => collect_const_refs(
            operand,
            all_const_names,
            builtin_consts,
            builtin_fns,
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
                src,
                deps,
            )?;
            collect_const_refs(
                then_branch,
                all_const_names,
                builtin_consts,
                builtin_fns,
                src,
                deps,
            )?;
            collect_const_refs(
                else_branch,
                all_const_names,
                builtin_consts,
                builtin_fns,
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
    src: &NamedSource<Arc<String>>,
    graph_refs: &mut HashSet<String>,
    const_refs: &mut HashSet<String>,
) -> Result<(), KasuriError> {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::Bool(_) => Ok(()),
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
            if !builtin_fns.contains_key(name.name.as_str()) {
                return Err(KasuriError::UnknownFunction {
                    name: name.name.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            let expected_arity = builtin_fns[name.name.as_str()].arity;
            if args.len() != expected_arity {
                return Err(KasuriError::WrongArity {
                    name: name.name.clone(),
                    expected: expected_arity,
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
                src,
                graph_refs,
                const_refs,
            )
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
        let err = parse_and_resolve("param x = 1.0;\nnode x = 2.0;").unwrap_err();
        assert!(matches!(err, KasuriError::DuplicateName { .. }));
    }

    #[test]
    fn resolve_unknown_graph_ref() {
        let err = parse_and_resolve("node x = @nonexistent + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::UnknownGraphRef { .. }));
    }

    #[test]
    fn resolve_unknown_const_ref() {
        let err = parse_and_resolve("node x = NONEXISTENT + 1.0;").unwrap_err();
        assert!(matches!(err, KasuriError::UnknownConstRef { .. }));
    }

    #[test]
    fn resolve_at_in_const() {
        let err = parse_and_resolve("param p = 1.0;\nconst BAD = @p * 2.0;").unwrap_err();
        assert!(matches!(err, KasuriError::GraphRefInConst { .. }));
    }

    #[test]
    fn parser_rejects_bad_const_casing() {
        let result = Parser::new("const bad_name = 42.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn parser_rejects_bad_param_casing() {
        let result = Parser::new("param BAD = 42.0;").parse_file();
        assert!(result.is_err());
    }

    #[test]
    fn resolve_builtin_const_recognized() {
        let resolved = parse_and_resolve("node x = PI * 2.0;").unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_builtin_function_recognized() {
        let resolved = parse_and_resolve("param x = 4.0;\nnode y = sqrt(@x);").unwrap();
        assert_eq!(resolved.nodes.len(), 1);
    }

    #[test]
    fn resolve_unknown_function() {
        let err = parse_and_resolve("node x = unknown_fn(1.0);").unwrap_err();
        assert!(matches!(err, KasuriError::UnknownFunction { .. }));
    }

    #[test]
    fn resolve_wrong_arity() {
        let err = parse_and_resolve("node x = sqrt(1.0, 2.0);").unwrap_err();
        assert!(matches!(err, KasuriError::WrongArity { .. }));
    }

    #[test]
    fn resolve_const_deps_extracted() {
        let resolved = parse_and_resolve("const A = 1.0;\nconst B = A + 2.0;").unwrap();
        let b_deps = &resolved.const_deps["B"];
        assert!(b_deps.contains("A"));
        assert_eq!(b_deps.len(), 1);
    }

    #[test]
    fn resolve_runtime_deps_extracted() {
        let resolved =
            parse_and_resolve("param a = 1.0;\nparam b = 2.0;\nnode c = @a + @b;").unwrap();
        let c_deps = &resolved.runtime_deps["c"];
        assert!(c_deps.contains("a"));
        assert!(c_deps.contains("b"));
        assert_eq!(c_deps.len(), 2);
    }
}

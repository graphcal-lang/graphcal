mod deps;
mod names;
mod scope;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::TypeExpr;
use graphcal_syntax::ast::{AssertBody, DeclKind, Expr, ExprKind, File, FnDecl};
use graphcal_syntax::span::Span;

use graphcal_registry::builtins::{builtin_constants, builtin_functions};
use graphcal_registry::error::GraphcalError;
use graphcal_registry::resolve_types::{
    ResolvedAssertEntry, ResolvedConstEntry, ResolvedFigureEntry, ResolvedFunctionEntry,
    ResolvedNodeEntry, ResolvedParamEntry, ResolvedPlotEntry,
};

// Re-export types and constants from graphcal-registry's resolve_types module.
pub use graphcal_registry::resolve_types::{
    DATETIME_EXTRACT_FNS, DATETIME_FROM_FNS, DATETIME_TO_FNS, DeclCategory, ExpectedFail,
    ExpectedFailKey, ImportedValueNames, ResolvedFile, ScopedName, is_aggregation_fn,
    is_constructor_fn, is_conversion_fn, is_datetime_extract_fn, is_datetime_from_fn,
    is_datetime_to_fn, is_time_scale_name,
};

// Re-export public items from submodules.
pub use deps::collect_graph_refs;
pub use names::{is_lower_snake_case, is_upper_snake_case};

// Import helpers from submodules for use within this file.
use deps::{extract_all_refs, extract_const_refs};
use names::parse_expected_fail_args;
use scope::{check_no_assert_graph_refs, check_no_graph_refs, check_no_graph_refs_in_fn};

/// Declarations imported from other files, to be injected into the resolve scope.
///
/// These are treated as if they were declared locally, appearing before local declarations.
#[derive(Debug, Default)]
pub struct ImportedNames {
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    pub params: Vec<(String, TypeExpr, Expr, Span)>,
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
    pub functions: Vec<(String, FnDecl, Span)>,
    pub asserts: Vec<(String, AssertBody, Span)>,
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
    let mut asserts = Vec::new();
    let mut plots = Vec::new();
    let mut figures = Vec::new();
    let mut functions = Vec::new();
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();
    let mut user_fn_names: HashSet<String> = HashSet::new();
    let mut assert_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    for (name, _, _, span) in &imported.consts {
        names.insert(name.clone(), *span);
    }
    for (name, _, _, span) in &imported.params {
        names.insert(name.clone(), *span);
    }
    for (name, _, _, span) in &imported.nodes {
        names.insert(name.clone(), *span);
    }
    for (name, _, span) in &imported.functions {
        names.insert(name.clone(), *span);
        user_fn_names.insert(name.clone());
    }
    for (name, _, span) in &imported.asserts {
        names.insert(name.clone(), *span);
        assert_names.insert(name.clone());
    }

    // First pass: collect all declarations and check for duplicates + casing
    for decl in &file.declarations {
        // Dimension and Unit declarations are handled by the registry, not the resolver
        let (name, name_span, is_const) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Node(n) => (n.name.value.to_string(), n.name.span, false),
            DeclKind::Const(c) => (c.name.value.to_string(), c.name.span, true),
            DeclKind::Assert(a) => (a.name.value.to_string(), a.name.span, false),
            DeclKind::Plot(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Figure(f) => (f.name.value.to_string(), f.name.span, false),
            DeclKind::Fn(f) => {
                let fn_name_str = f.name.value.to_string();
                // Check fn name for duplicates (same namespace as param/node/const)
                if let Some(first_span) = names.get(&fn_name_str) {
                    return Err(GraphcalError::DuplicateName {
                        name: fn_name_str,
                        src: src.clone(),
                        duplicate: f.name.span.into(),
                        first: (*first_span).into(),
                    });
                }
                names.insert(fn_name_str.clone(), f.name.span);
                user_fn_names.insert(fn_name_str);
                continue;
            }
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_) => {
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

        // Track source order and assert names
        let category = match &decl.kind {
            DeclKind::Const(_) => DeclCategory::Const,
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Assert(_) => {
                assert_names.insert(name.clone());
                DeclCategory::Assert
            }
            DeclKind::Plot(_) => DeclCategory::Plot,
            DeclKind::Figure(_) => DeclCategory::Figure,
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Fn(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_) => {
                // These declarations are handled earlier (continue'd before reaching here).
                continue;
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

    // Build the set of all known names for reference checking.
    // Module-qualified names (e.g. "constants::G0", "params::dry_mass") are classified
    // by the casing of the part after "::" so they land in the right set.
    let all_const_names: HashSet<&str> = names
        .keys()
        .filter(|n| {
            if let Some((_module, member)) = n.split_once("::") {
                is_upper_snake_case(member)
            } else {
                is_upper_snake_case(n)
            }
        })
        .map(String::as_str)
        .collect();
    let all_runtime_names: HashSet<&str> = names
        .keys()
        .filter(|n| {
            if let Some((_module, member)) = n.split_once("::") {
                is_lower_snake_case(member)
            } else {
                is_lower_snake_case(n)
            }
        })
        .map(String::as_str)
        .collect();
    // Second pass: resolve references and extract dependencies
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_) => {}
            DeclKind::Assert(a) => {
                // Collect all expressions from the assert body for validation
                let body_exprs: Vec<&Expr> = match &a.body {
                    AssertBody::Expr(expr) => vec![expr],
                    AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => vec![actual, expected, tolerance],
                };
                for body_expr in &body_exprs {
                    // Validate references in assert body (asserts CAN use @)
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        body_expr,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    // Check that assert body doesn't reference other assert names via @
                    check_no_assert_graph_refs(body_expr, &assert_names, src)?;
                }
                let aname = a.name.value.to_string();
                asserts.push(ResolvedAssertEntry {
                    name: aname,
                    body: a.body.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Plot(p) => {
                // Validate references in plot field expressions (plots CAN use @)
                for field in &p.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                }
                let pname = p.name.value.to_string();
                let hidden = decl.attributes.iter().any(|a| a.name.name == "hidden");
                plots.push(ResolvedPlotEntry {
                    name: pname,
                    decl: p.clone(),
                    span: decl.span,
                    hidden,
                });
            }
            DeclKind::Figure(f) => {
                // Validate references in figure field expressions (figures CAN use @)
                for field in &f.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                }
                let fname = f.name.value.to_string();
                figures.push(ResolvedFigureEntry {
                    name: fname,
                    decl: f.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Fn(f) => {
                // Enforce @ prohibition in function bodies
                check_no_graph_refs_in_fn(f, src)?;
                functions.push(ResolvedFunctionEntry {
                    name: f.name.value.to_string(),
                    decl: f.clone(),
                    span: decl.span,
                });
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
                let cname = c.name.value.to_string();
                const_deps.insert(cname.clone(), deps);
                consts.push(ResolvedConstEntry {
                    name: cname,
                    expr: c.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Param(p) => {
                let pname = p.name.value.to_string();
                if let Some(ref value) = p.value {
                    check_no_assert_graph_refs(value, &assert_names, src)?;
                    let (graph_refs, _const_refs) = extract_all_refs(
                        value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    runtime_deps.insert(pname.clone(), graph_refs);
                } else {
                    runtime_deps.insert(pname.clone(), HashSet::new());
                }
                params.push(ResolvedParamEntry {
                    name: pname,
                    default_expr: p.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Node(n) => {
                check_no_assert_graph_refs(&n.value, &assert_names, src)?;
                let (mut graph_refs, _const_refs) = extract_all_refs(
                    &n.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                let nname = n.name.value.to_string();
                // Unfold self-references (@self[prev_i]) are not true
                // cyclic dependencies — they access the previous step.
                // Remove the self-edge so the DAG stays acyclic.
                if matches!(n.value.kind, ExprKind::Unfold { .. }) {
                    graph_refs.remove(&nname);
                }
                runtime_deps.insert(nname.clone(), graph_refs);
                nodes.push(ResolvedNodeEntry {
                    name: nname,
                    expr: n.value.clone(),
                    span: decl.span,
                });
            }
        }
    }

    // Extract dependencies for imported declarations so the DAG is complete.
    // Without this, imported nodes' @-references are invisible to the topological sort,
    // causing evaluation-order errors (Bug 2).
    for (name, _, expr, _) in &imported.consts {
        let deps = extract_const_refs(
            expr,
            &all_const_names,
            &builtin_consts,
            &builtin_fns,
            &user_fn_names,
            src,
        )?;
        const_deps.insert(name.clone(), deps);
    }
    for (name, _, expr, _) in &imported.params {
        let (graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            &builtin_consts,
            &builtin_fns,
            &user_fn_names,
            src,
        )?;
        runtime_deps.insert(name.clone(), graph_refs);
    }
    for (name, _, expr, _) in &imported.nodes {
        let (mut graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            &builtin_consts,
            &builtin_fns,
            &user_fn_names,
            src,
        )?;
        if matches!(expr.kind, ExprKind::Unfold { .. }) {
            graph_refs.remove(name);
        }
        runtime_deps.insert(name.clone(), graph_refs);
    }

    // Prepend imported declarations so they appear before local ones in eval order.
    // Strip TypeExpr from imported tuples and convert to entry types.
    let mut all_consts: Vec<ResolvedConstEntry> = imported
        .consts
        .iter()
        .map(|(name, _, expr, span)| ResolvedConstEntry {
            name: name.clone(),
            expr: expr.clone(),
            span: *span,
        })
        .collect();
    all_consts.extend(consts);
    let mut all_params: Vec<ResolvedParamEntry> = imported
        .params
        .iter()
        .map(|(name, _, expr, span)| ResolvedParamEntry {
            name: name.clone(),
            default_expr: Some(expr.clone()),
            span: *span,
        })
        .collect();
    all_params.extend(params);
    let mut all_nodes: Vec<ResolvedNodeEntry> = imported
        .nodes
        .iter()
        .map(|(name, _, expr, span)| ResolvedNodeEntry {
            name: name.clone(),
            expr: expr.clone(),
            span: *span,
        })
        .collect();
    all_nodes.extend(nodes);
    let mut all_functions: Vec<ResolvedFunctionEntry> = imported
        .functions
        .iter()
        .map(|(name, decl, span)| ResolvedFunctionEntry {
            name: name.clone(),
            decl: decl.clone(),
            span: *span,
        })
        .collect();
    all_functions.extend(functions);
    let mut all_asserts: Vec<ResolvedAssertEntry> = imported
        .asserts
        .iter()
        .map(|(name, body, span)| ResolvedAssertEntry {
            name: name.clone(),
            body: body.clone(),
            span: *span,
        })
        .collect();
    all_asserts.extend(asserts);

    // Prepend imported source_order entries
    let mut all_source_order: Vec<(String, DeclCategory)> = Vec::new();
    for (name, _, _, _) in &imported.consts {
        all_source_order.push((name.clone(), DeclCategory::Const));
    }
    for (name, _, _, _) in &imported.params {
        all_source_order.push((name.clone(), DeclCategory::Param));
    }
    for (name, _, _, _) in &imported.nodes {
        all_source_order.push((name.clone(), DeclCategory::Node));
    }
    for (name, _, _) in &imported.asserts {
        all_source_order.push((name.clone(), DeclCategory::Assert));
    }
    all_source_order.extend(source_order);

    // Validate attributes and build assumes_map / expected_fail_map
    let mut assumes_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut expected_fail_map: HashMap<String, ExpectedFail> = HashMap::new();
    for decl in &file.declarations {
        let decl_name = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.to_string()),
            DeclKind::Node(n) => Some(n.name.value.to_string()),
            DeclKind::Const(c) => Some(c.name.value.to_string()),
            DeclKind::Assert(a) => Some(a.name.value.to_string()),
            DeclKind::Plot(p) => Some(p.name.value.to_string()),
            DeclKind::Figure(f) => Some(f.name.value.to_string()),
            DeclKind::Fn(f) => Some(f.name.value.to_string()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name = attr.name.name.as_str();
            match attr_name {
                "hidden" => {
                    // #[hidden] is only valid on plot declarations
                    let kind = match &decl.kind {
                        DeclKind::Plot(_) => continue,
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Assert(_) => "assert",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                        DeclKind::Import(_) => "import",
                    };
                    return Err(GraphcalError::InvalidAttributeTarget {
                        attr_name: "hidden".to_string(),
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                "assumes" => {
                    // #[assumes] is only valid on node and param
                    let kind = match &decl.kind {
                        DeclKind::Param(_) | DeclKind::Node(_) => None,
                        DeclKind::Const(_) => Some("const"),
                        DeclKind::Assert(_) => Some("assert"),
                        DeclKind::Plot(_) => Some("plot"),
                        DeclKind::Figure(_) => Some("figure"),
                        DeclKind::Fn(_) => Some("fn"),
                        DeclKind::Dimension(_) => Some("dimension"),
                        DeclKind::Unit(_) => Some("unit"),
                        DeclKind::Type(_) => Some("type"),
                        DeclKind::Index(_) => Some("index"),
                        DeclKind::Import(_) => Some("import"),
                    };
                    if let Some(kind) = kind {
                        return Err(GraphcalError::InvalidAssumesTarget {
                            kind: kind.to_string(),
                            src: src.clone(),
                            span: attr.span.into(),
                        });
                    }
                    // Each argument must reference an existing assert declaration
                    for arg in &attr.args {
                        let ident =
                            arg.as_single_ident()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message:
                                        "`#[assumes(...)]` arguments must be plain identifiers"
                                            .to_string(),
                                    src: src.clone(),
                                    span: arg.span().into(),
                                })?;
                        let arg_name = ident.name.as_str();
                        if !assert_names.contains(arg_name) {
                            return Err(GraphcalError::UnknownAssertInAssumes {
                                name: arg_name.to_string(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                        if let Some(ref dname) = decl_name {
                            assumes_map
                                .entry(arg_name.to_string())
                                .or_default()
                                .push(dname.clone());
                        }
                    }
                }
                "expected_fail" => {
                    let kind = match &decl.kind {
                        DeclKind::Assert(a) => {
                            // Valid target — parse args and record
                            let ef = parse_expected_fail_args(&attr.args, src)?;
                            // #[expected_fail] (no args) on an indexed assertion is
                            // an error — the user must specify which variants fail.
                            if matches!(ef, ExpectedFail::All) {
                                let is_indexed = matches!(
                                    &a.body,
                                    AssertBody::Expr(expr) if matches!(expr.kind, ExprKind::ForComp { .. })
                                );
                                if is_indexed {
                                    return Err(GraphcalError::ExpectedFailAllOnIndexed {
                                        src: src.clone(),
                                        span: attr.span.into(),
                                    });
                                }
                            }
                            if let Some(ref dname) = decl_name {
                                expected_fail_map.insert(dname.clone(), ef);
                            }
                            continue;
                        }
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                        DeclKind::Import(_) => "import",
                    };
                    return Err(GraphcalError::InvalidExpectedFailTarget {
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                "lazy" => {
                    // Recognized but semantics deferred — no validation needed
                }
                "allow_defaults" => {
                    // #[allow_defaults] is only valid on import declarations
                    let kind = match &decl.kind {
                        DeclKind::Import(_) => continue,
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Assert(_) => "assert",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                    };
                    return Err(GraphcalError::InvalidAttributeTarget {
                        attr_name: "allow_defaults".to_string(),
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                _ => {
                    return Err(GraphcalError::UnknownAttribute {
                        name: attr_name.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
            }
        }
    }

    Ok(ResolvedFile {
        consts: all_consts,
        params: all_params,
        nodes: all_nodes,
        asserts: all_asserts,
        plots,
        figures,
        runtime_deps,
        const_deps,
        source_order: all_source_order,
        functions: all_functions,
        assert_names,
        assumes_map,
        expected_fail: expected_fail_map,
    })
}

/// Resolve names with pre-evaluated imported value names in scope.
///
/// Unlike [`resolve_with_imports`], this does **not** inject imported expressions
/// into the DAG. Imported names are only used for scope checking (so that
/// references to imported values are recognized as valid). The actual values
/// are injected later via the execution plan.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, casing
/// violations, or arity mismatches are found.
#[expect(
    clippy::too_many_lines,
    reason = "complex resolution logic with multiple passes"
)]
pub fn resolve_with_imported_values(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedValueNames,
) -> Result<ResolvedFile, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<String, Span> = HashMap::new();
    let mut consts = Vec::new();
    let mut params = Vec::new();
    let mut nodes = Vec::new();
    let mut asserts = Vec::new();
    let mut plots = Vec::new();
    let mut figures = Vec::new();
    let mut functions = Vec::new();
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();
    let mut user_fn_names: HashSet<String> = HashSet::new();
    let mut assert_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (for scope checking only).
    // ScopedName -> String conversion: the resolver's internal scope uses flat strings
    // because it mixes imported names with local declarations.
    for (name, span) in &imported.const_names {
        names.insert(name.to_string(), *span);
    }
    for (name, span) in &imported.param_names {
        names.insert(name.to_string(), *span);
    }
    for (name, span) in &imported.node_names {
        names.insert(name.to_string(), *span);
    }
    for entry in &imported.functions {
        names.insert(entry.name.clone(), entry.span);
        user_fn_names.insert(entry.name.clone());
    }
    for (name, span) in &imported.assert_names {
        names.insert(name.clone(), *span);
        assert_names.insert(name.clone());
    }

    // First pass: collect all declarations and check for duplicates + casing
    for decl in &file.declarations {
        let (name, name_span, is_const) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Node(n) => (n.name.value.to_string(), n.name.span, false),
            DeclKind::Const(c) => (c.name.value.to_string(), c.name.span, true),
            DeclKind::Assert(a) => (a.name.value.to_string(), a.name.span, false),
            DeclKind::Plot(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Figure(f) => (f.name.value.to_string(), f.name.span, false),
            DeclKind::Fn(f) => {
                let fn_name_str = f.name.value.to_string();
                if let Some(first_span) = names.get(&fn_name_str) {
                    return Err(GraphcalError::DuplicateName {
                        name: fn_name_str,
                        src: src.clone(),
                        duplicate: f.name.span.into(),
                        first: (*first_span).into(),
                    });
                }
                names.insert(fn_name_str.clone(), f.name.span);
                user_fn_names.insert(fn_name_str);
                continue;
            }
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_) => {
                continue;
            }
        };

        if let Some(first_span) = names.get(&name) {
            return Err(GraphcalError::DuplicateName {
                name,
                src: src.clone(),
                duplicate: name_span.into(),
                first: (*first_span).into(),
            });
        }
        names.insert(name.clone(), name_span);

        let category = match &decl.kind {
            DeclKind::Const(_) => DeclCategory::Const,
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Assert(_) => {
                assert_names.insert(name.clone());
                DeclCategory::Assert
            }
            DeclKind::Plot(_) => DeclCategory::Plot,
            DeclKind::Figure(_) => DeclCategory::Figure,
            _ => continue,
        };
        source_order.push((name.clone(), category));

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

    // Build known name sets (including imported names for scope checking).
    let all_const_names: HashSet<&str> = names
        .keys()
        .filter(|n| {
            if let Some((_module, member)) = n.split_once("::") {
                is_upper_snake_case(member)
            } else {
                is_upper_snake_case(n)
            }
        })
        .map(String::as_str)
        .collect();
    let all_runtime_names: HashSet<&str> = names
        .keys()
        .filter(|n| {
            if let Some((_module, member)) = n.split_once("::") {
                is_lower_snake_case(member)
            } else {
                is_lower_snake_case(n)
            }
        })
        .map(String::as_str)
        .collect();

    // Second pass: resolve references and extract dependencies (local only).
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_) => {}
            DeclKind::Assert(a) => {
                let body_exprs: Vec<&Expr> = match &a.body {
                    AssertBody::Expr(expr) => vec![expr],
                    AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => vec![actual, expected, tolerance],
                };
                for body_expr in &body_exprs {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        body_expr,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    check_no_assert_graph_refs(body_expr, &assert_names, src)?;
                }
                let aname = a.name.value.to_string();
                asserts.push(ResolvedAssertEntry {
                    name: aname,
                    body: a.body.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Plot(p) => {
                for field in &p.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                }
                let pname = p.name.value.to_string();
                let hidden = decl.attributes.iter().any(|a| a.name.name == "hidden");
                plots.push(ResolvedPlotEntry {
                    name: pname,
                    decl: p.clone(),
                    span: decl.span,
                    hidden,
                });
            }
            DeclKind::Figure(f) => {
                for field in &f.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                }
                let fname = f.name.value.to_string();
                figures.push(ResolvedFigureEntry {
                    name: fname,
                    decl: f.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Fn(f) => {
                check_no_graph_refs_in_fn(f, src)?;
                functions.push(ResolvedFunctionEntry {
                    name: f.name.value.to_string(),
                    decl: f.clone(),
                    span: decl.span,
                });
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
                let cname = c.name.value.to_string();
                const_deps.insert(cname.clone(), deps);
                consts.push(ResolvedConstEntry {
                    name: cname,
                    expr: c.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Param(p) => {
                let pname = p.name.value.to_string();
                if let Some(ref value) = p.value {
                    check_no_assert_graph_refs(value, &assert_names, src)?;
                    let (graph_refs, _const_refs) = extract_all_refs(
                        value,
                        &all_runtime_names,
                        &all_const_names,
                        &builtin_consts,
                        &builtin_fns,
                        &user_fn_names,
                        src,
                    )?;
                    runtime_deps.insert(pname.clone(), graph_refs);
                } else {
                    runtime_deps.insert(pname.clone(), HashSet::new());
                }
                params.push(ResolvedParamEntry {
                    name: pname,
                    default_expr: p.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Node(n) => {
                check_no_assert_graph_refs(&n.value, &assert_names, src)?;
                let (mut graph_refs, _const_refs) = extract_all_refs(
                    &n.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                let nname = n.name.value.to_string();
                if matches!(n.value.kind, ExprKind::Unfold { .. }) {
                    graph_refs.remove(&nname);
                }
                runtime_deps.insert(nname.clone(), graph_refs);
                nodes.push(ResolvedNodeEntry {
                    name: nname,
                    expr: n.value.clone(),
                    span: decl.span,
                });
            }
        }
    }

    // Imported functions still need to be in the resolved output for IR compilation.
    let mut all_functions = imported.functions.clone();
    all_functions.extend(functions);

    // Validate attributes and build assumes_map / expected_fail_map
    let mut assumes_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut expected_fail_map: HashMap<String, ExpectedFail> = HashMap::new();
    for decl in &file.declarations {
        let decl_name = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.to_string()),
            DeclKind::Node(n) => Some(n.name.value.to_string()),
            DeclKind::Const(c) => Some(c.name.value.to_string()),
            DeclKind::Assert(a) => Some(a.name.value.to_string()),
            DeclKind::Plot(p) => Some(p.name.value.to_string()),
            DeclKind::Figure(f) => Some(f.name.value.to_string()),
            DeclKind::Fn(f) => Some(f.name.value.to_string()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name = attr.name.name.as_str();
            match attr_name {
                "hidden" => {
                    // #[hidden] is only valid on plot declarations
                    let kind = match &decl.kind {
                        DeclKind::Plot(_) => continue,
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Assert(_) => "assert",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                        DeclKind::Import(_) => "import",
                    };
                    return Err(GraphcalError::InvalidAttributeTarget {
                        attr_name: "hidden".to_string(),
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                "assumes" => {
                    let kind = match &decl.kind {
                        DeclKind::Param(_) | DeclKind::Node(_) => None,
                        DeclKind::Const(_) => Some("const"),
                        DeclKind::Assert(_) => Some("assert"),
                        DeclKind::Plot(_) => Some("plot"),
                        DeclKind::Figure(_) => Some("figure"),
                        DeclKind::Fn(_) => Some("fn"),
                        DeclKind::Dimension(_) => Some("dimension"),
                        DeclKind::Unit(_) => Some("unit"),
                        DeclKind::Type(_) => Some("type"),
                        DeclKind::Index(_) => Some("index"),
                        DeclKind::Import(_) => Some("import"),
                    };
                    if let Some(kind) = kind {
                        return Err(GraphcalError::InvalidAssumesTarget {
                            kind: kind.to_string(),
                            src: src.clone(),
                            span: attr.span.into(),
                        });
                    }
                    for arg in &attr.args {
                        let ident =
                            arg.as_single_ident()
                                .ok_or_else(|| GraphcalError::EvalError {
                                    message:
                                        "`#[assumes(...)]` arguments must be plain identifiers"
                                            .to_string(),
                                    src: src.clone(),
                                    span: arg.span().into(),
                                })?;
                        let arg_name = ident.name.as_str();
                        if !assert_names.contains(arg_name) {
                            return Err(GraphcalError::UnknownAssertInAssumes {
                                name: arg_name.to_string(),
                                src: src.clone(),
                                span: ident.span.into(),
                            });
                        }
                        if let Some(ref dname) = decl_name {
                            assumes_map
                                .entry(arg_name.to_string())
                                .or_default()
                                .push(dname.clone());
                        }
                    }
                }
                "expected_fail" => {
                    let kind = match &decl.kind {
                        DeclKind::Assert(a) => {
                            let ef = parse_expected_fail_args(&attr.args, src)?;
                            // #[expected_fail] (no args) on an indexed assertion is
                            // an error -- the user must specify which variants fail.
                            if matches!(ef, ExpectedFail::All) {
                                let is_indexed = matches!(
                                    &a.body,
                                    AssertBody::Expr(expr) if matches!(expr.kind, ExprKind::ForComp { .. })
                                );
                                if is_indexed {
                                    return Err(GraphcalError::ExpectedFailAllOnIndexed {
                                        src: src.clone(),
                                        span: attr.span.into(),
                                    });
                                }
                            }
                            if let Some(ref dname) = decl_name {
                                expected_fail_map.insert(dname.clone(), ef);
                            }
                            continue;
                        }
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                        DeclKind::Import(_) => "import",
                    };
                    return Err(GraphcalError::InvalidExpectedFailTarget {
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                "lazy" => {}
                "allow_defaults" => {
                    let kind = match &decl.kind {
                        DeclKind::Import(_) => continue,
                        DeclKind::Param(_) => "param",
                        DeclKind::Node(_) => "node",
                        DeclKind::Const(_) => "const",
                        DeclKind::Assert(_) => "assert",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Fn(_) => "fn",
                        DeclKind::Dimension(_) => "dimension",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "index",
                    };
                    return Err(GraphcalError::InvalidAttributeTarget {
                        attr_name: "allow_defaults".to_string(),
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                _ => {
                    return Err(GraphcalError::UnknownAttribute {
                        name: attr_name.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
            }
        }
    }

    Ok(ResolvedFile {
        consts,
        params,
        nodes,
        asserts,
        plots,
        figures,
        runtime_deps,
        const_deps,
        source_order,
        functions: all_functions,
        assert_names,
        assumes_map,
        expected_fail: expected_fail_map,
    })
}

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{AssertBody, DeclKind, Expr, ExprKind, File, FnBody, FnDecl, TypeExpr};
use graphcal_syntax::span::Span;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::error::GraphcalError;

/// Aggregation functions recognized as special forms (not registered as builtins).
const AGGREGATION_FNS: &[&str] = &["sum", "min", "max", "mean", "count"];
const CONVERSION_FNS: &[&str] = &["to_float", "to_int"];

/// A declaration name that may optionally be module-qualified.
///
/// Selective imports produce `Local` names (`x`), while module imports produce
/// `Qualified` names (`module::x`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScopedName {
    /// A bare local name: `x`, `G0`, etc.
    Local(String),
    /// A module-qualified name: `module::x`, `constants::G0`, etc.
    Qualified { module: String, member: String },
}

impl ScopedName {
    /// Returns the member (leaf) part of the name.
    ///
    /// For `Local("x")` this returns `"x"`.
    /// For `Qualified { module: "m", member: "x" }` this also returns `"x"`.
    pub fn member(&self) -> &str {
        match self {
            Self::Local(name) => name,
            Self::Qualified { member, .. } => member,
        }
    }
}

impl std::fmt::Display for ScopedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(name) => write!(f, "{name}"),
            Self::Qualified { module, member } => write!(f, "{module}::{member}"),
        }
    }
}

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

/// Pre-evaluated value bindings imported from already-evaluated dependency files.
///
/// Unlike [`ImportedNames`] which carries AST expressions, this carries
/// evaluated values. Used in per-file evaluation where each file is
/// compiled and evaluated independently.
#[derive(Debug, Default)]
pub struct ImportedValueNames {
    /// Imported const names (for scope checking only — actual values are in the exec plan).
    pub const_names: Vec<(ScopedName, Span)>,
    /// Imported param names.
    pub param_names: Vec<(ScopedName, Span)>,
    /// Imported node names.
    pub node_names: Vec<(ScopedName, Span)>,
    /// Imported function declarations (still need AST for compilation).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Imported assert names (for `#[assumes]` validation).
    pub assert_names: Vec<(String, Span)>,
}

/// The kind of a declaration (used for source-order tracking).
#[derive(Debug, Clone, Copy)]
pub enum DeclCategory {
    Const,
    Param,
    Node,
    Assert,
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
    /// Assert declarations in source order: (name, body, span).
    pub asserts: Vec<(String, AssertBody, Span)>,
    /// For each node/param, the set of `@`-references (graph deps).
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of `CONST_REF` references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Set of all assert names (for checking `@assert_name` errors).
    pub assert_names: HashSet<String>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub assumes_map: HashMap<String, Vec<String>>,
}

/// Resolve names, check casing, detect duplicates, and extract dependencies.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, casing
/// violations, or arity mismatches are found.
#[cfg(test)]
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
                asserts.push((aname, a.body.clone(), decl.span));
            }
            DeclKind::Fn(f) => {
                // Enforce @ prohibition in function bodies
                check_no_graph_refs_in_fn(f, src)?;
                functions.push((f.name.value.to_string(), f.clone(), decl.span));
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
                consts.push((cname, c.value.clone(), decl.span));
            }
            DeclKind::Param(p) => {
                check_no_assert_graph_refs(&p.value, &assert_names, src)?;
                let (graph_refs, _const_refs) = extract_all_refs(
                    &p.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                let pname = p.name.value.to_string();
                runtime_deps.insert(pname.clone(), graph_refs);
                params.push((pname, p.value.clone(), decl.span));
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
                nodes.push((nname, n.value.clone(), decl.span));
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
    // Strip TypeExpr from imported tuples since ResolvedFile uses 3-tuples.
    let mut all_consts: Vec<(String, Expr, Span)> = imported
        .consts
        .iter()
        .map(|(name, _, expr, span)| (name.clone(), expr.clone(), *span))
        .collect();
    all_consts.extend(consts);
    let mut all_params: Vec<(String, Expr, Span)> = imported
        .params
        .iter()
        .map(|(name, _, expr, span)| (name.clone(), expr.clone(), *span))
        .collect();
    all_params.extend(params);
    let mut all_nodes: Vec<(String, Expr, Span)> = imported
        .nodes
        .iter()
        .map(|(name, _, expr, span)| (name.clone(), expr.clone(), *span))
        .collect();
    all_nodes.extend(nodes);
    let mut all_functions = imported.functions.clone();
    all_functions.extend(functions);
    let mut all_asserts: Vec<(String, AssertBody, Span)> = imported.asserts.clone();
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

    // Validate attributes and build assumes_map
    let mut assumes_map: HashMap<String, Vec<String>> = HashMap::new();
    for decl in &file.declarations {
        let decl_name = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.to_string()),
            DeclKind::Node(n) => Some(n.name.value.to_string()),
            DeclKind::Const(c) => Some(c.name.value.to_string()),
            DeclKind::Assert(a) => Some(a.name.value.to_string()),
            DeclKind::Fn(f) => Some(f.name.value.to_string()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name = attr.name.name.as_str();
            match attr_name {
                "assumes" => {
                    // #[assumes] is only valid on node and param
                    let kind = match &decl.kind {
                        DeclKind::Param(_) | DeclKind::Node(_) => None,
                        DeclKind::Const(_) => Some("const"),
                        DeclKind::Assert(_) => Some("assert"),
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
                        let arg_name = arg.name.as_str();
                        if !assert_names.contains(arg_name) {
                            return Err(GraphcalError::UnknownAssertInAssumes {
                                name: arg_name.to_string(),
                                src: src.clone(),
                                span: arg.span.into(),
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
                "lazy" => {
                    // Recognized but semantics deferred — no validation needed
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
        runtime_deps,
        const_deps,
        source_order: all_source_order,
        functions: all_functions,
        assert_names,
        assumes_map,
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
    let mut functions = Vec::new();
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();
    let mut user_fn_names: HashSet<String> = HashSet::new();
    let mut assert_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (for scope checking only).
    // ScopedName → String conversion: the resolver's internal scope uses flat strings
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
    for (name, _, span) in &imported.functions {
        names.insert(name.clone(), *span);
        user_fn_names.insert(name.clone());
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
                asserts.push((aname, a.body.clone(), decl.span));
            }
            DeclKind::Fn(f) => {
                check_no_graph_refs_in_fn(f, src)?;
                functions.push((f.name.value.to_string(), f.clone(), decl.span));
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
                consts.push((cname, c.value.clone(), decl.span));
            }
            DeclKind::Param(p) => {
                check_no_assert_graph_refs(&p.value, &assert_names, src)?;
                let (graph_refs, _const_refs) = extract_all_refs(
                    &p.value,
                    &all_runtime_names,
                    &all_const_names,
                    &builtin_consts,
                    &builtin_fns,
                    &user_fn_names,
                    src,
                )?;
                let pname = p.name.value.to_string();
                runtime_deps.insert(pname.clone(), graph_refs);
                params.push((pname, p.value.clone(), decl.span));
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
                nodes.push((nname, n.value.clone(), decl.span));
            }
        }
    }

    // Imported functions still need to be in the resolved output for IR compilation.
    let mut all_functions = imported.functions.clone();
    all_functions.extend(functions);

    // Validate attributes and build assumes_map
    let mut assumes_map: HashMap<String, Vec<String>> = HashMap::new();
    for decl in &file.declarations {
        let decl_name = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.to_string()),
            DeclKind::Node(n) => Some(n.name.value.to_string()),
            DeclKind::Const(c) => Some(c.name.value.to_string()),
            DeclKind::Assert(a) => Some(a.name.value.to_string()),
            DeclKind::Fn(f) => Some(f.name.value.to_string()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name = attr.name.name.as_str();
            match attr_name {
                "assumes" => {
                    let kind = match &decl.kind {
                        DeclKind::Param(_) | DeclKind::Node(_) => None,
                        DeclKind::Const(_) => Some("const"),
                        DeclKind::Assert(_) => Some("assert"),
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
                        let arg_name = arg.name.as_str();
                        if !assert_names.contains(arg_name) {
                            return Err(GraphcalError::UnknownAssertInAssumes {
                                name: arg_name.to_string(),
                                src: src.clone(),
                                span: arg.span.into(),
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
                "lazy" => {}
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
        runtime_deps,
        const_deps,
        source_order,
        functions: all_functions,
        assert_names,
        assumes_map,
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
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            Err(GraphcalError::GraphRefInConst {
                name: ident.value.clone(),
                src: src.clone(),
                span: expr.span.into(),
            })
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs(lhs, src)?;
            check_no_graph_refs(rhs, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs(operand, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
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
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            check_no_graph_refs(inner, src)
        }
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
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
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
        ExprKind::Unfold { init, body, .. } => {
            check_no_graph_refs(init, src)?;
            check_no_graph_refs(body, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_graph_refs(scrutinee, src)?;
            for arm in arms {
                check_no_graph_refs(&arm.body, src)?;
            }
            Ok(())
        }
    }
}

/// Check that a function body contains no `@` references (purity enforcement).
fn check_no_graph_refs_in_fn(
    fn_decl: &FnDecl,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let check = |expr: &Expr| -> Result<(), GraphcalError> {
        check_no_graph_refs_in_fn_expr(expr, fn_decl.name.value.as_str(), src)
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
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            Err(GraphcalError::GraphRefInFn {
                name: ident.value.clone(),
                src: src.clone(),
                span: expr.span.into(),
                help: format!("pass `{fn_name}` as a function parameter instead"),
            })
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_graph_refs_in_fn_expr(lhs, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(rhs, fn_name, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_graph_refs_in_fn_expr(operand, fn_name, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
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
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
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
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
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
        ExprKind::Unfold { init, body, .. } => {
            check_no_graph_refs_in_fn_expr(init, fn_name, src)?;
            check_no_graph_refs_in_fn_expr(body, fn_name, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_graph_refs_in_fn_expr(scrutinee, fn_name, src)?;
            for arm in arms {
                check_no_graph_refs_in_fn_expr(&arm.body, fn_name, src)?;
            }
            Ok(())
        }
    }
}

/// Check that an expression does not reference any assert name via `@`.
///
/// Assert declarations are leaf nodes — they cannot be referenced by other declarations.
fn check_no_assert_graph_refs(
    expr: &Expr,
    assert_names: &HashSet<String>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    match &expr.kind {
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            if assert_names.contains(ident.value.as_str()) {
                return Err(GraphcalError::GraphRefToAssert {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: expr.span.into(),
                });
            }
            Ok(())
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::UnitLiteral { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::BinOp { lhs, rhs, .. } => {
            check_no_assert_graph_refs(lhs, assert_names, src)?;
            check_no_assert_graph_refs(rhs, assert_names, src)
        }
        ExprKind::UnaryOp { operand, .. } => check_no_assert_graph_refs(operand, assert_names, src),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                check_no_assert_graph_refs(arg, assert_names, src)?;
            }
            Ok(())
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_no_assert_graph_refs(condition, assert_names, src)?;
            check_no_assert_graph_refs(then_branch, assert_names, src)?;
            check_no_assert_graph_refs(else_branch, assert_names, src)
        }
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            check_no_assert_graph_refs(inner, assert_names, src)
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                check_no_assert_graph_refs(&stmt.value, assert_names, src)?;
            }
            check_no_assert_graph_refs(expr, assert_names, src)
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            check_no_assert_graph_refs(expr, assert_names, src)
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    check_no_assert_graph_refs(val, assert_names, src)?;
                }
            }
            Ok(())
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                check_no_assert_graph_refs(&entry.value, assert_names, src)?;
            }
            Ok(())
        }
        ExprKind::ForComp { body, .. } => check_no_assert_graph_refs(body, assert_names, src),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            check_no_assert_graph_refs(source, assert_names, src)?;
            check_no_assert_graph_refs(init, assert_names, src)?;
            check_no_assert_graph_refs(body, assert_names, src)
        }
        ExprKind::Unfold { init, body, .. } => {
            check_no_assert_graph_refs(init, assert_names, src)?;
            check_no_assert_graph_refs(body, assert_names, src)
        }
        ExprKind::Match { scrutinee, arms } => {
            check_no_assert_graph_refs(scrutinee, assert_names, src)?;
            for arm in arms {
                check_no_assert_graph_refs(&arm.body, assert_names, src)?;
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
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            Err(GraphcalError::EvalError {
                message: format!(
                    "internal: graph reference `@{}` found in const expression",
                    ident.value
                ),
                src: src.clone(),
                span: expr.span.into(),
            })
        }
        ExprKind::ConstRef(ident) | ExprKind::QualifiedConstRef { name: ident, .. } => {
            if builtin_consts.contains_key(ident.value.as_str()) {
                Ok(())
            } else if all_const_names.contains(ident.value.as_str()) {
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
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            let name_str = name.value.as_str();
            if !builtin_fns.contains_key(name_str)
                && !user_fn_names.contains(name_str)
                && !AGGREGATION_FNS.contains(&name_str)
                && !CONVERSION_FNS.contains(&name_str)
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
                && args.len() != builtin.arity
                && !AGGREGATION_FNS.contains(&name_str)
            {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
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
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            collect_const_refs(
                inner,
                all_const_names,
                builtin_consts,
                builtin_fns,
                user_fn_names,
                src,
                deps,
            )
        }
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
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => Ok(()),
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            if all_runtime_names.contains(ident.value.as_str()) {
                graph_refs.insert(ident.value.to_string());
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
            if builtin_consts.contains_key(ident.value.as_str()) {
                Ok(())
            } else if all_const_names.contains(ident.value.as_str()) {
                const_refs.insert(ident.value.to_string());
                Ok(())
            } else {
                Err(GraphcalError::UnknownConstRef {
                    name: ident.value.clone(),
                    src: src.clone(),
                    span: ident.span.into(),
                })
            }
        }
        ExprKind::FnCall { name, args } | ExprKind::QualifiedFnCall { name, args, .. } => {
            let name_str = name.value.as_str();
            if !builtin_fns.contains_key(name_str)
                && !user_fn_names.contains(name_str)
                && !AGGREGATION_FNS.contains(&name_str)
                && !CONVERSION_FNS.contains(&name_str)
            {
                return Err(GraphcalError::UnknownFunction {
                    name: name.value.clone(),
                    src: src.clone(),
                    span: name.span.into(),
                });
            }
            if let Some(builtin) = builtin_fns.get(name_str)
                && args.len() != builtin.arity
                && !AGGREGATION_FNS.contains(&name_str)
            {
                return Err(GraphcalError::WrongArity {
                    name: name.value.clone(),
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
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            collect_all_refs(
                inner,
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
        ExprKind::GraphRef(ident) | ExprKind::QualifiedGraphRef { name: ident, .. } => {
            if all_runtime_names.contains(ident.value.as_str()) {
                refs.insert(ident.value.to_string());
            }
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_graph_refs(lhs, all_runtime_names, refs);
            collect_graph_refs(rhs, all_runtime_names, refs);
        }
        ExprKind::UnaryOp { operand, .. } => {
            collect_graph_refs(operand, all_runtime_names, refs);
        }
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
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
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
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
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
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
        ExprKind::Unfold { init, body, .. } => {
            collect_graph_refs(init, all_runtime_names, refs);
            collect_graph_refs(body, all_runtime_names, refs);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_graph_refs(scrutinee, all_runtime_names, refs);
            for arm in arms {
                collect_graph_refs(&arm.body, all_runtime_names, refs);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
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
    fn resolve_import_decl_skipped() {
        // import declarations should not be treated as param/node/const
        let source = r#"import "./helper.gcl" { something };"#;
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

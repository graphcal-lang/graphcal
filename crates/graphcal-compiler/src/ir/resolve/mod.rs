mod deps;
mod names;
mod scope;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::registry::builtins::{builtin_constants, builtin_functions};
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{
    ResolvedAssertEntry, ResolvedConstEntry, ResolvedFigureEntry, ResolvedLayerEntry,
    ResolvedNodeEntry, ResolvedParamEntry, ResolvedPlotEntry,
};
use crate::syntax::ast::TypeExpr;
use crate::syntax::ast::{AssertBody, DeclKind, Expr, ExprKind, File};
use crate::syntax::span::Span;

// Re-export types and constants from graphcal-registry's resolve_types module.
pub use crate::registry::resolve_types::{
    DeclCategory, ExpectedFail, ExpectedFailKey, ImportedValueNames, ResolvedFile, ScopedName,
    is_aggregation_fn, is_time_scale_name,
};

// Re-export items from submodules (crate-internal only).
pub use deps::collect_graph_refs;

// Import helpers from submodules for use within this file.
use deps::{extract_all_refs, extract_const_refs};
use names::parse_expected_fail_args;
use scope::{
    check_no_assert_graph_refs, check_no_pub_index_variant_literals, check_no_runtime_graph_refs,
};

/// Known attribute names in the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributeName {
    Assumes,
    ExpectedFail,
    Lazy,
    AllowDefaults,
}

impl std::str::FromStr for AttributeName {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "assumes" => Ok(Self::Assumes),
            "expected_fail" => Ok(Self::ExpectedFail),
            "lazy" => Ok(Self::Lazy),
            "allow_defaults" => Ok(Self::AllowDefaults),
            _ => Err(()),
        }
    }
}

impl AttributeName {
    /// Get the string representation of the attribute name.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Assumes => "assumes",
            Self::ExpectedFail => "expected_fail",
            Self::Lazy => "lazy",
            Self::AllowDefaults => "allow_defaults",
        }
    }
}

/// Classification of a name in the resolver's scope.
///
/// Used to partition names into const vs runtime sets without relying on casing heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameCategory {
    Const,
    Runtime,
}

/// Result of collecting local declarations from the AST.
struct CollectedDeclarations {
    consts: Vec<ResolvedConstEntry>,
    params: Vec<ResolvedParamEntry>,
    nodes: Vec<ResolvedNodeEntry>,
    asserts: Vec<ResolvedAssertEntry>,
    plots: Vec<ResolvedPlotEntry>,
    figures: Vec<ResolvedFigureEntry>,
    layers: Vec<ResolvedLayerEntry>,
    runtime_deps: HashMap<String, HashSet<String>>,
    const_deps: HashMap<String, HashSet<String>>,
    source_order: Vec<(String, DeclCategory)>,
    user_fn_names: HashSet<String>,
    assert_names: HashSet<String>,
    pub_names: HashSet<String>,
}

/// Collect all local declarations, check for duplicates and casing violations.
///
/// The `imported_user_fns` parameter should contain function names that were imported
/// and should be recognized as valid user functions during reference checking.
///
/// Returns the collected declarations and the names map for further processing.
#[expect(
    clippy::too_many_lines,
    reason = "complex declaration collection with multiple passes"
)]
fn collect_local_declarations(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &mut HashMap<String, (Span, NameCategory)>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
    imported_user_fns: &HashSet<String>,
) -> Result<CollectedDeclarations, GraphcalError> {
    let mut consts = Vec::new();
    let mut params = Vec::new();
    let mut nodes = Vec::new();
    let mut asserts = Vec::new();
    let mut plots = Vec::new();
    let mut figures = Vec::new();
    let mut layers = Vec::new();
    let mut runtime_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut const_deps: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_order = Vec::new();
    let mut assert_names: HashSet<String> = HashSet::new();

    // Collect names of all visible declarations. Explicit `pub`/`pub(bind)`
    // declarations contribute; params are implicitly visible+bindable under
    // A5 and always contribute.
    let mut pub_names: HashSet<String> = HashSet::new();
    for decl in &file.declarations {
        let implicitly_visible = matches!(decl.kind, DeclKind::Param(_));
        if !decl.is_pub() && !implicitly_visible {
            continue;
        }
        let Some((name, _)) = decl.kind.name_and_span() else {
            continue;
        };
        pub_names.insert(name.to_string());
    }

    // Validate: required `index`, `type`, `dim` must be `pub(bind)` (V002).
    //
    // Required `param` is excluded from this check: per axiom A5 §4.0,
    // `param` is implicitly V=visible + B=bindable and never carries a
    // visibility annotation. The parser rejects `pub`/`pub(bind)` on
    // `param`.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Index(idx) if idx.kind.is_required() && !decl.is_bindable() => {
                return Err(GraphcalError::RequiredItemMustBeBindable {
                    kind: "index".to_string(),
                    name: idx.name.value.to_string(),
                    src: src.clone(),
                    span: idx.name.span.into(),
                });
            }
            DeclKind::Type(t) if t.fields.is_none() && !decl.is_bindable() => {
                return Err(GraphcalError::RequiredItemMustBeBindable {
                    kind: "type".to_string(),
                    name: t.name.value.to_string(),
                    src: src.clone(),
                    span: t.name.span.into(),
                });
            }
            DeclKind::Dimension(d) if d.definition.is_none() && !decl.is_bindable() => {
                return Err(GraphcalError::RequiredItemMustBeBindable {
                    kind: "dim".to_string(),
                    name: d.name.value.to_string(),
                    src: src.clone(),
                    span: d.name.span.into(),
                });
            }
            _ => {}
        }
    }

    // Collect `pub(bind)` index names with concrete variants (for V004 /
    // A10(c) variant-literal restriction). Plain `pub` indexes are not
    // bindable, so A10 never fires on their variant literals. Required
    // `pub(bind)` indexes have no declared variants so they carry no
    // variant literals either; the filter below excludes them.
    let pub_bind_index_names: HashSet<String> = file
        .declarations
        .iter()
        .filter_map(|decl| {
            if !decl.is_bindable() {
                return None;
            }
            if let DeclKind::Index(idx) = &decl.kind
                && !idx.kind.is_required()
            {
                return Some(idx.name.value.to_string());
            }
            None
        })
        .collect();

    // Build combined user function names (imported + local) for reference checking
    let all_user_fn_names = imported_user_fns.clone();

    // First pass: collect all declarations and check for duplicates + casing
    for decl in &file.declarations {
        // Dimension and Unit declarations are handled by the registry, not the resolver
        let (name, name_span, is_const) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Node(n) => (n.name.value.to_string(), n.name.span, false),
            DeclKind::ConstNode(c) => (c.name.value.to_string(), c.name.span, true),
            DeclKind::Assert(a) => (a.name.value.to_string(), a.name.span, false),
            DeclKind::Plot(p) => (p.name.value.to_string(), p.name.span, false),
            DeclKind::Figure(f) => (f.name.value.to_string(), f.name.span, false),
            DeclKind::Layer(l) => (l.name.value.to_string(), l.name.span, false),
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                continue;
            }
        };

        // Check for duplicates
        if let Some((first_span, _)) = names.get(&name) {
            return Err(GraphcalError::DuplicateName {
                name,
                src: src.clone(),
                duplicate: name_span.into(),
                first: (*first_span).into(),
            });
        }
        let name_cat = if is_const {
            NameCategory::Const
        } else {
            NameCategory::Runtime
        };
        names.insert(name.clone(), (name_span, name_cat));

        // Track source order and assert names
        let category = match &decl.kind {
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::ConstNode(_) => DeclCategory::Const,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Assert(_) => {
                assert_names.insert(name.clone());
                DeclCategory::Assert
            }
            DeclKind::Plot(_) => DeclCategory::Plot,
            DeclKind::Figure(_) => DeclCategory::Figure,
            DeclKind::Layer(_) => DeclCategory::Layer,
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                // These declarations are handled earlier (continue'd before reaching here).
                continue;
            }
        };
        source_order.push((name.clone(), category));
    }

    // Build the set of all known names for reference checking.
    let (all_const_names, all_runtime_names) = build_name_sets(names);

    // Second pass: resolve references and extract dependencies
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::UnionType(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {}
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
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    // Check that assert body doesn't reference other assert names via @
                    check_no_assert_graph_refs(body_expr, &assert_names, src)?;
                    // A10(b): public sink kinds travel with the include,
                    // so their bodies must abstract over pub(bind)
                    // indexes. Private sinks are pruned on include, so
                    // their literal mentions cannot orphan anything.
                    if decl.is_pub() {
                        check_no_pub_index_variant_literals(body_expr, &pub_bind_index_names, src)?;
                    }
                }
                let aname = a.name.value.to_string();
                asserts.push(ResolvedAssertEntry {
                    name: aname,
                    body: a.body.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Plot(p) => {
                // Validate references in plot encoding and property expressions (plots CAN use @).
                // A10(b): public sinks must abstract over pub(bind) indexes.
                let pub_sink = decl.is_pub();
                for encoding in &p.encodings {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &encoding.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    check_no_assert_graph_refs(&encoding.value, &assert_names, src)?;
                    if pub_sink {
                        check_no_pub_index_variant_literals(
                            &encoding.value,
                            &pub_bind_index_names,
                            src,
                        )?;
                    }
                }
                for prop in &p.mark.properties {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &prop.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    check_no_assert_graph_refs(&prop.value, &assert_names, src)?;
                    if pub_sink {
                        check_no_pub_index_variant_literals(
                            &prop.value,
                            &pub_bind_index_names,
                            src,
                        )?;
                    }
                }
                for prop in &p.properties {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &prop.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    check_no_assert_graph_refs(&prop.value, &assert_names, src)?;
                    if pub_sink {
                        check_no_pub_index_variant_literals(
                            &prop.value,
                            &pub_bind_index_names,
                            src,
                        )?;
                    }
                }
                let pname = p.name.value.to_string();
                plots.push(ResolvedPlotEntry {
                    name: pname,
                    decl: p.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Figure(f) => {
                // Validate references in figure field expressions (figures CAN use @).
                let pub_sink = decl.is_pub();
                for field in &f.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                    if pub_sink {
                        check_no_pub_index_variant_literals(
                            &field.value,
                            &pub_bind_index_names,
                            src,
                        )?;
                    }
                }
                let fname = f.name.value.to_string();
                figures.push(ResolvedFigureEntry {
                    name: fname,
                    decl: f.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Layer(l) => {
                // Validate references in layer field expressions (layers CAN use @).
                let pub_sink = decl.is_pub();
                for field in &l.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
                    )?;
                    check_no_assert_graph_refs(&field.value, &assert_names, src)?;
                    if pub_sink {
                        check_no_pub_index_variant_literals(
                            &field.value,
                            &pub_bind_index_names,
                            src,
                        )?;
                    }
                }
                let lname = l.name.value.to_string();
                layers.push(ResolvedLayerEntry {
                    name: lname,
                    decl: l.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Param(p) => {
                let pname = p.name.value.to_string();
                if let Some(ref value) = p.value {
                    check_no_assert_graph_refs(value, &assert_names, src)?;
                    // A10(a) + A5: `param` is implicitly bindable, so a
                    // variant literal of a `pub(bind)` index in a param
                    // default is always fine — the importer rebinding the
                    // index will also be required to rebind the param
                    // (V005, enforced at the include site).
                    let (graph_refs, _const_refs) = extract_all_refs(
                        value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
                        &all_user_fn_names,
                        src,
                        None,
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
            DeclKind::ConstNode(c) => {
                check_no_runtime_graph_refs(&c.value, &all_runtime_names, src)?;
                check_no_pub_index_variant_literals(&c.value, &pub_bind_index_names, src)?;
                let deps = extract_const_refs(
                    &c.value,
                    &all_const_names,
                    builtin_consts,
                    builtin_fns,
                    &all_user_fn_names,
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
            DeclKind::Node(n) => {
                check_no_assert_graph_refs(&n.value, &assert_names, src)?;
                check_no_pub_index_variant_literals(&n.value, &pub_bind_index_names, src)?;
                let nname = n.name.value.to_string();
                let (graph_refs, _const_refs) = extract_all_refs(
                    &n.value,
                    &all_runtime_names,
                    &all_const_names,
                    builtin_consts,
                    builtin_fns,
                    &all_user_fn_names,
                    src,
                    Some(&nname),
                )?;
                runtime_deps.insert(nname.clone(), graph_refs);
                nodes.push(ResolvedNodeEntry {
                    name: nname,
                    expr: n.value.clone(),
                    span: decl.span,
                });
            }
        }
    }

    Ok(CollectedDeclarations {
        consts,
        params,
        nodes,
        asserts,
        plots,
        figures,
        layers,
        runtime_deps,
        const_deps,
        source_order,
        user_fn_names: HashSet::new(),
        assert_names,
        pub_names,
    })
}

/// Build const and runtime name sets from the names map using stored categories.
fn build_name_sets(
    names: &HashMap<String, (Span, NameCategory)>,
) -> (HashSet<&str>, HashSet<&str>) {
    let all_const_names: HashSet<&str> = names
        .iter()
        .filter(|(_, (_, cat))| *cat == NameCategory::Const)
        .map(|(name, _)| name.as_str())
        .collect();
    let all_runtime_names: HashSet<&str> = names
        .iter()
        .filter(|(_, (_, cat))| *cat == NameCategory::Runtime)
        .map(|(name, _)| name.as_str())
        .collect();
    (all_const_names, all_runtime_names)
}

/// Result of attribute validation.
struct ValidatedAttributes {
    assumes_map: HashMap<String, Vec<String>>,
    expected_fail_map: HashMap<String, ExpectedFail>,
}

/// Validate attributes and build `assumes_map` / `expected_fail_map`.
#[expect(clippy::too_many_lines, reason = "comprehensive attribute validation")]
fn validate_attributes(
    file: &File,
    src: &NamedSource<Arc<String>>,
    assert_names: &HashSet<String>,
) -> Result<ValidatedAttributes, GraphcalError> {
    let mut assumes_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut expected_fail_map: HashMap<String, ExpectedFail> = HashMap::new();

    for decl in &file.declarations {
        let decl_name = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.to_string()),
            DeclKind::Node(n) => Some(n.name.value.to_string()),
            DeclKind::ConstNode(c) => Some(c.name.value.to_string()),
            DeclKind::Assert(a) => Some(a.name.value.to_string()),
            DeclKind::Plot(p) => Some(p.name.value.to_string()),
            DeclKind::Figure(f) => Some(f.name.value.to_string()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name_str = attr.name.name.as_str();
            let attr_name = attr_name_str.parse::<AttributeName>().map_err(|()| {
                GraphcalError::UnknownAttribute {
                    name: attr_name_str.to_string(),
                    src: src.clone(),
                    span: attr.span.into(),
                }
            })?;

            match attr_name {
                AttributeName::Assumes => {
                    // #[assumes] is only valid on non-const node and param
                    let kind = match &decl.kind {
                        DeclKind::ConstNode(_) => Some("const node"),
                        DeclKind::Param(_) | DeclKind::Node(_) => None,
                        DeclKind::Assert(_) => Some("assert"),
                        DeclKind::Plot(_) => Some("plot"),
                        DeclKind::Figure(_) => Some("figure"),
                        DeclKind::Layer(_) => Some("layer"),

                        DeclKind::BaseDimension(_) | DeclKind::Dimension(_) => Some("dim"),
                        DeclKind::Unit(_) => Some("unit"),
                        DeclKind::Type(_) | DeclKind::UnionType(_) => Some("type"),
                        DeclKind::Index(_) => Some("cat/range"),
                        DeclKind::Import(_) => Some("import"),
                        DeclKind::Include(_) => Some("include"),
                        DeclKind::Dag(_) => Some("dag"),
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
                AttributeName::ExpectedFail => {
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
                        DeclKind::ConstNode(_) => "const node",
                        DeclKind::Node(_) => "node",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Layer(_) => "layer",

                        DeclKind::BaseDimension(_) | DeclKind::Dimension(_) => "dim",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) | DeclKind::UnionType(_) => "type",
                        DeclKind::Index(_) => "cat/range",
                        DeclKind::Import(_) => "import",
                        DeclKind::Include(_) => "include",
                        DeclKind::Dag(_) => "dag",
                    };
                    return Err(GraphcalError::InvalidExpectedFailTarget {
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
                AttributeName::Lazy => {
                    // Recognized but semantics deferred — no validation needed
                }
                AttributeName::AllowDefaults => {
                    // #[allow_defaults] is only valid on include declarations
                    let kind = match &decl.kind {
                        DeclKind::Include(_) => continue,
                        DeclKind::Param(_) => "param",
                        DeclKind::ConstNode(_) => "const node",
                        DeclKind::Node(_) => "node",
                        DeclKind::Assert(_) => "assert",
                        DeclKind::Plot(_) => "plot",
                        DeclKind::Figure(_) => "figure",
                        DeclKind::Layer(_) => "layer",

                        DeclKind::BaseDimension(_) | DeclKind::Dimension(_) => "dim",
                        DeclKind::Unit(_) => "unit",
                        DeclKind::Type(_) | DeclKind::UnionType(_) => "type",
                        DeclKind::Index(_) => "cat/range",
                        DeclKind::Import(_) => "import",
                        DeclKind::Dag(_) => "dag",
                    };
                    return Err(GraphcalError::InvalidAttributeTarget {
                        attr_name: AttributeName::AllowDefaults.as_str().to_string(),
                        kind: kind.to_string(),
                        src: src.clone(),
                        span: attr.span.into(),
                    });
                }
            }
        }
    }

    Ok(ValidatedAttributes {
        assumes_map,
        expected_fail_map,
    })
}

/// Validate that every visible declaration names only visible type-system
/// symbols in its written signature (V003 / A9 case 1).
///
/// A declaration's signature is checked when the declaration is visible
/// at the library boundary: either explicitly `pub` / `pub(bind)`, or
/// implicitly visible (`param`, per A5 §4.0).
///
/// Built-in type-system items (prelude dimensions like `Length`, and
/// built-in types `Bool`, `Int`, `Dimensionless`, `Datetime`) are
/// always considered visible.
#[expect(
    clippy::too_many_lines,
    reason = "per-kind signature extraction for every declaration kind"
)]
fn validate_private_in_public(
    file: &File,
    src: &NamedSource<Arc<String>>,
    pub_names: &HashSet<String>,
) -> Result<(), GraphcalError> {
    use crate::syntax::ast::{DimExpr, IndexDeclKind, IndexExpr, TypeExpr, TypeExprKind};

    // Collect all locally-declared type-system names (dims, indexes, types) with their spans.
    let mut local_type_names: HashMap<String, Span> = HashMap::new();
    for decl in &file.declarations {
        let (name, span) = match &decl.kind {
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), d.name.span),
            DeclKind::Dimension(d) => (d.name.value.to_string(), d.name.span),
            DeclKind::Index(idx) => (idx.name.value.to_string(), idx.name.span),
            DeclKind::Type(t) => (t.name.value.to_string(), t.name.span),
            DeclKind::UnionType(u) => (u.name.value.to_string(), u.name.span),
            _ => continue,
        };
        local_type_names.insert(name, span);
    }

    // If there are no local type-system names, nothing to check.
    if local_type_names.is_empty() {
        return Ok(());
    }

    // Recursively collect type-system references from a `TypeExpr`.
    #[expect(
        clippy::items_after_statements,
        reason = "helper function scoped to this validation"
    )]
    fn collect_type_refs(type_expr: &TypeExpr, refs: &mut Vec<(String, Span)>) {
        match &type_expr.kind {
            TypeExprKind::DimExpr(dim_expr) => collect_dim_refs(dim_expr, refs),
            TypeExprKind::Indexed { base, indexes } => {
                collect_type_refs(base, refs);
                for idx in indexes {
                    if let IndexExpr::Name(ident) = idx {
                        refs.push((ident.name.clone(), ident.span));
                    }
                }
            }
            TypeExprKind::TypeApplication { name, type_args } => {
                refs.push((name.name.clone(), name.span));
                for arg in type_args {
                    collect_type_refs(arg, refs);
                }
            }
            TypeExprKind::Dimensionless
            | TypeExprKind::Bool
            | TypeExprKind::Int
            | TypeExprKind::Datetime => {}
        }
    }

    #[expect(
        clippy::items_after_statements,
        reason = "helper function scoped to this validation"
    )]
    fn collect_dim_refs(dim_expr: &DimExpr, refs: &mut Vec<(String, Span)>) {
        for item in &dim_expr.terms {
            refs.push((item.term.name.name.clone(), item.term.span));
        }
    }

    // Classify the owning declaration of a referenced name for the error
    // message.
    #[expect(
        clippy::items_after_statements,
        reason = "helper function scoped to this validation"
    )]
    fn ref_kind_for(file: &File, ref_name: &str) -> &'static str {
        match file
            .declarations
            .iter()
            .find(|d| match &d.kind {
                DeclKind::BaseDimension(bd) => bd.name.value.as_str() == ref_name,
                DeclKind::Dimension(d) => d.name.value.as_str() == ref_name,
                DeclKind::Index(idx) => idx.name.value.as_str() == ref_name,
                DeclKind::Type(t) => t.name.value.as_str() == ref_name,
                DeclKind::UnionType(u) => u.name.value.as_str() == ref_name,
                _ => false,
            })
            .map(|d| &d.kind)
        {
            Some(DeclKind::BaseDimension(_) | DeclKind::Dimension(_)) => "dim",
            Some(DeclKind::Index(_)) => "index",
            Some(DeclKind::Type(_) | DeclKind::UnionType(_)) => "type",
            _ => "item",
        }
    }

    let emit = |pub_kind: &str,
                pub_name: String,
                pub_span: Span,
                refs: &[(String, Span)]|
     -> Result<(), GraphcalError> {
        for (ref_name, ref_span) in refs {
            if local_type_names.contains_key(ref_name) && !pub_names.contains(ref_name) {
                return Err(GraphcalError::PrivateInPublic {
                    pub_kind: pub_kind.to_string(),
                    pub_name,
                    ref_kind: ref_kind_for(file, ref_name).to_string(),
                    ref_name: ref_name.clone(),
                    src: src.clone(),
                    ref_span: (*ref_span).into(),
                    pub_span: pub_span.into(),
                });
            }
        }
        Ok(())
    };

    for decl in &file.declarations {
        // `param` is always visible (A5 §4.0); other kinds only when
        // explicitly marked `pub` / `pub(bind)`.
        let implicitly_visible = matches!(decl.kind, DeclKind::Param(_));
        if !decl.is_pub() && !implicitly_visible {
            continue;
        }

        let mut refs: Vec<(String, Span)> = Vec::new();
        let (kind, name): (&str, String) = match &decl.kind {
            DeclKind::Param(p) => {
                collect_type_refs(&p.type_ann, &mut refs);
                ("param", p.name.value.to_string())
            }
            DeclKind::Node(n) => {
                collect_type_refs(&n.type_ann, &mut refs);
                ("node", n.name.value.to_string())
            }
            DeclKind::ConstNode(c) => {
                collect_type_refs(&c.type_ann, &mut refs);
                ("const node", c.name.value.to_string())
            }
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    collect_dim_refs(def, &mut refs);
                }
                ("dim", d.name.value.to_string())
            }
            DeclKind::Unit(u) => {
                collect_dim_refs(&u.dim_type, &mut refs);
                ("unit", u.name.value.to_string())
            }
            DeclKind::Type(t) => {
                if let Some(fields) = &t.fields {
                    for field in fields {
                        collect_type_refs(&field.type_ann, &mut refs);
                    }
                }
                ("type", t.name.value.to_string())
            }
            DeclKind::UnionType(u) => {
                for member in &u.members {
                    refs.push((member.name.value.to_string(), member.name.span));
                    for arg in &member.type_args {
                        collect_type_refs(arg, &mut refs);
                    }
                }
                ("type", u.name.value.to_string())
            }
            DeclKind::Index(idx) => {
                if let IndexDeclKind::RequiredRange { dimension } = &idx.kind {
                    collect_dim_refs(dimension, &mut refs);
                }
                ("index", idx.name.value.to_string())
            }
            // Sink kinds have no written signature; bodies are not A9 case 1.
            // BaseDimension has no body. Import/Include are use-sites. Dag is
            // a use-site at the signature level.
            _ => continue,
        };

        emit(kind, name, decl.span, &refs)?;
    }
    Ok(())
}

/// Declarations imported from other files, to be injected into the resolve scope.
///
/// These are treated as if they were declared locally, appearing before local declarations.
#[derive(Debug, Default)]
pub(crate) struct ImportedNames {
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    pub params: Vec<(String, TypeExpr, Expr, Span)>,
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
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
pub(crate) fn resolve_with_imports(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<ResolvedFile, GraphcalError> {
    // Ensure NameRef/QualifiedNameRef are resolved before proceeding.
    let mut file = file.clone();
    crate::syntax::name_resolve::resolve_name_refs(&mut file);
    let file = &file;

    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<String, (Span, NameCategory)> = HashMap::new();
    let imported_fn_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    for (name, _, _, span) in &imported.consts {
        names.insert(name.clone(), (*span, NameCategory::Const));
    }
    for (name, _, _, span) in &imported.params {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }
    for (name, _, _, span) in &imported.nodes {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }
    for (name, _, span) in &imported.asserts {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }

    // Collect local declarations
    let local = collect_local_declarations(
        file,
        src,
        &mut names,
        builtin_consts,
        builtin_fns,
        &imported_fn_names,
    )?;

    // Build name sets for dependency extraction
    let (all_const_names, all_runtime_names) = build_name_sets(&names);

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names = HashSet::new();
    for (name, _, _) in &imported.asserts {
        all_assert_names.insert(name.clone());
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

    // Extract dependencies for imported declarations so the DAG is complete.
    // Without this, imported nodes' @-references are invisible to the topological sort,
    // causing evaluation-order errors (Bug 2).
    let mut runtime_deps = local.runtime_deps;
    let mut const_deps = local.const_deps;

    for (name, _, expr, _) in &imported.consts {
        let deps = extract_const_refs(
            expr,
            &all_const_names,
            builtin_consts,
            builtin_fns,
            &local.user_fn_names,
            src,
        )?;
        const_deps.insert(name.clone(), deps);
    }
    for (name, _, expr, _) in &imported.params {
        let (graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            builtin_consts,
            builtin_fns,
            &local.user_fn_names,
            src,
            None,
        )?;
        runtime_deps.insert(name.clone(), graph_refs);
    }
    for (name, _, expr, _) in &imported.nodes {
        let (graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            builtin_consts,
            builtin_fns,
            &local.user_fn_names,
            src,
            Some(name.as_str()),
        )?;
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
    all_consts.extend(local.consts);
    let mut all_params: Vec<ResolvedParamEntry> = imported
        .params
        .iter()
        .map(|(name, _, expr, span)| ResolvedParamEntry {
            name: name.clone(),
            default_expr: Some(expr.clone()),
            span: *span,
        })
        .collect();
    all_params.extend(local.params);
    let mut all_nodes: Vec<ResolvedNodeEntry> = imported
        .nodes
        .iter()
        .map(|(name, _, expr, span)| ResolvedNodeEntry {
            name: name.clone(),
            expr: expr.clone(),
            span: *span,
        })
        .collect();
    all_nodes.extend(local.nodes);
    let mut all_asserts: Vec<ResolvedAssertEntry> = imported
        .asserts
        .iter()
        .map(|(name, body, span)| ResolvedAssertEntry {
            name: name.clone(),
            body: body.clone(),
            span: *span,
        })
        .collect();
    all_asserts.extend(local.asserts);

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
    all_source_order.extend(local.source_order);

    // Validate attributes and build assumes_map / expected_fail_map
    let validated = validate_attributes(file, src, &all_assert_names)?;

    // Validate private-in-public: pub declarations must not reference private type-system items
    validate_private_in_public(file, src, &local.pub_names)?;

    Ok(ResolvedFile {
        consts: all_consts,
        params: all_params,
        nodes: all_nodes,
        asserts: all_asserts,
        plots: local.plots,
        figures: local.figures,
        layers: local.layers,
        runtime_deps,
        const_deps,
        source_order: all_source_order,
        assert_names: all_assert_names,
        assumes_map: validated.assumes_map,
        expected_fail: validated.expected_fail_map,
        pub_names: local.pub_names,
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
pub(crate) fn resolve_with_imported_values(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedValueNames,
) -> Result<ResolvedFile, GraphcalError> {
    // Ensure NameRef/QualifiedNameRef are resolved before proceeding.
    // This is idempotent if already resolved by the loader.
    let mut file = file.clone();
    crate::syntax::name_resolve::resolve_name_refs(&mut file);
    let file = &file;

    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<String, (Span, NameCategory)> = HashMap::new();
    let imported_fn_names: HashSet<String> = HashSet::new();

    // Pre-populate with imported names (for scope checking only).
    // ScopedName -> String conversion: the resolver's internal scope uses flat strings
    // because it mixes imported names with local declarations.
    for (name, span) in &imported.const_names {
        names.insert(name.to_string(), (*span, NameCategory::Const));
    }
    for (name, span) in &imported.param_names {
        names.insert(name.to_string(), (*span, NameCategory::Runtime));
    }
    for (name, span) in &imported.node_names {
        names.insert(name.to_string(), (*span, NameCategory::Runtime));
    }
    for (name, span) in &imported.assert_names {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }

    // Collect local declarations
    let local = collect_local_declarations(
        file,
        src,
        &mut names,
        builtin_consts,
        builtin_fns,
        &imported_fn_names,
    )?;

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names = HashSet::new();
    for (name, _) in &imported.assert_names {
        all_assert_names.insert(name.clone());
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

    // Validate attributes and build assumes_map / expected_fail_map
    let validated = validate_attributes(file, src, &all_assert_names)?;

    // Validate private-in-public: pub declarations must not reference private type-system items
    validate_private_in_public(file, src, &local.pub_names)?;

    Ok(ResolvedFile {
        consts: local.consts,
        params: local.params,
        nodes: local.nodes,
        asserts: local.asserts,
        plots: local.plots,
        figures: local.figures,
        layers: local.layers,
        runtime_deps: local.runtime_deps,
        const_deps: local.const_deps,
        source_order: local.source_order,
        assert_names: all_assert_names,
        assumes_map: validated.assumes_map,
        expected_fail: validated.expected_fail_map,
        pub_names: local.pub_names,
    })
}

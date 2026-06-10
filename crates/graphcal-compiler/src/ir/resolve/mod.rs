mod deps;
pub(crate) mod names;
mod scope;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::resolved_ast::{
    AssertBody, AttributeArg, DeclKind, DimExpr, Expr, ExprKind, File, IndexExpr, TypeDeclBody,
    TypeExpr, TypeExprKind,
};
use crate::registry::builtins::{builtin_constants, builtin_functions};
use crate::registry::error::GraphcalError;
use crate::registry::resolve_types::{
    ResolvedAssertEntry, ResolvedConstEntry, ResolvedFigureEntry, ResolvedLayerEntry,
    ResolvedNodeEntry, ResolvedParamEntry, ResolvedPlotEntry,
};
use crate::syntax::attribute::AttributeName;
use crate::syntax::names::DeclName;
use crate::syntax::span::Span;

// Re-export types and constants from graphcal-registry's resolve_types module.
pub use crate::registry::resolve_types::{
    DeclCategory, ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart, ImportedValueNames,
    ResolvedFile, is_aggregation_fn, is_time_scale_name,
};
pub use crate::syntax::names::ScopedName;

// Re-export items from submodules (crate-internal only).
pub(crate) use deps::collect_scoped_graph_refs;
pub use deps::{collect_graph_ref_names, collect_graph_refs, contains_graph_ref};

// Import helpers from submodules for use within this file.
use deps::{extract_all_refs, extract_const_refs};
use names::parse_expected_fail_args;
use scope::{
    check_no_assert_graph_refs, check_no_pub_index_variant_literals, check_no_runtime_graph_refs,
};

/// Classification of a name in the resolver's scope.
///
/// Used to partition names into const vs runtime sets without relying on casing heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameCategory {
    Const,
    Runtime,
}

fn register_value_namespace_name(
    value_names: &mut HashMap<ScopedName, Span>,
    name: String,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let scoped_name = ScopedName::local(name.clone());
    if let Some(first_span) = value_names.get(&scoped_name) {
        return Err(GraphcalError::DuplicateName {
            name,
            src: src.clone(),
            duplicate: span.into(),
            first: (*first_span).into(),
        });
    }
    value_names.insert(scoped_name, span);
    Ok(())
}

fn check_value_namespace_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &HashMap<ScopedName, (Span, NameCategory)>,
) -> Result<(), GraphcalError> {
    let mut value_names: HashMap<ScopedName, Span> = names
        .iter()
        .map(|(name, (span, _))| (name.clone(), *span))
        .collect();

    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Param(p) => register_value_namespace_name(
                &mut value_names,
                p.name.value.to_string(),
                p.name.span,
                src,
            )?,
            DeclKind::Node(n) => register_value_namespace_name(
                &mut value_names,
                n.name.value.to_string(),
                n.name.span,
                src,
            )?,
            DeclKind::ConstNode(c) => register_value_namespace_name(
                &mut value_names,
                c.name.value.to_string(),
                c.name.span,
                src,
            )?,
            DeclKind::Assert(a) => register_value_namespace_name(
                &mut value_names,
                a.name.value.to_string(),
                a.name.span,
                src,
            )?,
            DeclKind::Plot(p) => register_value_namespace_name(
                &mut value_names,
                p.name.value.to_string(),
                p.name.span,
                src,
            )?,
            DeclKind::Figure(f) => register_value_namespace_name(
                &mut value_names,
                f.name.value.to_string(),
                f.name.span,
                src,
            )?,
            DeclKind::Layer(l) => register_value_namespace_name(
                &mut value_names,
                l.name.value.to_string(),
                l.name.span,
                src,
            )?,
            DeclKind::Type(t) => {
                if let TypeDeclBody::Constructors(members) = &t.body {
                    for member in members {
                        register_value_namespace_name(
                            &mut value_names,
                            member.name.value.to_string(),
                            member.name.span,
                            src,
                        )?;
                    }
                }
            }
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {}
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        }
    }

    Ok(())
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
    runtime_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    const_deps: HashMap<ScopedName, HashSet<ScopedName>>,
    source_order: Vec<(DeclName, DeclCategory)>,
    assert_names: HashSet<DeclName>,
    pub_names: HashSet<DeclName>,
}

/// Collect all local declarations and check for duplicates.
///
/// Returns the collected declarations and the names map for further processing.
#[expect(
    clippy::too_many_lines,
    reason = "complex declaration collection with multiple passes"
)]
fn collect_local_declarations(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &mut HashMap<ScopedName, (Span, NameCategory)>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::registry::builtins::BuiltinFunction>,
) -> Result<CollectedDeclarations, GraphcalError> {
    let mut consts = Vec::new();
    let mut params = Vec::new();
    let mut nodes = Vec::new();
    let mut asserts = Vec::new();
    let mut plots = Vec::new();
    let mut figures = Vec::new();
    let mut layers = Vec::new();
    let mut runtime_deps: HashMap<ScopedName, HashSet<ScopedName>> = HashMap::new();
    let mut const_deps: HashMap<ScopedName, HashSet<ScopedName>> = HashMap::new();
    let mut source_order: Vec<(DeclName, DeclCategory)> = Vec::new();
    let mut assert_names: HashSet<DeclName> = HashSet::new();

    check_value_namespace_collisions(file, src, names)?;

    // Collect names of all visible declarations. Explicit `pub`/`pub(bind)`
    // declarations contribute; params are implicitly visible+bindable under
    // A5 and always contribute.
    let mut pub_names: HashSet<DeclName> = HashSet::new();
    for decl in &file.declarations {
        let is_visible = match &decl.kind {
            DeclKind::Param(_) => true,
            DeclKind::Node(d) => d.visibility.is_public(),
            DeclKind::ConstNode(d) => d.visibility.is_public(),
            DeclKind::BaseDimension(d) => d.visibility.is_public(),
            DeclKind::Dimension(d) => d.visibility.is_public(),
            DeclKind::Unit(d) => d.visibility.is_public(),
            DeclKind::Type(d) => d.visibility.is_public(),
            DeclKind::Index(d) => d.visibility.is_public(),
            DeclKind::Import(d) => d.visibility.is_public(),
            DeclKind::Include(d) => d.visibility.is_public(),
            DeclKind::Dag(d) => d.visibility.is_public(),
            DeclKind::Assert(d) => d.visibility.is_public(),
            DeclKind::Plot(d) => d.visibility.is_public(),
            DeclKind::Figure(d) => d.visibility.is_public(),
            DeclKind::Layer(d) => d.visibility.is_public(),
            DeclKind::Sugar(_) => false,
        };
        if !is_visible {
            continue;
        }
        let Some((name, _)) = decl.kind.name_and_span() else {
            continue;
        };
        pub_names.insert(DeclName::new(name));
    }

    // Validate: required `index`, `type`, `dim` must be `pub(bind)` (V002).
    //
    // Required `param` is excluded from this check: per axiom A5 §4.0,
    // `param` is implicitly V=visible + B=bindable and never carries a
    // visibility annotation. The parser rejects `pub`/`pub(bind)` on
    // `param`.
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Index(idx) if idx.kind.is_required() && !idx.visibility.is_bindable() => {
                return Err(GraphcalError::RequiredItemMustBeBindable {
                    kind: "index".to_string(),
                    name: idx.name.value.to_string(),
                    src: src.clone(),
                    span: idx.name.span.into(),
                });
            }
            DeclKind::Type(t)
                if matches!(t.body, TypeDeclBody::Required) && !t.visibility.is_bindable() =>
            {
                return Err(GraphcalError::RequiredItemMustBeBindable {
                    kind: "type".to_string(),
                    name: t.name.value.to_string(),
                    src: src.clone(),
                    span: t.name.span.into(),
                });
            }
            DeclKind::Dimension(d) if d.definition.is_none() && !d.visibility.is_bindable() => {
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
    let pub_bind_index_names: HashSet<crate::syntax::names::IndexName> = file
        .declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Index(idx) if idx.visibility.is_bindable() && !idx.kind.is_required() => {
                Some(idx.name.value.clone())
            }
            _ => None,
        })
        .collect();

    // First pass: collect all declarations and check for duplicates
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
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                continue;
            }
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        };

        let scoped_name = ScopedName::local(name.clone());
        let name_cat = if is_const {
            NameCategory::Const
        } else {
            NameCategory::Runtime
        };
        names.insert(scoped_name, (name_span, name_cat));

        // Track source order and assert names
        let category = match &decl.kind {
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::ConstNode(_) => DeclCategory::Const,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Assert(_) => {
                assert_names.insert(DeclName::new(name.as_str()));
                DeclCategory::Assert
            }
            DeclKind::Plot(_) => DeclCategory::Plot,
            DeclKind::Figure(_) => DeclCategory::Figure,
            DeclKind::Layer(_) => DeclCategory::Layer,
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                // These declarations are handled earlier (continue'd before reaching here).
                continue;
            }
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        };
        source_order.push((DeclName::new(name.as_str()), category));
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
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {}
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
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
                        src,
                        None,
                    )?;
                    // Check that assert body doesn't reference other assert names via @
                    check_no_assert_graph_refs(body_expr, &assert_names, src)?;
                    // A10(b): public sink kinds travel with the include,
                    // so their bodies must abstract over pub(bind)
                    // indexes. Private sinks are pruned on include, so
                    // their literal mentions cannot orphan anything.
                    if a.visibility.is_public() {
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
                let pub_sink = p.visibility.is_public();
                for encoding in &p.encodings {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &encoding.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
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
                let pub_sink = f.visibility.is_public();
                for field in &f.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
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
                let pub_sink = l.visibility.is_public();
                for field in &l.fields {
                    let (_graph_refs, _const_refs) = extract_all_refs(
                        &field.value,
                        &all_runtime_names,
                        &all_const_names,
                        builtin_consts,
                        builtin_fns,
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
                        src,
                        None,
                    )?;
                    runtime_deps.insert(ScopedName::local(pname.as_str()), graph_refs);
                } else {
                    runtime_deps.insert(ScopedName::local(pname.as_str()), HashSet::new());
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
                    src,
                )?;
                let cname = c.name.value.to_string();
                const_deps.insert(ScopedName::local(cname.as_str()), deps);
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
                    src,
                    Some(&nname),
                )?;
                runtime_deps.insert(ScopedName::local(nname.as_str()), graph_refs);
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
        assert_names,
        pub_names,
    })
}

/// Build const and runtime name sets from the names map using stored categories.
///
/// The returned sets borrow keys from `names`; downstream resolver code uses
/// them for typed `ScopedName` membership checks against AST `GraphRef` /
/// `ConstRef` values directly.
fn build_name_sets(
    names: &HashMap<ScopedName, (Span, NameCategory)>,
) -> (HashSet<&ScopedName>, HashSet<&ScopedName>) {
    let all_const_names: HashSet<&ScopedName> = names
        .iter()
        .filter(|(_, (_, cat))| *cat == NameCategory::Const)
        .map(|(name, _)| name)
        .collect();
    let all_runtime_names: HashSet<&ScopedName> = names
        .iter()
        .filter(|(_, (_, cat))| *cat == NameCategory::Runtime)
        .map(|(name, _)| name)
        .collect();
    (all_const_names, all_runtime_names)
}

/// Result of attribute validation.
struct ValidatedAttributes {
    assumes_map: HashMap<DeclName, Vec<DeclName>>,
    expected_fail_map: HashMap<DeclName, ExpectedFail>,
}

/// Validate attributes and build `assumes_map` / `expected_fail_map`.
#[expect(clippy::too_many_lines, reason = "comprehensive attribute validation")]
fn validate_attributes(
    file: &File,
    src: &NamedSource<Arc<String>>,
    assert_names: &HashSet<DeclName>,
) -> Result<ValidatedAttributes, GraphcalError> {
    let mut assumes_map: HashMap<DeclName, Vec<DeclName>> = HashMap::new();
    let mut expected_fail_map: HashMap<DeclName, ExpectedFail> = HashMap::new();

    for decl in &file.declarations {
        let decl_name: Option<DeclName> = match &decl.kind {
            DeclKind::Param(p) => Some(p.name.value.clone()),
            DeclKind::Node(n) => Some(n.name.value.clone()),
            DeclKind::ConstNode(c) => Some(c.name.value.clone()),
            DeclKind::Assert(a) => Some(a.name.value.clone()),
            DeclKind::Plot(p) => Some(p.name.value.clone()),
            DeclKind::Figure(f) => Some(f.name.value.clone()),
            _ => None,
        };
        for attr in &decl.attributes {
            let attr_name_str = attr.name.name.as_str();
            let attr_name = attr_name_str.parse::<AttributeName>().map_err(|err| {
                GraphcalError::UnknownAttribute {
                    name: err.into_raw(),
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
                        DeclKind::Type(_) => Some("type"),
                        DeclKind::Index(_) => Some("cat/range"),
                        DeclKind::Import(_) => Some("import"),
                        DeclKind::Include(_) => Some("include"),
                        DeclKind::Dag(_) => Some("dag"),
                        DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
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
                        let ident = match arg {
                            AttributeArg::Path { segments, .. } if segments.len() == 1 => {
                                segments.first()
                            }
                            AttributeArg::Path { .. } | AttributeArg::Group { .. } => {
                                return Err(GraphcalError::EvalError {
                                    message:
                                        "`#[assumes(...)]` arguments must be plain identifiers"
                                            .to_string(),
                                    src: src.clone(),
                                    span: arg.span().into(),
                                });
                            }
                        };
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
                                .entry(DeclName::new(arg_name))
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
                        DeclKind::Type(_) => "type",
                        DeclKind::Index(_) => "cat/range",
                        DeclKind::Import(_) => "import",
                        DeclKind::Include(_) => "include",
                        DeclKind::Dag(_) => "dag",
                        DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
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
    reason = "exhaustive declaration-kind validation is clearer in one pass"
)]
fn validate_private_in_public(
    file: &File,
    src: &NamedSource<Arc<String>>,
    pub_names: &HashSet<DeclName>,
) -> Result<(), GraphcalError> {
    use crate::desugar::resolved_ast::IndexDeclKind;

    // Collect all locally-declared type-system names (dims, indexes, types) with their spans.
    let mut local_type_names: HashMap<String, Span> = HashMap::new();
    for decl in &file.declarations {
        let (name, span) = match &decl.kind {
            DeclKind::BaseDimension(d) => (d.name.value.to_string(), d.name.span),
            DeclKind::Dimension(d) => (d.name.value.to_string(), d.name.span),
            DeclKind::Index(idx) => (idx.name.value.to_string(), idx.name.span),
            DeclKind::Type(t) => (t.name.value.to_string(), t.name.span),
            _ => continue,
        };
        local_type_names.insert(name, span);
    }

    // If there are no local type-system names, nothing to check.
    if local_type_names.is_empty() {
        return Ok(());
    }

    let emit = |pub_kind: &str,
                pub_name: String,
                pub_span: Span,
                refs: &[(crate::syntax::names::NamePath, Span)]|
     -> Result<(), GraphcalError> {
        for (ref_path, ref_span) in refs {
            // Only a bare (single-segment) path can name a local type-system
            // declaration; qualified refs belong to another module.
            let Some(ref_name) = ref_path.as_bare() else {
                continue;
            };
            if local_type_names.contains_key(ref_name.as_str())
                && !pub_names.contains(ref_name.as_str())
            {
                return Err(GraphcalError::PrivateInPublic {
                    pub_kind: pub_kind.to_string(),
                    pub_name,
                    ref_kind: ref_kind_for(file, ref_name.as_str()).to_string(),
                    ref_name: ref_name.to_string(),
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
        let is_visible = match &decl.kind {
            DeclKind::Param(_) => true,
            DeclKind::Node(d) => d.visibility.is_public(),
            DeclKind::ConstNode(d) => d.visibility.is_public(),
            DeclKind::BaseDimension(d) => d.visibility.is_public(),
            DeclKind::Dimension(d) => d.visibility.is_public(),
            DeclKind::Unit(d) => d.visibility.is_public(),
            DeclKind::Type(d) => d.visibility.is_public(),
            DeclKind::Index(d) => d.visibility.is_public(),
            DeclKind::Import(d) => d.visibility.is_public(),
            DeclKind::Include(d) => d.visibility.is_public(),
            DeclKind::Dag(d) => d.visibility.is_public(),
            DeclKind::Assert(d) => d.visibility.is_public(),
            DeclKind::Plot(d) => d.visibility.is_public(),
            DeclKind::Figure(d) => d.visibility.is_public(),
            DeclKind::Layer(d) => d.visibility.is_public(),
            DeclKind::Sugar(_) => false,
        };
        if !is_visible {
            continue;
        }

        let mut refs: Vec<(crate::syntax::names::NamePath, Span)> = Vec::new();
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
                // Each constructor payload field type is part of the
                // type's signature for A9 dependency tracking.
                if let TypeDeclBody::Constructors(members) = &t.body {
                    for member in members {
                        if let Some(fields) = &member.payload {
                            for field in fields {
                                collect_type_refs(&field.type_ann, &mut refs);
                            }
                        }
                    }
                }
                ("type", t.name.value.to_string())
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

/// Recursively collect type-system references from a [`TypeExpr`].
fn collect_type_refs(type_expr: &TypeExpr, refs: &mut Vec<(crate::syntax::names::NamePath, Span)>) {
    match &type_expr.kind {
        TypeExprKind::DimExpr(dim_expr) => collect_dim_refs(dim_expr, refs),
        TypeExprKind::Indexed { base, indexes } => {
            collect_type_refs(base, refs);
            for idx in indexes {
                if let IndexExpr::Name(path) = idx {
                    refs.push((path.value.clone(), path.span));
                }
            }
        }
        TypeExprKind::TypeApplication { name, type_args } => {
            refs.push((name.value.clone(), name.span));
            for arg in type_args {
                collect_type_refs(arg, refs);
            }
        }
        TypeExprKind::DatetimeApplication { type_args } => {
            // No top-level name to record — `Datetime` is built-in. Recurse
            // into the args so any user-defined name reachable from the time
            // scale expression is still collected.
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

/// Collect every term name in a [`DimExpr`] as a `(name, span)` reference.
fn collect_dim_refs(dim_expr: &DimExpr, refs: &mut Vec<(crate::syntax::names::NamePath, Span)>) {
    for item in &dim_expr.terms {
        refs.push((item.term.name.value.clone(), item.term.span));
    }
}

/// Classify the owning declaration of a referenced name for diagnostic messages.
fn ref_kind_for(file: &File, ref_name: &str) -> &'static str {
    match file
        .declarations
        .iter()
        .find(|d| match &d.kind {
            DeclKind::BaseDimension(bd) => bd.name.value.as_str() == ref_name,
            DeclKind::Dimension(d) => d.name.value.as_str() == ref_name,
            DeclKind::Index(idx) => idx.name.value.as_str() == ref_name,
            DeclKind::Type(t) => t.name.value.as_str() == ref_name,
            _ => false,
        })
        .map(|d| &d.kind)
    {
        Some(DeclKind::BaseDimension(_) | DeclKind::Dimension(_)) => "dim",
        Some(DeclKind::Index(_)) => "index",
        Some(DeclKind::Type(_)) => "type",
        _ => "item",
    }
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

/// Resolve names, detect duplicates, and extract dependencies.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, or
/// arity mismatches are found.
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
/// Returns a [`GraphcalError`] if duplicate names, unknown references, or
/// arity mismatches are found.
#[expect(
    clippy::too_many_lines,
    reason = "complex resolution logic with multiple passes"
)]
pub(crate) fn resolve_with_imports(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<ResolvedFile, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<ScopedName, (Span, NameCategory)> = HashMap::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    // The `imported.*` compatibility entries pass bare names; the at-rest dep map
    // produced upstream already classifies them as locals.
    for (name, _, _, span) in &imported.consts {
        names.insert(
            ScopedName::local(name.as_str()),
            (*span, NameCategory::Const),
        );
    }
    for (name, _, _, span) in &imported.params {
        names.insert(
            ScopedName::local(name.as_str()),
            (*span, NameCategory::Runtime),
        );
    }
    for (name, _, _, span) in &imported.nodes {
        names.insert(
            ScopedName::local(name.as_str()),
            (*span, NameCategory::Runtime),
        );
    }
    for (name, _, span) in &imported.asserts {
        names.insert(
            ScopedName::local(name.as_str()),
            (*span, NameCategory::Runtime),
        );
    }

    // Collect local declarations
    let local = collect_local_declarations(file, src, &mut names, builtin_consts, builtin_fns)?;

    // Build name sets for dependency extraction
    let (all_const_names, all_runtime_names) = build_name_sets(&names);

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names: HashSet<DeclName> = HashSet::new();
    for (name, _, _) in &imported.asserts {
        all_assert_names.insert(DeclName::new(name.as_str()));
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

    // Extract dependencies for imported declarations so the DAG is complete.
    // Without this, imported nodes' @-references are invisible to the topological sort,
    // causing evaluation-order errors (Bug 2).
    let mut runtime_deps = local.runtime_deps;
    let mut const_deps = local.const_deps;

    for (name, _, expr, _) in &imported.consts {
        let deps = extract_const_refs(expr, &all_const_names, builtin_consts, builtin_fns, src)?;
        const_deps.insert(ScopedName::local(name.as_str()), deps);
    }
    for (name, _, expr, _) in &imported.params {
        let (graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            builtin_consts,
            builtin_fns,
            src,
            None,
        )?;
        runtime_deps.insert(ScopedName::local(name.as_str()), graph_refs);
    }
    for (name, _, expr, _) in &imported.nodes {
        let (graph_refs, _const_refs) = extract_all_refs(
            expr,
            &all_runtime_names,
            &all_const_names,
            builtin_consts,
            builtin_fns,
            src,
            Some(name.as_str()),
        )?;
        runtime_deps.insert(ScopedName::local(name.as_str()), graph_refs);
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
    let mut all_source_order: Vec<(DeclName, DeclCategory)> = Vec::new();
    for (name, _, _, _) in &imported.consts {
        all_source_order.push((DeclName::new(name.as_str()), DeclCategory::Const));
    }
    for (name, _, _, _) in &imported.params {
        all_source_order.push((DeclName::new(name.as_str()), DeclCategory::Param));
    }
    for (name, _, _, _) in &imported.nodes {
        all_source_order.push((DeclName::new(name.as_str()), DeclCategory::Node));
    }
    for (name, _, _) in &imported.asserts {
        all_source_order.push((DeclName::new(name.as_str()), DeclCategory::Assert));
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
/// Returns a [`GraphcalError`] if duplicate names, unknown references, or
/// arity mismatches are found.
pub(crate) fn resolve_with_imported_values(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedValueNames,
) -> Result<ResolvedFile, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();

    let mut names: HashMap<ScopedName, (Span, NameCategory)> = HashMap::new();

    // Pre-populate with imported names. The scope here mixes typed imported
    // `ScopedName`s (which may be `Qualified` for module aliases) with
    // local declarations; both share the same key type so subsequent lookups
    // pattern-match against AST `GraphRef`/`ConstRef` values directly.
    for (name, span) in &imported.const_names {
        names.insert(name.clone(), (*span, NameCategory::Const));
    }
    for (name, span) in &imported.param_names {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }
    for (name, span) in &imported.node_names {
        names.insert(name.clone(), (*span, NameCategory::Runtime));
    }
    for (name, span) in &imported.assert_names {
        names.insert(
            ScopedName::local(name.as_str()),
            (*span, NameCategory::Runtime),
        );
    }

    // Collect local declarations
    let local = collect_local_declarations(file, src, &mut names, builtin_consts, builtin_fns)?;

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names: HashSet<DeclName> = HashSet::new();
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

mod deps;
pub(crate) mod names;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use crate::desugar::desugared_ast::{
    AssertBody, AttributeArg, DeclKind, DimExpr, Expr, ExprKind, File, IndexExpr, TypeDeclBody,
    TypeExpr, TypeExprKind,
};
use crate::registry::error::GraphcalError;
use crate::registry::prelude::{
    PRELUDE_BUILTIN_TYPE_NAMES, PRELUDE_DIMENSION_NAMES, PRELUDE_UNIT_NAMES,
};
use crate::registry::resolve_types::{
    ResolvedAssertEntry, ResolvedConstEntry, ResolvedFigureEntry, ResolvedLayerEntry,
    ResolvedNodeEntry, ResolvedParamEntry, ResolvedPlotEntry,
};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;

// Re-export types and constants from graphcal-registry's resolve_types module.
pub use crate::registry::resolve_types::{
    DeclCategory, ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart, ImportedValueNames,
    ParsedExpectedFail, ParsedExpectedFailKey, ParsedExpectedFailKeyPart, ResolvedFile,
    is_time_scale_name,
};
pub use crate::syntax::module_name::ScopedName;

// Re-export items from submodules (crate-internal only).
pub use deps::{collect_graph_ref_names, collect_graph_refs, contains_graph_ref};

// Import helpers from submodules for use within this file.
use names::parse_expected_fail_args;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExclusiveUniverse {
    Type,
    Index,
    Value,
}

type ExclusiveUniverseBinding = (ExclusiveUniverse, Span);

fn register_exclusive_universe_name(
    occupied: &mut HashMap<NameAtom, ExclusiveUniverseBinding>,
    atom: &NameAtom,
    universe: ExclusiveUniverse,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    occupied
        .insert(atom.clone(), (universe, span))
        .map_or(Ok(()), |first| {
            Err(GraphcalError::DuplicateName {
                name: atom.to_string(),
                src: src.clone(),
                duplicate: span.into(),
                first: first.1.into(),
            })
        })
}

fn check_builtin_name_shadowing(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        let shadowed = match &decl.kind {
            DeclKind::BaseDimension(d) if is_builtin_type_name(d.name.value.as_str()) => {
                Some(("dimension", d.name.value.to_string(), d.name.span))
            }
            DeclKind::Dimension(d) if is_builtin_type_name(d.name.value.as_str()) => {
                Some(("dimension", d.name.value.to_string(), d.name.span))
            }
            DeclKind::Type(t) if is_builtin_type_name(t.name.value.as_str()) => {
                Some(("type", t.name.value.to_string(), t.name.span))
            }
            DeclKind::Index(i) if is_builtin_type_name(i.name.value.as_str()) => {
                Some(("index", i.name.value.to_string(), i.name.span))
            }
            DeclKind::Unit(u) if PRELUDE_UNIT_NAMES.contains(&u.name.value.as_str()) => {
                Some(("unit", u.name.value.to_string(), u.name.span))
            }
            _ => None,
        };

        if let Some((kind, name, span)) = shadowed {
            return Err(GraphcalError::BuiltinNameShadowed {
                kind,
                name,
                src: src.clone(),
                span: span.into(),
            });
        }
    }

    Ok(())
}

fn is_builtin_type_name(name: &str) -> bool {
    PRELUDE_DIMENSION_NAMES.contains(&name) || PRELUDE_BUILTIN_TYPE_NAMES.contains(&name)
}

fn check_exclusive_universe_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &HashMap<ScopedName, Span>,
) -> Result<(), GraphcalError> {
    let mut occupied = names
        .iter()
        .filter(|(name, _)| !name.is_qualified())
        .map(|(name, span)| {
            (
                DeclName::expect_valid(name.member()).into_atom(),
                (ExclusiveUniverse::Value, *span),
            )
        })
        .collect::<HashMap<_, _>>();

    for (atom, universe, span) in file
        .declarations
        .iter()
        .filter_map(|decl| exclusive_universe_decl(&decl.kind))
    {
        register_exclusive_universe_name(&mut occupied, atom, universe, span, src)?;
    }

    Ok(())
}

fn exclusive_universe_decl(decl: &DeclKind) -> Option<(&NameAtom, ExclusiveUniverse, Span)> {
    match decl {
        DeclKind::Param(p) => Some((p.name.value.atom(), ExclusiveUniverse::Value, p.name.span)),
        DeclKind::Node(n) => Some((n.name.value.atom(), ExclusiveUniverse::Value, n.name.span)),
        DeclKind::ConstNode(c) => {
            Some((c.name.value.atom(), ExclusiveUniverse::Value, c.name.span))
        }
        DeclKind::Assert(a) => Some((a.name.value.atom(), ExclusiveUniverse::Value, a.name.span)),
        DeclKind::Plot(p) => Some((p.name.value.atom(), ExclusiveUniverse::Value, p.name.span)),
        DeclKind::Figure(f) => Some((f.name.value.atom(), ExclusiveUniverse::Value, f.name.span)),
        DeclKind::Layer(l) => Some((l.name.value.atom(), ExclusiveUniverse::Value, l.name.span)),
        DeclKind::Dag(d) => Some((d.name.value.atom(), ExclusiveUniverse::Value, d.name.span)),
        DeclKind::BaseDimension(d) => {
            Some((d.name.value.atom(), ExclusiveUniverse::Type, d.name.span))
        }
        DeclKind::Dimension(d) => Some((d.name.value.atom(), ExclusiveUniverse::Type, d.name.span)),
        DeclKind::Type(t) => Some((t.name.value.atom(), ExclusiveUniverse::Type, t.name.span)),
        DeclKind::Index(i) => Some((i.name.value.atom(), ExclusiveUniverse::Index, i.name.span)),
        DeclKind::Unit(_) | DeclKind::Import(_) | DeclKind::Include(_) => None,
        DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
    }
}

fn check_value_namespace_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &HashMap<ScopedName, Span>,
) -> Result<(), GraphcalError> {
    let mut value_names: HashMap<ScopedName, Span> = names.clone();

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
    names: &mut HashMap<ScopedName, Span>,
) -> Result<CollectedDeclarations, GraphcalError> {
    let mut consts = Vec::new();
    let mut params = Vec::new();
    let mut nodes = Vec::new();
    let mut asserts = Vec::new();
    let mut plots = Vec::new();
    let mut figures = Vec::new();
    let mut layers = Vec::new();
    let mut source_order: Vec<(DeclName, DeclCategory)> = Vec::new();
    let mut assert_names: HashSet<DeclName> = HashSet::new();

    check_builtin_name_shadowing(file, src)?;
    check_exclusive_universe_collisions(file, src, names)?;
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
        pub_names.insert(DeclName::expect_valid(name));
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

    // First pass: collect all declarations and check for duplicates
    for decl in &file.declarations {
        // Dimension and Unit declarations are handled by the registry, not the resolver
        let (name, name_span) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.to_string(), p.name.span),
            DeclKind::Node(n) => (n.name.value.to_string(), n.name.span),
            DeclKind::ConstNode(c) => (c.name.value.to_string(), c.name.span),
            DeclKind::Assert(a) => (a.name.value.to_string(), a.name.span),
            DeclKind::Plot(p) => (p.name.value.to_string(), p.name.span),
            DeclKind::Figure(f) => (f.name.value.to_string(), f.name.span),
            DeclKind::Layer(l) => (l.name.value.to_string(), l.name.span),
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
        names.insert(scoped_name, name_span);

        // Track source order and assert names
        let category = match &decl.kind {
            DeclKind::Param(_) => DeclCategory::Param,
            DeclKind::ConstNode(_) => DeclCategory::Const,
            DeclKind::Node(_) => DeclCategory::Node,
            DeclKind::Assert(_) => {
                assert_names.insert(DeclName::expect_valid(name.as_str()));
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
        source_order.push((DeclName::expect_valid(name.as_str()), category));
    }

    // Second pass: collect declaration entries. Reference validation and
    // dependency extraction happen after HIR lowering — this pass only
    // gathers declaration bodies in source order.
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
                asserts.push(ResolvedAssertEntry {
                    name: a.name.value.to_string(),
                    body: a.body.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Plot(p) => {
                plots.push(ResolvedPlotEntry {
                    name: p.name.value.to_string(),
                    decl: p.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Figure(f) => {
                figures.push(ResolvedFigureEntry {
                    name: f.name.value.to_string(),
                    decl: f.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Layer(l) => {
                layers.push(ResolvedLayerEntry {
                    name: l.name.value.to_string(),
                    decl: l.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Param(p) => {
                params.push(ResolvedParamEntry {
                    name: p.name.value.to_string(),
                    default_expr: p.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::ConstNode(c) => {
                consts.push(ResolvedConstEntry {
                    name: c.name.value.to_string(),
                    expr: c.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Node(n) => {
                nodes.push(ResolvedNodeEntry {
                    name: n.name.value.to_string(),
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
        source_order,
        assert_names,
        pub_names,
    })
}

/// Result of attribute validation.
struct ValidatedAttributes {
    assumes_map: HashMap<DeclName, Vec<DeclName>>,
    expected_fail_map: HashMap<DeclName, ParsedExpectedFail>,
    /// Plot names carrying `#[hidden]`: evaluated and referenceable from
    /// figures/layers, but excluded from standalone output (#847).
    hidden_plots: HashSet<DeclName>,
}

/// Validate attributes and build `assumes_map` / `expected_fail_map`.
#[expect(clippy::too_many_lines, reason = "comprehensive attribute validation")]
fn validate_attributes(
    file: &File,
    src: &NamedSource<Arc<String>>,
    assert_names: &HashSet<DeclName>,
) -> Result<ValidatedAttributes, GraphcalError> {
    let mut assumes_map: HashMap<DeclName, Vec<DeclName>> = HashMap::new();
    let mut expected_fail_map: HashMap<DeclName, ParsedExpectedFail> = HashMap::new();
    let mut hidden_plots: HashSet<DeclName> = HashSet::new();

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
                            AttributeArg::Path { .. }
                            | AttributeArg::RangeStep { .. }
                            | AttributeArg::Group { .. } => {
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
                                .entry(DeclName::expect_valid(arg_name))
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
                AttributeName::Hidden => {
                    // #[hidden] is plot-only: figures/layers cannot be
                    // referenced by anything, so hiding one is equivalent to
                    // deleting it; other declarations have no display axis.
                    let kind = match &decl.kind {
                        DeclKind::Plot(_) => None,
                        DeclKind::Param(_) => Some("param"),
                        DeclKind::ConstNode(_) => Some("const node"),
                        DeclKind::Node(_) => Some("node"),
                        DeclKind::Assert(_) => Some("assert"),
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
                        return Err(GraphcalError::InvalidHiddenTarget {
                            kind: kind.to_string(),
                            src: src.clone(),
                            span: attr.span.into(),
                        });
                    }
                    if !attr.args.is_empty() {
                        return Err(GraphcalError::EvalError {
                            message: "`#[hidden]` takes no arguments".to_string(),
                            src: src.clone(),
                            span: attr.span.into(),
                        });
                    }
                    if let Some(ref dname) = decl_name {
                        hidden_plots.insert(dname.clone());
                    }
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
        hidden_plots,
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
    use crate::desugar::desugared_ast::IndexDeclKind;

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

/// Collect declaration entries and validate declaration shells.
///
/// Reference resolution and dependency extraction happen in HIR lowering;
/// this pass checks duplicates, visibility rules, and attributes.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names or invalid declaration
/// shells are found.
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
/// Returns a [`GraphcalError`] if duplicate names or invalid declaration
/// shells are found.
pub(crate) fn resolve_with_imports(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<ResolvedFile, GraphcalError> {
    let mut names: HashMap<ScopedName, Span> = HashMap::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    for (name, _, _, span) in &imported.consts {
        names.insert(ScopedName::local(name.as_str()), *span);
    }
    for (name, _, _, span) in &imported.params {
        names.insert(ScopedName::local(name.as_str()), *span);
    }
    for (name, _, _, span) in &imported.nodes {
        names.insert(ScopedName::local(name.as_str()), *span);
    }
    for (name, _, span) in &imported.asserts {
        names.insert(ScopedName::local(name.as_str()), *span);
    }

    // Collect local declarations
    let local = collect_local_declarations(file, src, &mut names)?;

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names: HashSet<DeclName> = HashSet::new();
    for (name, _, _) in &imported.asserts {
        all_assert_names.insert(DeclName::expect_valid(name.as_str()));
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

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
        all_source_order.push((DeclName::expect_valid(name.as_str()), DeclCategory::Const));
    }
    for (name, _, _, _) in &imported.params {
        all_source_order.push((DeclName::expect_valid(name.as_str()), DeclCategory::Param));
    }
    for (name, _, _, _) in &imported.nodes {
        all_source_order.push((DeclName::expect_valid(name.as_str()), DeclCategory::Node));
    }
    for (name, _, _) in &imported.asserts {
        all_source_order.push((DeclName::expect_valid(name.as_str()), DeclCategory::Assert));
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
        source_order: all_source_order,
        assert_names: all_assert_names,
        assumes_map: validated.assumes_map,
        expected_fail: validated.expected_fail_map,
        hidden_plots: validated.hidden_plots,
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
    let mut names: HashMap<ScopedName, Span> = HashMap::new();

    // Pre-populate with imported names. The scope here mixes typed imported
    // `ScopedName`s (which may be `Qualified` for module aliases) with
    // local declarations; both share the same key type so the value-namespace
    // collision check sees the complete scope.
    for (name, span) in &imported.const_names {
        names.insert(name.clone(), *span);
    }
    for (name, span) in &imported.param_names {
        names.insert(name.clone(), *span);
    }
    for (name, span) in &imported.node_names {
        names.insert(name.clone(), *span);
    }
    for (name, span) in &imported.assert_names {
        names.insert(ScopedName::local(name.as_str()), *span);
    }
    for (name, span) in &imported.plot_names {
        names.insert(name.clone(), *span);
    }

    // Collect local declarations
    let local = collect_local_declarations(file, src, &mut names)?;

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
        source_order: local.source_order,
        assert_names: all_assert_names,
        assumes_map: validated.assumes_map,
        expected_fail: validated.expected_fail_map,
        hidden_plots: validated.hidden_plots,
        pub_names: local.pub_names,
    })
}

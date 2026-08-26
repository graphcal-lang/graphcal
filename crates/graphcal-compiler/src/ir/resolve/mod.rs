pub mod attribute_validation;
mod deps;
#[cfg(test)]
mod formal_conformance;
pub mod include_selection;
pub(crate) mod names;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use super::required_bindability::{
    self, InterfaceDecl, NominalKind, Requirement, Violation as RequiredBindabilityViolation,
};

use crate::desugar::desugared_ast::{
    AssertBody, DeclKind, DimExpr, Expr, ExprKind, File, IndexExpr, TypeDeclBody, TypeExpr,
    TypeExprKind,
};
use crate::registry::error::GraphcalError;
use crate::registry::reserved_name::{ReservedNameNamespace, validate_reserved_name};
use crate::registry::resolve_types::{
    CollectedAssertEntry, CollectedConstEntry, CollectedExpectedFail, CollectedFigureEntry,
    CollectedLayerEntry, CollectedNodeEntry, CollectedParamEntry, CollectedPlotEntry,
    ExternalDeclSurface,
};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::span::Span;

// Re-export types and constants from graphcal-registry's resolve_types module.
pub(crate) use crate::registry::resolve_types::CollectedFile;
pub use crate::registry::resolve_types::{
    DeclCategory, ExpectedFail, ExpectedFailKey, ExpectedFailKeyPart, ImportedValueNames,
    ParsedExpectedFail,
};
pub use crate::syntax::module_name::ScopedName;

// Re-export items from submodules (crate-internal only).
pub(crate) use deps::contains_graph_ref;

// Import helpers from submodules for use within this file.
use names::parse_expected_fail_args;

fn register_value_namespace_name(
    value_names: &mut HashMap<ScopedName, Span>,
    name: &NameAtom,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let scoped_name = ScopedName::from(name.clone());
    if let Some(first_span) = value_names.get(&scoped_name) {
        return Err(GraphcalError::DuplicateName {
            name: name.to_string(),
            src: src.clone(),
            duplicate: span.into(),
            first: (*first_span).into(),
        });
    }
    value_names.insert(scoped_name, span);
    Ok(())
}

fn register_exclusive_universe_name(
    occupied: &mut HashMap<NameAtom, Span>,
    atom: &NameAtom,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    occupied.insert(atom.clone(), span).map_or(Ok(()), |first| {
        Err(GraphcalError::DuplicateName {
            name: atom.to_string(),
            src: src.clone(),
            duplicate: span.into(),
            first: first.into(),
        })
    })
}

fn check_builtin_name_shadowing(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        let introduced = match &decl.kind {
            DeclKind::BaseDimension(d) => Some((
                ReservedNameNamespace::TypeSystem,
                "dimension",
                d.name.value.atom(),
                d.name.span,
            )),
            DeclKind::Dimension(d) => Some((
                ReservedNameNamespace::TypeSystem,
                "dimension",
                d.name.value.atom(),
                d.name.span,
            )),
            DeclKind::Type(t) => Some((
                ReservedNameNamespace::TypeSystem,
                "type",
                t.name.value.atom(),
                t.name.span,
            )),
            DeclKind::Index(i) => Some((
                ReservedNameNamespace::TypeSystem,
                "index",
                i.name.value.atom(),
                i.name.span,
            )),
            DeclKind::Unit(u) => Some((
                ReservedNameNamespace::Unit,
                "unit",
                u.name.value.atom(),
                u.name.span,
            )),
            DeclKind::Param(p) => Some((
                ReservedNameNamespace::GraphValue,
                "param",
                p.name.value.atom(),
                p.name.span,
            )),
            DeclKind::Node(n) => Some((
                ReservedNameNamespace::GraphValue,
                "node",
                n.name.value.atom(),
                n.name.span,
            )),
            DeclKind::ConstNode(c) => Some((
                ReservedNameNamespace::GraphValue,
                "const node",
                c.name.value.atom(),
                c.name.span,
            )),
            DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_)
            | DeclKind::Sugar(_) => None,
        };

        if let Some((namespace, kind, name, span)) = introduced
            && validate_reserved_name(namespace, name).is_err()
        {
            return Err(GraphcalError::BuiltinNameShadowed {
                kind,
                name: name.to_string(),
                src: src.clone(),
                span: span.into(),
            });
        }
    }

    Ok(())
}

fn check_imported_graph_value_names(
    imported: &ImportedValueNames,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    imported
        .const_names
        .iter()
        .chain(&imported.param_names)
        .chain(&imported.node_names)
        .filter(|(name, _)| !name.is_qualified())
        .try_for_each(|(name, span)| {
            let atom = name.member().atom();
            validate_reserved_name(ReservedNameNamespace::GraphValue, atom).map_err(|_| {
                GraphcalError::BuiltinNameShadowed {
                    kind: "graph-value alias",
                    name: atom.to_string(),
                    src: src.clone(),
                    span: (*span).into(),
                }
            })
        })
}

fn check_exclusive_universe_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &HashMap<ScopedName, Span>,
) -> Result<(), GraphcalError> {
    let mut occupied = names
        .iter()
        .filter(|(name, _)| !name.is_qualified())
        .map(|(name, span)| (name.member().atom().clone(), *span))
        .collect::<HashMap<_, _>>();

    for (atom, span) in file
        .declarations
        .iter()
        .filter_map(|decl| exclusive_universe_decl(&decl.kind))
    {
        register_exclusive_universe_name(&mut occupied, atom, span, src)?;
    }

    Ok(())
}

fn exclusive_universe_decl(decl: &DeclKind) -> Option<(&NameAtom, Span)> {
    match decl {
        DeclKind::Param(p) => Some((p.name.value.atom(), p.name.span)),
        DeclKind::Node(n) => Some((n.name.value.atom(), n.name.span)),
        DeclKind::ConstNode(c) => Some((c.name.value.atom(), c.name.span)),
        DeclKind::Assert(a) => Some((a.name.value.atom(), a.name.span)),
        DeclKind::Plot(p) => Some((p.name.value.atom(), p.name.span)),
        DeclKind::Figure(f) => Some((f.name.value.atom(), f.name.span)),
        DeclKind::Layer(l) => Some((l.name.value.atom(), l.name.span)),
        DeclKind::Dag(d) => Some((d.name.value.atom(), d.name.span)),
        DeclKind::BaseDimension(d) => Some((d.name.value.atom(), d.name.span)),
        DeclKind::Dimension(d) => Some((d.name.value.atom(), d.name.span)),
        DeclKind::Type(t) => Some((t.name.value.atom(), t.name.span)),
        DeclKind::Index(i) => Some((i.name.value.atom(), i.name.span)),
        DeclKind::Unit(_)
        | DeclKind::Import(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Include(_) => None,
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
                p.name.value.atom(),
                p.name.span,
                src,
            )?,
            DeclKind::Node(n) => register_value_namespace_name(
                &mut value_names,
                n.name.value.atom(),
                n.name.span,
                src,
            )?,
            DeclKind::ConstNode(c) => register_value_namespace_name(
                &mut value_names,
                c.name.value.atom(),
                c.name.span,
                src,
            )?,
            DeclKind::Assert(a) => register_value_namespace_name(
                &mut value_names,
                a.name.value.atom(),
                a.name.span,
                src,
            )?,
            DeclKind::Plot(p) => register_value_namespace_name(
                &mut value_names,
                p.name.value.atom(),
                p.name.span,
                src,
            )?,
            DeclKind::Figure(f) => register_value_namespace_name(
                &mut value_names,
                f.name.value.atom(),
                f.name.span,
                src,
            )?,
            DeclKind::Layer(l) => register_value_namespace_name(
                &mut value_names,
                l.name.value.atom(),
                l.name.span,
                src,
            )?,
            DeclKind::Type(t) => {
                if let TypeDeclBody::Constructors(members) = &t.body {
                    for member in members {
                        register_value_namespace_name(
                            &mut value_names,
                            member.name.value.atom(),
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
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {}
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        }
    }

    Ok(())
}

/// Result of collecting local declarations from the AST.
struct CollectedDeclarations {
    consts: Vec<CollectedConstEntry>,
    params: Vec<CollectedParamEntry>,
    nodes: Vec<CollectedNodeEntry>,
    asserts: Vec<CollectedAssertEntry>,
    plots: Vec<CollectedPlotEntry>,
    figures: Vec<CollectedFigureEntry>,
    layers: Vec<CollectedLayerEntry>,
    source_order: Vec<(DeclName, DeclCategory)>,
    assert_names: HashSet<DeclName>,
    external_surface: ExternalDeclSurface,
}

/// Project one desugared declaration onto the small semantic state used by
/// V002. Declarations that cannot be required or externally supplied are
/// outside this rule's domain.
fn required_bindability_interface(decl: &DeclKind) -> Option<(InterfaceDecl, &str, Span)> {
    let requirement_from_missing_definition = |missing| {
        if missing {
            Requirement::Required
        } else {
            Requirement::Defaulted
        }
    };

    match decl {
        DeclKind::Param(param) => Some((
            InterfaceDecl::InputPort {
                requirement: requirement_from_missing_definition(param.value.is_none()),
            },
            param.name.value.as_str(),
            param.name.span,
        )),
        DeclKind::Index(index) => Some((
            InterfaceDecl::Nominal {
                kind: NominalKind::Index,
                visibility: index.visibility,
                requirement: requirement_from_missing_definition(index.kind.is_required()),
            },
            index.name.value.as_str(),
            index.name.span,
        )),
        DeclKind::Type(type_decl) => Some((
            InterfaceDecl::Nominal {
                kind: NominalKind::Type,
                visibility: type_decl.visibility,
                requirement: requirement_from_missing_definition(matches!(
                    type_decl.body,
                    TypeDeclBody::Required
                )),
            },
            type_decl.name.value.as_str(),
            type_decl.name.span,
        )),
        DeclKind::Dimension(dimension) => Some((
            InterfaceDecl::Nominal {
                kind: NominalKind::Dimension,
                visibility: dimension.visibility,
                requirement: requirement_from_missing_definition(dimension.definition.is_none()),
            },
            dimension.name.value.as_str(),
            dimension.name.span,
        )),
        _ => None,
    }
}

/// Validate that every required interface declaration can be supplied from
/// outside its module. This is the production implementation of V002.
fn validate_required_bindability(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    file.declarations
        .iter()
        .filter_map(|decl| required_bindability_interface(&decl.kind))
        .try_for_each(|(interface, name, span)| {
            required_bindability::validate(interface).map_err(|violation| match violation {
                RequiredBindabilityViolation::RequiredMustBeBindable { kind } => {
                    GraphcalError::RequiredItemMustBeBindable {
                        kind: kind.to_string(),
                        name: name.to_string(),
                        src: src.clone(),
                        span: span.into(),
                    }
                }
            })
        })
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

    // Classify the externally addressable surface without treating `param`
    // input ports as ordinary exports. Explicit `pub`/`pub(bind)` declarations
    // are exports; the `param` kind itself declares a named input port.
    let mut external_surface = ExternalDeclSurface::default();
    for decl in &file.declarations {
        let Some((name, _)) = decl.kind.name_and_span() else {
            continue;
        };
        let name = DeclName::expect_valid(name);
        match &decl.kind {
            DeclKind::Param(_) => {
                external_surface.insert_input_port(name);
            }
            DeclKind::Node(d) | DeclKind::ConstNode(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::BaseDimension(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Dimension(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Unit(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Type(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Index(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Dag(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Assert(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Plot(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Figure(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Layer(d) if d.visibility.is_public() => {
                external_surface.insert_explicit_export(name);
            }
            DeclKind::Node(_)
            | DeclKind::ConstNode(_)
            | DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_)
            | DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Sugar(_) => {}
        }
    }

    validate_required_bindability(file, src)?;

    // First pass: collect all declarations and check for duplicates
    for decl in &file.declarations {
        // Dimension and Unit declarations are handled by the registry, not the resolver
        let (name, name_span) = match &decl.kind {
            DeclKind::Param(p) => (p.name.value.clone(), p.name.span),
            DeclKind::Node(n) => (n.name.value.clone(), n.name.span),
            DeclKind::ConstNode(c) => (c.name.value.clone(), c.name.span),
            DeclKind::Assert(a) => (a.name.value.clone(), a.name.span),
            DeclKind::Plot(p) => (p.name.value.clone(), p.name.span),
            DeclKind::Figure(f) => (f.name.value.clone(), f.name.span),
            DeclKind::Layer(l) => (l.name.value.clone(), l.name.span),
            DeclKind::BaseDimension(_)
            | DeclKind::Dimension(_)
            | DeclKind::Unit(_)
            | DeclKind::Type(_)
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                continue;
            }
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        };

        names.insert(ScopedName::local(name.clone()), name_span);

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
            | DeclKind::Index(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {
                // These declarations are handled earlier (continue'd before reaching here).
                continue;
            }
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
        };
        source_order.push((name, category));
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
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
            | DeclKind::Dag(_) => {}
            DeclKind::Sugar(_) => crate::syntax::desugar::unreachable_post_desugar(),
            DeclKind::Assert(a) => {
                asserts.push(CollectedAssertEntry {
                    name: a.name.value.clone(),
                    body: a.body.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Plot(p) => {
                plots.push(CollectedPlotEntry {
                    name: p.name.value.clone(),
                    decl: p.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Figure(f) => {
                figures.push(CollectedFigureEntry {
                    name: f.name.value.clone(),
                    decl: f.clone(),
                });
            }
            DeclKind::Layer(l) => {
                layers.push(CollectedLayerEntry {
                    name: l.name.value.clone(),
                    decl: l.clone(),
                });
            }
            DeclKind::Param(p) => {
                params.push(CollectedParamEntry {
                    name: p.name.value.clone(),
                    default_expr: p.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::ConstNode(c) => {
                consts.push(CollectedConstEntry {
                    name: c.name.value.clone(),
                    expr: c.value.clone(),
                    span: decl.span,
                });
            }
            DeclKind::Node(n) => {
                nodes.push(CollectedNodeEntry {
                    name: n.name.value.clone(),
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
        external_surface,
    })
}

/// Result of attribute validation.
struct ValidatedAttributes {
    assumes_map: HashMap<DeclName, Vec<DeclName>>,
    expected_fail_map: HashMap<DeclName, CollectedExpectedFail>,
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
    let mut expected_fail_map: HashMap<DeclName, CollectedExpectedFail> = HashMap::new();
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
        let attributes =
            attribute_validation::validate_attributes(&decl.attributes).map_err(|error| {
                attribute_validation::attribute_validation_error_to_graphcal(error, src)
            })?;
        for validated in attributes {
            let attr = validated.attribute();
            match validated.name() {
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
                        DeclKind::Import(_) | DeclKind::PluginImport(_) => Some("import"),
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
                    // Structural validation above guarantees a non-empty set
                    // of unique, plain assertion names.
                    for argument in validated.assumes_arguments() {
                        if !assert_names.contains(&argument.value) {
                            return Err(GraphcalError::UnknownAssertInAssumes {
                                name: argument.value.to_string(),
                                src: src.clone(),
                                span: argument.span.into(),
                            });
                        }
                        if let Some(ref dname) = decl_name {
                            assumes_map
                                .entry(argument.value.clone())
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
                                expected_fail_map.insert(
                                    dname.clone(),
                                    CollectedExpectedFail {
                                        expected: ef,
                                        attribute_span: attr.span,
                                    },
                                );
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
                        DeclKind::Import(_) | DeclKind::PluginImport(_) => "import",
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
                        DeclKind::Import(_) | DeclKind::PluginImport(_) => Some("import"),
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
                    return Err(GraphcalError::LazyNotSupported {
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
        hidden_plots,
    })
}

/// Validate that every external signature names only exported type-system
/// symbols (V003 / A9 case 1).
///
/// A declaration's signature is checked when it belongs to the external
/// boundary: either as an explicit `pub` / `pub(bind)` export or as a named
/// `param` input port.
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
    external_surface: &ExternalDeclSurface,
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
            let ref_decl_name = DeclName::from_atom(ref_name.clone());
            if local_type_names.contains_key(ref_name.as_str())
                && !external_surface.is_explicit_export(&ref_decl_name)
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
        // Every `param` signature is an external input-port signature; other
        // kinds participate only when explicitly exported with `pub` / `pub(bind)`.
        let has_external_signature = match &decl.kind {
            DeclKind::Param(_) => true,
            DeclKind::Node(d) | DeclKind::ConstNode(d) => d.visibility.is_public(),
            DeclKind::BaseDimension(d) => d.visibility.is_public(),
            DeclKind::Dimension(d) => d.visibility.is_public(),
            DeclKind::Unit(d) => d.visibility.is_public(),
            DeclKind::Type(d) => d.visibility.is_public(),
            DeclKind::Index(d) => d.visibility.is_public(),
            DeclKind::Dag(d) => d.visibility.is_public(),
            DeclKind::Assert(d) => d.visibility.is_public(),
            DeclKind::Plot(d) => d.visibility.is_public(),
            DeclKind::Figure(d) => d.visibility.is_public(),
            DeclKind::Layer(d) => d.visibility.is_public(),
            // Use-sites carry no blanket visibility; plugin functions are only
            // callable through their own alias; sugar is desugared away.
            DeclKind::Import(_)
            | DeclKind::Include(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Sugar(_) => false,
        };
        if !has_external_signature {
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
                if let IndexDeclKind::RequiredCoordinate { dimension } = &idx.kind {
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
        TypeExprKind::TypeApplication { name, generic_args } => {
            refs.push((name.value.clone(), name.span));
            for arg in generic_args {
                match arg {
                    crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
                        collect_type_refs(type_expr, refs);
                    }
                    crate::desugar::desugared_ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push((path.value.clone(), path.span));
                    }
                    crate::desugar::desugared_ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
                    crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_refs(ambiguous, refs);
                    }
                }
            }
        }
        TypeExprKind::ComplexApplication { generic_args }
        | TypeExprKind::KeyApplication { generic_args } => {
            // `Complex` and `Key` are built in; only their generic argument
            // contributes type-level dependencies.
            for arg in generic_args {
                match arg {
                    crate::desugar::desugared_ast::GenericArg::Type(type_expr) => {
                        collect_type_refs(type_expr, refs);
                    }
                    crate::desugar::desugared_ast::GenericArg::Index(IndexExpr::Name(path)) => {
                        refs.push((path.value.clone(), path.span));
                    }
                    crate::desugar::desugared_ast::GenericArg::Index(
                        IndexExpr::Finite { .. } | IndexExpr::BareNat(_),
                    )
                    | crate::desugar::desugared_ast::GenericArg::Nat(_) => {}
                    crate::desugar::desugared_ast::GenericArg::Ambiguous(ambiguous) => {
                        collect_ambiguous_generic_refs(ambiguous, refs);
                    }
                }
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

fn collect_ambiguous_generic_refs(
    arg: &crate::desugar::desugared_ast::AmbiguousGenericArg,
    refs: &mut Vec<(crate::syntax::names::NamePath, Span)>,
) {
    match arg {
        crate::desugar::desugared_ast::AmbiguousGenericArg::Name(ident) => refs.push((
            crate::syntax::names::NamePath::local(ident.name.clone()),
            ident.span,
        )),
        crate::desugar::desugared_ast::AmbiguousGenericArg::Mul(operands, _) => {
            for operand in operands {
                collect_ambiguous_generic_refs(operand, refs);
            }
        }
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
    pub consts: Vec<(DeclName, TypeExpr, Expr, Span)>,
    pub params: Vec<(DeclName, TypeExpr, Expr, Span)>,
    pub nodes: Vec<(DeclName, TypeExpr, Expr, Span)>,
    pub asserts: Vec<(DeclName, AssertBody, Span)>,
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
#[cfg(test)]
fn resolve(file: &File, src: &NamedSource<Arc<String>>) -> Result<CollectedFile, GraphcalError> {
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
) -> Result<CollectedFile, GraphcalError> {
    let mut names: HashMap<ScopedName, Span> = HashMap::new();

    // Pre-populate with imported names (they don't get duplicate-checked against
    // each other here because they were validated in their source files).
    for (name, _, _, span) in &imported.consts {
        names.insert(ScopedName::from(name), *span);
    }
    for (name, _, _, span) in &imported.params {
        names.insert(ScopedName::from(name), *span);
    }
    for (name, _, _, span) in &imported.nodes {
        names.insert(ScopedName::from(name), *span);
    }
    for (name, _, span) in &imported.asserts {
        names.insert(ScopedName::from(name), *span);
    }

    // Collect local declarations
    let local = collect_local_declarations(file, src, &mut names)?;

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names: HashSet<DeclName> = HashSet::new();
    for (name, _, _) in &imported.asserts {
        all_assert_names.insert(name.clone());
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

    // Prepend imported declarations so they appear before local ones in eval order.
    // Strip TypeExpr from imported tuples and convert to entry types.
    let mut all_consts: Vec<CollectedConstEntry> = imported
        .consts
        .iter()
        .map(|(name, _, expr, span)| CollectedConstEntry {
            name: name.clone(),
            expr: expr.clone(),
            span: *span,
        })
        .collect();
    all_consts.extend(local.consts);
    let mut all_params: Vec<CollectedParamEntry> = imported
        .params
        .iter()
        .map(|(name, _, expr, span)| CollectedParamEntry {
            name: name.clone(),
            default_expr: Some(expr.clone()),
            span: *span,
        })
        .collect();
    all_params.extend(local.params);
    let mut all_nodes: Vec<CollectedNodeEntry> = imported
        .nodes
        .iter()
        .map(|(name, _, expr, span)| CollectedNodeEntry {
            name: name.clone(),
            expr: expr.clone(),
            span: *span,
        })
        .collect();
    all_nodes.extend(local.nodes);
    let mut all_asserts: Vec<CollectedAssertEntry> = imported
        .asserts
        .iter()
        .map(|(name, body, span)| CollectedAssertEntry {
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

    // Validate external signatures: exports and input ports must not reference private type-system items.
    validate_private_in_public(file, src, &local.external_surface)?;

    Ok(CollectedFile {
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
        external_surface: local.external_surface,
    })
}

/// Resolve names with imported value declarations in lexical scope.
///
/// Unlike [`resolve_with_imports`], this does **not** inject imported expressions
/// into the DAG. Imported names are used only for scope checking. HIR lowering
/// attaches canonical targets, and static checking later attaches declared
/// types and any available compile-time values.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if duplicate names, unknown references, or
/// arity mismatches are found.
pub(crate) fn resolve_with_imported_values(
    file: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedValueNames,
) -> Result<CollectedFile, GraphcalError> {
    check_imported_graph_value_names(imported, src)?;
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
        names.insert(ScopedName::from(name), *span);
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

    // Validate external signatures: exports and input ports must not reference private type-system items.
    validate_private_in_public(file, src, &local.external_surface)?;

    Ok(CollectedFile {
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
        external_surface: local.external_surface,
    })
}

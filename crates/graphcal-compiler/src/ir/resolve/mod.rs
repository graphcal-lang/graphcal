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
    AssertBody, DeclKind, DimExpr, ExprKind, File, IndexExpr, TypeDeclBody, TypeExpr, TypeExprKind,
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
    AttributeTarget, DeclCategory, DeclarationKind, ExpectedFail, ExpectedFailKey,
    ExpectedFailKeyPart, ImportedValueNames, ParsedExpectedFail,
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

#[expect(
    clippy::too_many_lines,
    reason = "the namespace policy exhaustively classifies every declaration role"
)]
fn check_builtin_name_shadowing(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        let introduced = match &decl.kind {
            DeclKind::BaseDimension(d) => Some((
                ReservedNameNamespace::Static,
                "dimension",
                d.name.value.atom(),
                d.name.span,
            )),
            DeclKind::Dimension(d) => Some((
                ReservedNameNamespace::Static,
                "dimension",
                d.name.value.atom(),
                d.name.span,
            )),
            DeclKind::Type(t) => Some((
                ReservedNameNamespace::Static,
                "type",
                t.name.value.atom(),
                t.name.span,
            )),
            DeclKind::Index(i) => Some((
                ReservedNameNamespace::Static,
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
                ReservedNameNamespace::Term,
                "param",
                p.name.value.atom(),
                p.name.span,
            )),
            DeclKind::Node(n) => Some((
                ReservedNameNamespace::Term,
                "node",
                n.name.value.atom(),
                n.name.span,
            )),
            DeclKind::ConstNode(c) => Some((
                ReservedNameNamespace::Term,
                "const node",
                c.name.value.atom(),
                c.name.span,
            )),
            DeclKind::Assert(a) => Some((
                ReservedNameNamespace::Term,
                "assert",
                a.name.value.atom(),
                a.name.span,
            )),
            DeclKind::Plot(p) => Some((
                ReservedNameNamespace::Term,
                "plot",
                p.name.value.atom(),
                p.name.span,
            )),
            DeclKind::Figure(f) => Some((
                ReservedNameNamespace::Term,
                "figure",
                f.name.value.atom(),
                f.name.span,
            )),
            DeclKind::Layer(l) => Some((
                ReservedNameNamespace::Term,
                "layer",
                l.name.value.atom(),
                l.name.span,
            )),
            DeclKind::Dag(d) => Some((
                ReservedNameNamespace::Term,
                "dag",
                d.name.value.atom(),
                d.name.span,
            )),
            DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_)
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
        if let DeclKind::Type(type_decl) = &decl.kind
            && let TypeDeclBody::Constructors(constructors) = &type_decl.body
        {
            for constructor in constructors {
                if validate_reserved_name(
                    ReservedNameNamespace::Term,
                    constructor.name.value.atom(),
                )
                .is_err()
                {
                    return Err(GraphcalError::BuiltinNameShadowed {
                        kind: "constructor",
                        name: constructor.name.value.to_string(),
                        src: src.clone(),
                        span: constructor.name.span.into(),
                    });
                }
            }
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
            validate_reserved_name(ReservedNameNamespace::Term, atom).map_err(|_| {
                GraphcalError::BuiltinNameShadowed {
                    kind: "graph-value alias",
                    name: atom.to_string(),
                    src: src.clone(),
                    span: (*span).into(),
                }
            })
        })
}

fn check_static_namespace_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut occupied = HashMap::new();
    for (atom, span) in file
        .declarations
        .iter()
        .filter_map(|decl| static_namespace_decl(&decl.kind))
    {
        register_exclusive_universe_name(&mut occupied, atom, span, src)?;
    }
    Ok(())
}

fn static_namespace_decl(decl: &DeclKind) -> Option<(&NameAtom, Span)> {
    match decl {
        DeclKind::BaseDimension(d) => Some((d.name.value.atom(), d.name.span)),
        DeclKind::Dimension(d) => Some((d.name.value.atom(), d.name.span)),
        DeclKind::Type(t) => Some((t.name.value.atom(), t.name.span)),
        DeclKind::Index(i) => Some((i.name.value.atom(), i.name.span)),
        DeclKind::Param(_)
        | DeclKind::Node(_)
        | DeclKind::ConstNode(_)
        | DeclKind::Assert(_)
        | DeclKind::Plot(_)
        | DeclKind::Figure(_)
        | DeclKind::Layer(_)
        | DeclKind::Dag(_)
        | DeclKind::Unit(_)
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
            DeclKind::Dag(d) => register_value_namespace_name(
                &mut value_names,
                d.name.value.atom(),
                d.name.span,
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
            | DeclKind::Include(_) => {}
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
    check_static_namespace_collisions(file, src)?;
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
                external_surface.insert_static_export(name.into_atom());
            }
            DeclKind::Dimension(d) if d.visibility.is_public() => {
                external_surface.insert_static_export(name.into_atom());
            }
            DeclKind::Unit(d) if d.visibility.is_public() => {
                external_surface.insert_unit_export(name.into_atom());
            }
            DeclKind::Type(d) if d.visibility.is_public() => {
                external_surface.insert_static_export(name.into_atom());
            }
            DeclKind::Index(d) if d.visibility.is_public() => {
                external_surface.insert_static_export(name.into_atom());
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
        let declaration_kind = DeclarationKind::from_decl_kind(&decl.kind);
        let target = AttributeTarget::declaration(declaration_kind);
        let attributes = attribute_validation::validate_attributes(&decl.attributes, &target)
            .map_err(|error| {
                attribute_validation::attribute_validation_error_to_graphcal(error, src)
            })?;
        for validated in attributes {
            let attr = validated.attribute();
            match validated.name() {
                AttributeName::Assumes => {
                    // Shared applicability and structural validation guarantee
                    // a node/param target with a non-empty set
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
                    let DeclKind::Assert(assertion) = &decl.kind else {
                        return Err(GraphcalError::internal_error(
                            "attribute applicability accepted expected_fail on a non-assert",
                            src,
                            crate::diagnostic_anchor::DiagnosticAnchor::Source(attr.span),
                        ));
                    };
                    let expected = parse_expected_fail_args(&attr.args, src)?;
                    // A blanket expected failure on an indexed assertion is
                    // ambiguous; users must name the expected failing keys.
                    if matches!(expected, ExpectedFail::All) {
                        let is_indexed = matches!(
                            &assertion.body,
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
                                expected,
                                attribute_span: attr.span,
                            },
                        );
                    }
                }
                AttributeName::Hidden => {
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

#[derive(Debug, Clone)]
enum LocalTypeSystemDeclaration {
    Dimension(crate::syntax::dimension::DimName),
    Index(crate::syntax::index_name::IndexName),
    Type(crate::syntax::type_name::StructTypeName),
}

impl LocalTypeSystemDeclaration {
    const fn kind(&self) -> DeclarationKind {
        match self {
            Self::Dimension(_) => DeclarationKind::Dimension,
            Self::Index(_) => DeclarationKind::Index,
            Self::Type(_) => DeclarationKind::Type,
        }
    }

    const fn atom(&self) -> &NameAtom {
        match self {
            Self::Dimension(name) => name.atom(),
            Self::Index(name) => name.atom(),
            Self::Type(name) => name.atom(),
        }
    }
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

    // Preserve the semantic category beside each typed local name so the
    // visibility diagnostic never has to rescan declarations by string.
    let mut local_type_names: HashMap<NameAtom, (LocalTypeSystemDeclaration, Span)> =
        HashMap::new();
    for decl in &file.declarations {
        let (name, span) = match &decl.kind {
            DeclKind::BaseDimension(d) => (
                LocalTypeSystemDeclaration::Dimension(d.name.value.clone()),
                d.name.span,
            ),
            DeclKind::Dimension(d) => (
                LocalTypeSystemDeclaration::Dimension(d.name.value.clone()),
                d.name.span,
            ),
            DeclKind::Index(index) => (
                LocalTypeSystemDeclaration::Index(index.name.value.clone()),
                index.name.span,
            ),
            DeclKind::Type(r#type) => (
                LocalTypeSystemDeclaration::Type(r#type.name.value.clone()),
                r#type.name.span,
            ),
            _ => continue,
        };
        local_type_names.insert(name.atom().clone(), (name, span));
    }

    // If there are no local type-system names, nothing to check.
    if local_type_names.is_empty() {
        return Ok(());
    }

    let emit = |pub_kind: DeclarationKind,
                pub_name: NameAtom,
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
            if let Some((referenced, _)) = local_type_names.get(ref_name)
                && !external_surface.is_static_explicit_export(ref_decl_name.atom())
            {
                return Err(GraphcalError::PrivateInPublic {
                    pub_kind,
                    pub_name,
                    ref_kind: referenced.kind(),
                    ref_name: referenced.atom().clone(),
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
        let (kind, name): (DeclarationKind, NameAtom) = match &decl.kind {
            DeclKind::Param(p) => {
                collect_type_refs(&p.type_ann, &mut refs);
                (DeclarationKind::Param, p.name.value.atom().clone())
            }
            DeclKind::Node(n) => {
                collect_type_refs(&n.type_ann, &mut refs);
                (DeclarationKind::Node, n.name.value.atom().clone())
            }
            DeclKind::ConstNode(c) => {
                collect_type_refs(&c.type_ann, &mut refs);
                (DeclarationKind::ConstNode, c.name.value.atom().clone())
            }
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    collect_dim_refs(def, &mut refs);
                }
                (DeclarationKind::Dimension, d.name.value.atom().clone())
            }
            DeclKind::Unit(u) => {
                collect_dim_refs(&u.dim_type, &mut refs);
                (DeclarationKind::Unit, u.name.value.atom().clone())
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
                (DeclarationKind::Type, t.name.value.atom().clone())
            }
            DeclKind::Index(index) => {
                if let IndexDeclKind::RequiredCoordinate { dimension } = &index.kind {
                    collect_dim_refs(dimension, &mut refs);
                }
                (DeclarationKind::Index, index.name.value.atom().clone())
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
        TypeExprKind::IndexLabel { .. }
        | TypeExprKind::Dimensionless
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

/// Collect declaration entries and validate declaration shells through the
/// production imported-binding path.
#[cfg(test)]
fn resolve(file: &File, src: &NamedSource<Arc<String>>) -> Result<CollectedFile, GraphcalError> {
    resolve_with_imported_values(file, src, &ImportedValueNames::default())
}

/// Resolve names with imported value declarations in lexical scope.
///
/// Imported names are used only for scope checking; no imported AST
/// expressions are injected into the DAG. HIR lowering
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

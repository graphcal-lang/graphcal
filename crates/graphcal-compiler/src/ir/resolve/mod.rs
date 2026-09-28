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

use super::required_bindability::{self, InterfaceDecl, Violation as RequiredBindabilityViolation};
use super::static_interface::{Requirement, StaticInputKind as NominalKind};

use crate::assertion_expectation::ExpectedFail;
use crate::declaration_category::{DeclCategory, ValueDeclCategory};
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
use crate::syntax::ast::{DeclExposure, ImportItemNamespace, IntroducedKind};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::phase::never;
use crate::syntax::span::Span;

// Re-export types and constants from graphcal-registry's resolve_types module.
pub(crate) use crate::registry::resolve_types::CollectedFile;
pub use crate::registry::resolve_types::{AttributeTarget, DeclarationKind, ImportedValueNames};
pub use crate::syntax::module_name::ScopedName;

// Re-export items from submodules (crate-internal only).
pub(crate) use deps::contains_graph_ref;

// Import helpers from submodules for use within this file.
pub use names::parse_expected_fail_args;

fn register_value_namespace_name(
    value_names: &mut HashMap<ScopedName, Span>,
    name: &NameAtom,
    span: Span,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let scoped_name = ScopedName::local(DeclName::classify(name.clone()));
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

/// Reject every introduced name (declarations and `type` constructors) that
/// shadows a built-in spelling reserved in its namespace.
fn check_builtin_name_shadowing(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    file.declarations
        .iter()
        .flat_map(|decl| decl.kind.introduced_names())
        .try_for_each(|introduced| {
            validate_reserved_name(
                ReservedNameNamespace::of(introduced.namespace()),
                introduced.atom(),
            )
            .map_err(|_| GraphcalError::BuiltinNameShadowed {
                kind: introduced.kind().describe(),
                name: introduced.atom().to_string(),
                src: src.clone(),
                span: introduced.span().into(),
            })
        })
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
            let atom = name.leaf().atom();
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

/// Dimensions, types, and indexes share one exclusive Static universe.
fn check_static_namespace_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut occupied = HashMap::new();
    for introduced in file
        .declarations
        .iter()
        .filter_map(|decl| decl.kind.declared_name())
        .filter(|introduced| {
            ReservedNameNamespace::of(introduced.namespace()) == ReservedNameNamespace::Static
        })
    {
        register_exclusive_universe_name(&mut occupied, introduced.atom(), introduced.span(), src)?;
    }
    Ok(())
}

/// Term-namespace names (value declarations, DAGs, and constructors) must be
/// unique among themselves and against imported value names.
fn check_value_namespace_collisions(
    file: &File,
    src: &NamedSource<Arc<String>>,
    names: &HashMap<ScopedName, Span>,
) -> Result<(), GraphcalError> {
    let mut value_names: HashMap<ScopedName, Span> = names.clone();
    for introduced in file
        .declarations
        .iter()
        .flat_map(|decl| decl.kind.introduced_names())
        .filter(|introduced| introduced.namespace() == ImportItemNamespace::Term)
    {
        register_value_namespace_name(&mut value_names, introduced.atom(), introduced.span(), src)?;
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
fn required_bindability_interface(decl: &DeclKind) -> Option<InterfaceDecl> {
    let requirement_from_missing_definition = |missing| {
        if missing {
            Requirement::Required
        } else {
            Requirement::Defaulted
        }
    };

    match decl {
        DeclKind::Param(param) => Some(InterfaceDecl::InputPort {
            requirement: requirement_from_missing_definition(param.value.is_none()),
        }),
        DeclKind::Index(index) => Some(InterfaceDecl::Nominal {
            kind: NominalKind::Index,
            visibility: index.visibility,
            requirement: requirement_from_missing_definition(index.kind.is_required()),
        }),
        DeclKind::Type(type_decl) => Some(InterfaceDecl::Nominal {
            kind: NominalKind::Type,
            visibility: type_decl.visibility,
            requirement: requirement_from_missing_definition(matches!(
                type_decl.body,
                TypeDeclBody::Required
            )),
        }),
        DeclKind::Dimension(dimension) => Some(InterfaceDecl::Nominal {
            kind: NominalKind::Dimension,
            visibility: dimension.visibility,
            requirement: requirement_from_missing_definition(dimension.definition.is_none()),
        }),
        // Base dimensions and units always carry a definition; the remaining
        // kinds are not externally suppliable interface declarations.
        DeclKind::Node(_)
        | DeclKind::ConstNode(_)
        | DeclKind::BaseDimension(_)
        | DeclKind::Unit(_)
        | DeclKind::Import(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Include(_)
        | DeclKind::Dag(_)
        | DeclKind::Assert(_)
        | DeclKind::Plot(_)
        | DeclKind::Figure(_)
        | DeclKind::Layer(_) => None,
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar(Infallible) proves this arm unreachable"
        )]
        DeclKind::Sugar(s) => never(*s),
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
        .filter_map(|decl| {
            Some((
                required_bindability_interface(&decl.kind)?,
                decl.kind.declared_name()?,
            ))
        })
        .try_for_each(|(interface, introduced)| {
            required_bindability::validate(interface).map_err(|violation| match violation {
                RequiredBindabilityViolation::RequiredMustBeBindable { kind } => {
                    GraphcalError::RequiredItemMustBeBindable {
                        kind: kind.to_string(),
                        name: introduced.atom().to_string(),
                        src: src.clone(),
                        span: introduced.span().into(),
                    }
                }
            })
        })
}

/// Evaluation source-order category of a declaration's own name, or `None`
/// for declarations outside the evaluated value/sink order.
const fn source_order_category(kind: IntroducedKind) -> Option<DeclCategory> {
    match kind {
        IntroducedKind::Param => Some(DeclCategory::Value(ValueDeclCategory::Param)),
        IntroducedKind::ConstNode => Some(DeclCategory::Value(ValueDeclCategory::Const)),
        IntroducedKind::Node => Some(DeclCategory::Value(ValueDeclCategory::Node)),
        IntroducedKind::Assert => Some(DeclCategory::Assert),
        IntroducedKind::Plot => Some(DeclCategory::Plot),
        IntroducedKind::Figure => Some(DeclCategory::Figure),
        IntroducedKind::Layer => Some(DeclCategory::Layer),
        IntroducedKind::Dag
        | IntroducedKind::Constructor
        | IntroducedKind::BaseDimension
        | IntroducedKind::Dimension
        | IntroducedKind::Unit
        | IntroducedKind::Type
        | IntroducedKind::Index => None,
    }
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
    for introduced in file
        .declarations
        .iter()
        .filter_map(|decl| decl.kind.declared_name())
    {
        external_surface.record_declared(introduced);
    }

    validate_required_bindability(file, src)?;

    // First pass: record evaluated declarations in source order. Static
    // declarations, units, and DAGs are handled by the registry and module
    // resolver, not by this value scope.
    for introduced in file
        .declarations
        .iter()
        .filter_map(|decl| decl.kind.declared_name())
    {
        let Some(category) = source_order_category(introduced.kind()) else {
            continue;
        };
        let name = DeclName::classify(introduced.atom().clone());
        names.insert(ScopedName::local(name.clone()), introduced.span());
        if category == DeclCategory::Assert {
            assert_names.insert(name.clone());
        }
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
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(s) => never(*s),
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
                    definition: n.definition.clone(),
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
        // Attribute applicability limits name-bearing attributes to
        // param/node (`assumes`), assert (`expected_fail`), and plot
        // (`hidden`) targets, all of which declare a Term name.
        let decl_name: Option<DeclName> = decl
            .kind
            .declared_name()
            .map(|introduced| DeclName::classify(introduced.atom().clone()));
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
fn validate_private_in_public(
    file: &File,
    src: &NamedSource<Arc<String>>,
    external_surface: &ExternalDeclSurface,
) -> Result<(), GraphcalError> {
    use crate::desugar::desugared_ast::IndexDeclKind;

    // Preserve the semantic category beside each local type-system name so
    // the visibility diagnostic never has to rescan declarations.
    let local_type_names: HashMap<&NameAtom, DeclarationKind> = file
        .declarations
        .iter()
        .filter_map(|decl| {
            let introduced = decl.kind.declared_name()?;
            (ReservedNameNamespace::of(introduced.namespace()) == ReservedNameNamespace::Static)
                .then(|| {
                    (
                        introduced.atom(),
                        DeclarationKind::from_decl_kind(&decl.kind),
                    )
                })
        })
        .collect();

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
            if let Some(ref_kind) = local_type_names.get(ref_name)
                && !external_surface.is_static_explicit_export(ref_name)
            {
                return Err(GraphcalError::PrivateInPublic {
                    pub_kind,
                    pub_name,
                    ref_kind: *ref_kind,
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
        // Every `param` signature is an external input-port signature; other
        // kinds participate only when explicitly exported with `pub` /
        // `pub(bind)`. Use-sites (`import`, `include`, `import plugin`) bind
        // no declaration name and carry no blanket visibility.
        let Some(introduced) = decl.kind.declared_name() else {
            continue;
        };
        if introduced.exposure() == DeclExposure::Private {
            continue;
        }

        let mut refs: Vec<(crate::syntax::names::NamePath, Span)> = Vec::new();
        match &decl.kind {
            DeclKind::Param(p) => collect_type_refs(&p.type_ann, &mut refs),
            DeclKind::Node(n) => collect_type_refs(&n.type_ann, &mut refs),
            DeclKind::ConstNode(c) => collect_type_refs(&c.type_ann, &mut refs),
            DeclKind::Dimension(d) => {
                if let Some(def) = &d.definition {
                    collect_dim_refs(def, &mut refs);
                }
            }
            DeclKind::Unit(u) => collect_dim_refs(&u.dim_type, &mut refs),
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
            }
            DeclKind::Index(index) => {
                if let IndexDeclKind::RequiredCoordinate { dimension } = &index.kind {
                    collect_dim_refs(dimension, &mut refs);
                }
            }
            // Sink kinds have no written signature; bodies are not A9 case 1.
            // BaseDimension has no body. Import/Include are use-sites. Dag is
            // a use-site at the signature level.
            DeclKind::BaseDimension(_)
            | DeclKind::Dag(_)
            | DeclKind::Assert(_)
            | DeclKind::Plot(_)
            | DeclKind::Figure(_)
            | DeclKind::Layer(_)
            | DeclKind::Import(_)
            | DeclKind::PluginImport(_)
            | DeclKind::Include(_) => continue,
            #[expect(
                clippy::uninhabited_references,
                reason = "Sugar(Infallible) proves this arm unreachable"
            )]
            DeclKind::Sugar(s) => never(*s),
        }

        emit(
            DeclarationKind::from_decl_kind(&decl.kind),
            introduced.atom().clone(),
            decl.span,
            &refs,
        )?;
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
            crate::syntax::names::NamePath::local(ident.name.atom().clone()),
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
        names.insert(ScopedName::local(name.clone()), *span);
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
        assumes_map: validated.assumes_map,
        expected_fail: validated.expected_fail_map,
        hidden_plots: validated.hidden_plots,
        external_surface: local.external_surface,
    })
}

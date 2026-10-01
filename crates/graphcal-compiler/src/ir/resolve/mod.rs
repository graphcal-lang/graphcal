pub mod attribute_validation;
pub mod collected;
#[cfg(test)]
mod formal_conformance;
pub mod include_selection;
pub(crate) mod names;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use super::required_bindability::{self, InterfaceDecl, Violation as RequiredBindabilityViolation};
use crate::semantic_error::attribute::AttributeError;
use crate::semantic_error::visibility::VisibilityError;
use crate::source_id::SourceId;
use crate::static_interface::{Requirement, StaticInputKind as NominalKind};

use crate::assertion_expectation::ExpectedFail;
use crate::dag_id::DagId;
use crate::declaration_category::{DeclCategory, ValueDeclCategory};
use crate::declaration_kind::{AttributeTarget, DeclarationKind};
use crate::desugar::desugared_ast::{
    AssertBody, DeclKind, Declaration, DimExpr, ExprKind, File, IndexDeclKind, IndexExpr,
    TypeDeclBody, TypeExpr, TypeExprKind,
};
use crate::graphcal_error::GraphcalError;
use crate::ir::entry::{
    AssertEntry, ConstEntry, Decl, FigureEntry, InScope, LayerEntry, NodeEntry, ParamEntry,
    PlotEntry, PlotSyntax, Syntax,
};
use crate::ir::resolve::collected::{CollectedExpectedFail, ExternalDeclSurface};
use crate::plot_visibility::PlotVisibility;
use crate::resolve::ModuleResolver;
use crate::resolve::error::ModuleResolveError;
use crate::resolve::namespace::Namespace;
use crate::resolve::reserved_name::validate_reserved_name;
use crate::resolved_name::ResolvedDeclName;
use crate::syntax::ast::{DeclExposure, ImportItemNamespace, IntroducedKind};
use crate::syntax::attribute::AttributeName;
use crate::syntax::decl_name::DeclName;
use crate::syntax::names::NameAtom;
use crate::syntax::phase::never;
use crate::syntax::span::{Span, Spanned};

// Re-export declaration-collection data types.
pub use crate::ir::resolve::collected::ImportedValueNames;
pub use crate::syntax::module_name::ScopedName;

// Re-export items from submodules (crate-internal only).

// Import helpers from submodules for use within this file.
pub use names::parse_expected_fail_args;

fn register_value_namespace_name(
    value_names: &mut HashMap<ScopedName, Span>,
    name: &NameAtom,
    span: Span,
    src: SourceId,
) -> Result<(), GraphcalError> {
    let scoped_name = ScopedName::local(DeclName::classify(name.clone()));
    if let Some(first_span) = value_names.get(&scoped_name) {
        return Err(GraphcalError::DuplicateName {
            name: name.to_string(),
            src,
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
    src: SourceId,
) -> Result<(), GraphcalError> {
    occupied.insert(atom.clone(), span).map_or(Ok(()), |first| {
        Err(GraphcalError::DuplicateName {
            name: atom.to_string(),
            src,
            duplicate: span.into(),
            first: first.into(),
        })
    })
}

/// Reject every introduced name (declarations and `type` constructors) that
/// shadows a built-in spelling reserved in its namespace.
fn check_builtin_name_shadowing(file: &File, src: SourceId) -> Result<(), GraphcalError> {
    file.declarations
        .iter()
        .flat_map(|decl| decl.kind.introduced_names())
        .try_for_each(|introduced| {
            validate_reserved_name(Namespace::of(introduced.namespace()), introduced.atom())
                .map_err(|_| GraphcalError::BuiltinNameShadowed {
                    kind: introduced.kind().describe(),
                    name: introduced.atom().to_string(),
                    src,
                    span: introduced.span().into(),
                })
        })
}

fn check_imported_graph_value_names(
    imported: &ImportedValueNames,
    src: SourceId,
) -> Result<(), GraphcalError> {
    imported
        .const_names
        .iter()
        .chain(&imported.param_names)
        .chain(&imported.node_names)
        .filter(|(name, _)| !name.is_qualified())
        .try_for_each(|(name, span)| {
            let atom = name.leaf().atom();
            validate_reserved_name(Namespace::Term, atom).map_err(|_| {
                GraphcalError::BuiltinNameShadowed {
                    kind: "graph-value alias",
                    name: atom.to_string(),
                    src,
                    span: (*span).into(),
                }
            })
        })
}

/// Dimensions, types, and indexes share one exclusive Static universe.
fn check_static_namespace_collisions(file: &File, src: SourceId) -> Result<(), GraphcalError> {
    let mut occupied = HashMap::new();
    for introduced in file
        .declarations
        .iter()
        .filter_map(|decl| decl.kind.declared_name())
        .filter(|introduced| Namespace::of(introduced.namespace()) == Namespace::Static)
    {
        register_exclusive_universe_name(&mut occupied, introduced.atom(), introduced.span(), src)?;
    }
    Ok(())
}

/// Term-namespace names (value declarations, DAGs, and constructors) must be
/// unique among themselves and against imported value names.
fn check_value_namespace_collisions(
    file: &File,
    src: SourceId,
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

/// The result of declaration collection: declarations separated by category,
/// each entry carrying its complete signature and attribute-derived policy.
#[derive(Debug)]
pub(crate) struct CollectedFile {
    /// Output visibility of each plot; see [`declaration_entries`].
    pub(crate) plot_visibilities: HashMap<DeclName, PlotVisibility>,
    /// Mapping from assert name to the list of declarations that assume it.
    /// Built from `#[assumes(...)]` attributes.
    pub(crate) assumes_map: HashMap<DeclName, Vec<DeclName>>,
    /// Mapping from assert name to its expected-fail configuration.
    /// Built from `#[expected_fail]` / `#[expected_fail(...)]` attributes.
    pub(crate) expected_fail: HashMap<DeclName, CollectedExpectedFail>,
    /// Explicit exports and annotation-free `param` input ports, classified by role.
    pub(crate) external_surface: ExternalDeclSurface,
}

/// A collected file together with the entries built from it, for tests.
#[cfg(test)]
#[derive(Debug)]
struct CollectedWithEntries {
    file: CollectedFile,
    decls: Vec<Decl<Syntax>>,
}

#[cfg(test)]
impl std::ops::Deref for CollectedWithEntries {
    type Target = CollectedFile;

    fn deref(&self) -> &CollectedFile {
        &self.file
    }
}

#[cfg(test)]
impl CollectedWithEntries {
    fn consts(&self) -> Vec<&ConstEntry<Syntax>> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Const(entry) => Some(entry),
                _ => None,
            })
            .collect()
    }

    fn params(&self) -> Vec<&ParamEntry<Syntax>> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Param(entry) => Some(entry),
                _ => None,
            })
            .collect()
    }

    fn nodes(&self) -> Vec<&NodeEntry<Syntax>> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Node(entry) => Some(entry),
                _ => None,
            })
            .collect()
    }

    fn plots(&self) -> Vec<&PlotEntry<Syntax>> {
        self.decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Plot(entry) => Some(entry),
                _ => None,
            })
            .collect()
    }
}

/// Result of validating local declaration shells from the AST.
struct CollectedDeclarations {
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
fn validate_required_bindability(file: &File, src: SourceId) -> Result<(), GraphcalError> {
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
                    GraphcalError::located(
                        src,
                        introduced.span(),
                        VisibilityError::RequiredItemMustBeBindable {
                            kind: kind.to_string(),
                            name: introduced.atom().to_string(),
                        },
                    )
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

/// Validate all local declaration shells and check for duplicates.
///
/// Returns the local assertion names and external surface, and records local
/// evaluated names in the names map for further processing.
fn collect_local_declarations(
    file: &File,
    declared_surface: &ExternalDeclSurface,
    src: SourceId,
    names: &mut HashMap<ScopedName, Span>,
) -> Result<CollectedDeclarations, GraphcalError> {
    let mut assert_names: HashSet<DeclName> = HashSet::new();

    check_builtin_name_shadowing(file, src)?;
    check_static_namespace_collisions(file, src)?;
    check_value_namespace_collisions(file, src, names)?;

    // The module interface classifies the externally addressable surface
    // without treating `param` input ports as ordinary exports.
    let external_surface = declared_surface.clone();

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
            assert_names.insert(name);
        }
    }

    Ok(CollectedDeclarations {
        assert_names,
        external_surface,
    })
}

/// Declaration entries built alongside attribute validation.
#[derive(Default)]
struct CollectedEntries {
    plot_visibilities: HashMap<DeclName, PlotVisibility>,
    assumes_map: HashMap<DeclName, Vec<DeclName>>,
    expected_fail_map: HashMap<DeclName, CollectedExpectedFail>,
}

/// Build each value and sink declaration's entry, in source order, under the
/// identity the resolver declared for it.
///
/// `plot_visibilities` holds the output visibility validated for each plot,
/// the sole target that accepts `#[hidden]`.
///
/// # Errors
///
/// Returns the resolver's error if it did not declare one of `file`'s own
/// declarations in `dag_id`, which would be a compiler bug.
pub(crate) fn declaration_entries(
    file: &File,
    plot_visibilities: &HashMap<DeclName, PlotVisibility>,
    resolver: &ModuleResolver,
    dag_id: &DagId,
) -> Result<Vec<Decl<Syntax>>, ModuleResolveError> {
    file.declarations
        .iter()
        .filter_map(|decl| {
            entry(decl, dag_id, |name| {
                let visibility = plot_visibilities
                    .get(&name.value)
                    .copied()
                    .unwrap_or(PlotVisibility::Standalone);
                resolver
                    .declaration(dag_id, &name.value)
                    .map(|symbol| (symbol.into_resolved(), visibility))
            })
        })
        .collect()
}

/// The entry of `decl` with its complete signature, if it is a value or sink
/// declaration. `declare` supplies the canonical identity (and, for a plot,
/// the output visibility) of the declared name.
fn entry<E>(
    decl: &Declaration,
    dag_id: &DagId,
    declare: impl FnOnce(&Spanned<DeclName>) -> Result<(ResolvedDeclName, PlotVisibility), E>,
) -> Option<Result<Decl<Syntax>, E>> {
    Some(match &decl.kind {
        DeclKind::BaseDimension(_)
        | DeclKind::Dimension(_)
        | DeclKind::Unit(_)
        | DeclKind::Type(_)
        | DeclKind::Index(_)
        | DeclKind::Import(_)
        | DeclKind::PluginImport(_)
        | DeclKind::Include(_)
        | DeclKind::Dag(_) => return None,
        #[expect(
            clippy::uninhabited_references,
            reason = "Sugar(Infallible) proves this arm unreachable"
        )]
        DeclKind::Sugar(s) => never(*s),
        DeclKind::Assert(a) => declare(&a.name).map(|(identity, _)| {
            Decl::Assert(AssertEntry {
                identity,
                body: InScope::new(a.body.clone(), dag_id.clone()),
                span: decl.span,
            })
        }),
        DeclKind::Plot(p) => declare(&p.name).map(|(identity, visibility)| {
            Decl::Plot(PlotEntry {
                identity,
                mark_type: p.mark.mark_type,
                body: InScope::new(
                    PlotSyntax {
                        encodings: p.encodings.clone(),
                        mark_properties: p.mark.properties.clone(),
                        properties: p.properties.clone(),
                    },
                    dag_id.clone(),
                ),
                visibility,
            })
        }),
        DeclKind::Figure(f) => declare(&f.name).map(|(identity, _)| {
            Decl::Figure(FigureEntry {
                identity,
                plot_names: f.plot_names.clone(),
                fields: InScope::new(f.fields.clone(), dag_id.clone()),
            })
        }),
        DeclKind::Layer(l) => declare(&l.name).map(|(identity, _)| {
            Decl::Layer(LayerEntry {
                identity,
                plot_names: l.plot_names.clone(),
                fields: InScope::new(l.fields.clone(), dag_id.clone()),
            })
        }),
        DeclKind::Param(p) => declare(&p.name).map(|(identity, _)| {
            Decl::Param(ParamEntry {
                identity,
                type_ann: InScope::new(p.type_ann.clone(), dag_id.clone()),
                default: p
                    .value
                    .as_ref()
                    .map(|expr| InScope::new(expr.clone(), dag_id.clone())),
                span: decl.span,
                override_reconciliations: Vec::new(),
            })
        }),
        DeclKind::ConstNode(c) => declare(&c.name).map(|(identity, _)| {
            Decl::Const(ConstEntry {
                identity,
                type_ann: InScope::new(c.type_ann.clone(), dag_id.clone()),
                expr: InScope::new(c.value.clone(), dag_id.clone()),
                span: decl.span,
            })
        }),
        DeclKind::Node(n) => declare(&n.name).map(|(identity, _)| {
            Decl::Node(NodeEntry {
                identity,
                type_ann: InScope::new(n.type_ann.clone(), dag_id.clone()),
                definition: InScope::new(n.definition.clone(), dag_id.clone()),
                span: decl.span,
            })
        }),
    })
}

/// Validate every declaration's attributes, record `assumes_map` /
/// `expected_fail_map`, and build each value/sink declaration entry.
fn collect_entries(
    file: &File,
    src: SourceId,
    assert_names: &HashSet<DeclName>,
) -> Result<CollectedEntries, GraphcalError> {
    let mut entries = CollectedEntries::default();
    for decl in &file.declarations {
        let visibility = validate_declaration_attributes(
            decl,
            src,
            assert_names,
            &mut entries.assumes_map,
            &mut entries.expected_fail_map,
        )?;
        if let DeclKind::Plot(plot) = &decl.kind {
            entries
                .plot_visibilities
                .insert(plot.name.value.clone(), visibility);
        }
    }
    Ok(entries)
}

/// Validate one declaration's attributes, recording `#[assumes]` and
/// `#[expected_fail]` metadata. Returns the plot output visibility requested
/// by `#[hidden]` (#847); attribute applicability admits it only on plots.
fn validate_declaration_attributes(
    decl: &Declaration,
    src: SourceId,
    assert_names: &HashSet<DeclName>,
    assumes_map: &mut HashMap<DeclName, Vec<DeclName>>,
    expected_fail_map: &mut HashMap<DeclName, CollectedExpectedFail>,
) -> Result<PlotVisibility, GraphcalError> {
    let mut visibility = PlotVisibility::Standalone;
    // Attribute applicability limits name-bearing attributes to
    // param/node (`assumes`), assert (`expected_fail`), and plot
    // (`hidden`) targets, all of which declare a Term name.
    let decl_name: Option<DeclName> = decl
        .kind
        .declared_name()
        .map(|introduced| DeclName::classify(introduced.atom().clone()));
    let declaration_kind = DeclarationKind::from_decl_kind(&decl.kind);
    let target = AttributeTarget::declaration(declaration_kind);
    let attributes =
        attribute_validation::validate_attributes(&decl.attributes, &target).map_err(|error| {
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
                        return Err(GraphcalError::located(
                            src,
                            argument.span,
                            AttributeError::UnknownAssertInAssumes {
                                name: argument.value.to_string(),
                            },
                        ));
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
                        return Err(GraphcalError::located(
                            src,
                            attr.span,
                            AttributeError::ExpectedFailAllOnIndexed,
                        ));
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
                        src,
                        span: attr.span.into(),
                    });
                }
                visibility = PlotVisibility::CompositionOnly;
            }
            AttributeName::Lazy => {
                return Err(GraphcalError::located(
                    src,
                    attr.span,
                    AttributeError::LazyNotSupported,
                ));
            }
        }
    }
    Ok(visibility)
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
    src: SourceId,
    external_surface: &ExternalDeclSurface,
) -> Result<(), GraphcalError> {
    // Preserve the semantic category beside each local type-system name so
    // the visibility diagnostic never has to rescan declarations.
    let local_type_names: HashMap<&NameAtom, DeclarationKind> = file
        .declarations
        .iter()
        .filter_map(|decl| {
            let introduced = decl.kind.declared_name()?;
            (Namespace::of(introduced.namespace()) == Namespace::Static).then(|| {
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
            // Only a bare path can name a local declaration; qualified refs are foreign.
            let Some(ref_name) = ref_path.as_bare() else {
                continue;
            };
            if let Some(ref_kind) = local_type_names.get(ref_name)
                && !external_surface.is_static_explicit_export(ref_name)
            {
                return Err(GraphcalError::located(
                    src,
                    *ref_span,
                    VisibilityError::PrivateInPublic {
                        pub_kind,
                        pub_name,
                        ref_kind: *ref_kind,
                        ref_name: ref_name.clone(),
                        pub_span,
                    },
                ));
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

/// Validate declaration shells through the production imported-binding path,
/// then build the declaration entries against a single-module resolver.
#[cfg(test)]
fn resolve(file: &File, src: SourceId) -> Result<CollectedWithEntries, GraphcalError> {
    let interface = crate::ir::module_interface::ModuleInterface::new(&file.declarations);
    let collected = resolve_with_imported_values(
        file,
        interface.declared_surface(),
        src,
        &ImportedValueNames::default(),
    )?;
    let dag_id = DagId::root_in_package("test", "main");
    let mut modules = crate::resolve::builder::TestModules::default();
    modules.add(dag_id.clone(), &file.declarations);
    let resolver = modules
        .build()
        .expect("validated declarations build a single-module resolver");
    let decls = declaration_entries(file, &collected.plot_visibilities, &resolver, &dag_id)
        .expect("the resolver declares every collected declaration");
    Ok(CollectedWithEntries {
        file: collected,
        decls,
    })
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
    declared_surface: &ExternalDeclSurface,
    src: SourceId,
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
    let local = collect_local_declarations(file, declared_surface, src, &mut names)?;

    // Build assert names (imported + local) for attribute validation
    let mut all_assert_names: HashSet<DeclName> = HashSet::new();
    for (name, _) in &imported.assert_names {
        all_assert_names.insert(name.clone());
    }
    all_assert_names.extend(local.assert_names.iter().cloned());

    // Validate attributes and build assumes_map / expected_fail_map and the
    // plot visibilities that `declaration_entries` needs.
    let entries = collect_entries(file, src, &all_assert_names)?;

    // Validate external signatures: exports and input ports must not reference private type-system items.
    validate_private_in_public(file, src, &local.external_surface)?;

    Ok(CollectedFile {
        plot_visibilities: entries.plot_visibilities,
        assumes_map: entries.assumes_map,
        expected_fail: entries.expected_fail_map,
        external_surface: local.external_surface,
    })
}

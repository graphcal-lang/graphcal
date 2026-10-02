//! Checked-DAG specialization for semantic include instances.

use crate::semantic_error::dimension::DimensionError;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::{
    DagTIR, ResolvedDeclType, ResolvedDim, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex,
    ResolvedValueType, UncheckedTir,
};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::{BaseDimId, Dimension};
use crate::ir::instance::HirInstanceRecord;
use crate::ir::instance::frame::InstanceFrame;
use crate::ir::instance::identity::{instance_declaration, projection_alias, template_declaration};
use crate::ir::static_substitution::{InstanceIndexBindingTarget, StaticSubstitution};
use crate::nat::NatPolyForm;
use crate::plot_shape::PlotChannelShape;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::semantic::checked_type::{Concreteness, IndexTypeRef, StructTypeRef};
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::dimension::UnitName;
use crate::tir::presentation::DagPresentationFacts;

use super::complete_substitution::CompleteSubstitution;
use super::dag_position::DagPosition;
use super::dag_slots::LocalDagFacts;
use super::instance_graph::InstanceGraph;

fn specialize_dimension(
    dimension: &Dimension,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    dimension
        .iter()
        .try_fold(Dimension::dimensionless(), |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => substitution
                    .dimension(name)
                    .cloned()
                    .unwrap_or_else(|| Dimension::base(base.clone())),
                BaseDimId::Prelude(_) => Dimension::base(base.clone()),
            };
            // A bound dimension raised to the template's exponents can
            // leave the exponent range, like any other dimension arithmetic.
            let overflow = |_| {
                SemanticError::located(src, src.whole_span(), DimensionError::DimensionOverflow)
            };
            let factor = factor.pow(*exponent).map_err(overflow)?;
            acc.checked_mul(&factor).map_err(overflow)
        })
}

fn specialize_index(
    index: &ResolvedIndex,
    substitution: &CompleteSubstitution<'_>,
) -> ResolvedIndex {
    match index {
        ResolvedIndex::Concrete(name, span) => substitution.index(name).map_or_else(
            || index.clone(),
            |target| match target {
                InstanceIndexBindingTarget::Declared(target) => {
                    ResolvedIndex::Concrete(target.clone(), *span)
                }
                InstanceIndexBindingTarget::Finite(target) => {
                    ResolvedIndex::Finite(NatPolyForm::from_constant(target.size_u64()), *span)
                }
            },
        ),
        ResolvedIndex::GenericParam(_, _) | ResolvedIndex::Finite(_, _) => index.clone(),
    }
}

fn specialize_dim_arg(
    dimension: &ResolvedDim,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<ResolvedDim, SemanticError> {
    match dimension {
        ResolvedDim::Concrete(dimension) => {
            specialize_dimension(dimension, substitution, src).map(ResolvedDim::Concrete)
        }
        ResolvedDim::Symbolic { terms, span } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    specialize_dimension(dim, substitution, src).map(|dim| {
                        ResolvedDimTerm::Concrete {
                            dim,
                            power: *power,
                            op: *op,
                        }
                    })
                }
                ResolvedDimTerm::GenericParam { .. } => Ok(term.clone()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|terms| ResolvedDim::Symbolic { terms, span: *span }),
    }
}

fn specialize_value_type(
    resolved: &ResolvedValueType,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<ResolvedValueType, SemanticError> {
    match resolved {
        ResolvedValueType::Quantity(dimension) => {
            specialize_dim_arg(dimension, substitution, src).map(ResolvedValueType::Quantity)
        }
        ResolvedValueType::Complex { dimension, span } => {
            specialize_dim_arg(dimension, substitution, src).map(|dimension| {
                ResolvedValueType::Complex {
                    dimension,
                    span: *span,
                }
            })
        }
        ResolvedValueType::Key { index, span } => Ok(ResolvedValueType::Key {
            index: specialize_index(index, substitution),
            span: *span,
        }),
        ResolvedValueType::Struct {
            name,
            generic_args,
            span,
        } => {
            let generic_args = generic_args
                .iter()
                .map(|argument| match argument {
                    ResolvedGenericArg::Dim(dimension) => {
                        specialize_dim_arg(dimension, substitution, src)
                            .map(ResolvedGenericArg::Dim)
                    }
                    ResolvedGenericArg::Index(index) => Ok(ResolvedGenericArg::Index(
                        specialize_index(index, substitution),
                    )),
                    ResolvedGenericArg::Nat(_, _) => Ok(argument.clone()),
                    ResolvedGenericArg::Type(resolved) => {
                        specialize_value_type(resolved, substitution, src)
                            .map(ResolvedGenericArg::Type)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedValueType::Struct {
                name: substitution.nominal(name).unwrap_or(name).clone(),
                generic_args,
                span: *span,
            })
        }
        ResolvedValueType::Bool
        | ResolvedValueType::Int
        | ResolvedValueType::Datetime(_)
        | ResolvedValueType::GenericTypeParam(_, _) => Ok(resolved.clone()),
    }
}

pub fn specialize_type(
    resolved: &ResolvedDeclType,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<ResolvedDeclType, SemanticError> {
    match resolved {
        ResolvedDeclType::Value(value_type) => {
            specialize_value_type(value_type, substitution, src).map(ResolvedDeclType::Value)
        }
        ResolvedDeclType::Indexed { element, indexes } => Ok(ResolvedDeclType::Indexed {
            element: specialize_value_type(element, substitution, src)?,
            indexes: indexes.map_ref(|index| specialize_index(index, substitution)),
        }),
    }
}

pub fn specialize_index_ref<V: Concreteness>(
    index: &IndexTypeRef<V>,
    substitution: &CompleteSubstitution<'_>,
) -> IndexTypeRef<V> {
    let (Some(source), Some(leaf)) = (index.declared_resolved(), index.declared_name()) else {
        return index.clone();
    };
    match substitution.index(source) {
        Some(InstanceIndexBindingTarget::Declared(target)) => {
            IndexTypeRef::with_display_leaf(leaf.clone(), target.clone())
        }
        Some(InstanceIndexBindingTarget::Finite(target)) => {
            IndexTypeRef::from_finite_index(*target)
        }
        None => index.clone(),
    }
}

fn specialize_struct_ref(
    struct_type: &StructTypeRef,
    substitution: &CompleteSubstitution<'_>,
) -> StructTypeRef {
    substitution.nominal(struct_type.resolved()).map_or_else(
        || struct_type.clone(),
        |target| StructTypeRef::with_display_leaf(struct_type.name().clone(), target.clone()),
    )
}

pub fn specialize_expression_type<V: Concreteness>(
    ty: &crate::semantic::checked_type::CheckedType<V>,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<crate::semantic::checked_type::CheckedType<V>, SemanticError> {
    use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};
    let recurse = |ty: &CheckedType<V>| specialize_expression_type(ty, substitution, src);
    Ok(match ty {
        CheckedType::Quantity(dimension) => {
            CheckedType::Quantity(specialize_dimension(dimension, substitution, src)?)
        }
        CheckedType::Complex(dimension) => {
            CheckedType::Complex(specialize_dimension(dimension, substitution, src)?)
        }
        CheckedType::Key(index) => CheckedType::Key(specialize_index_ref(index, substitution)),
        CheckedType::Indexed { element, index } => CheckedType::Indexed {
            element: Box::new(recurse(element)?),
            index: specialize_index_ref(index, substitution),
        },
        CheckedType::Struct(name, args) => CheckedType::Struct(
            specialize_struct_ref(name, substitution),
            args.iter()
                .map(|arg| {
                    Ok(match arg {
                        CheckedGenericArg::Dim(dimension) => CheckedGenericArg::Dim(
                            specialize_dimension(dimension, substitution, src)?,
                        ),
                        CheckedGenericArg::Index(index) => {
                            CheckedGenericArg::Index(specialize_index_ref(index, substitution))
                        }
                        CheckedGenericArg::Type(ty) => CheckedGenericArg::Type(recurse(ty)?),
                        CheckedGenericArg::Nat(_) => arg.clone(),
                    })
                })
                .collect::<Result<_, SemanticError>>()?,
        ),
        CheckedType::Bool | CheckedType::Int | CheckedType::Datetime(_) => ty.clone(),
    })
}

fn specialize_plot_channel(
    channel: &PlotChannelShape,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<PlotChannelShape, SemanticError> {
    let leaf = match channel.leaf() {
        crate::plot_shape::PlotLeafKind::Quantity(dimension) => {
            crate::plot_shape::PlotLeafKind::Quantity(specialize_dimension(
                dimension,
                substitution,
                src,
            )?)
        }
        crate::plot_shape::PlotLeafKind::Key(index) => {
            crate::plot_shape::PlotLeafKind::Key(specialize_index_ref(index, substitution))
        }
        other => other.clone(),
    };
    let shape = crate::plot_shape::PlotChannelShape::new(
        channel
            .axes()
            .iter()
            .map(|index| specialize_index_ref(index, substitution))
            .collect(),
        leaf,
    );
    Ok(shape)
}

fn specialize_dependencies(
    dependencies: &mut super::ResolvedDagDependencies,
    frame: &InstanceFrame,
) {
    let remap = |values: &HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>| {
        values
            .iter()
            .map(|(declaration, dependencies)| {
                (
                    frame.rebase(declaration),
                    dependencies
                        .iter()
                        .map(|dependency| frame.rebase(dependency))
                        .collect(),
                )
            })
            .collect()
    };
    dependencies.runtime_deps = remap(&dependencies.runtime_deps);
    dependencies.const_deps = remap(&dependencies.const_deps);
}

fn install_override_reconciliations(instance: &mut DagTIR, edge: &HirInstanceRecord) {
    instance.semantic.override_reconciliations = edge
        .override_reconciliations
        .iter()
        .map(|(template_port, reconciliations)| {
            (
                instance_declaration(edge.instance.id(), template_port.to_unowned_def_name()),
                reconciliations.clone(),
            )
        })
        .collect();
}

fn specialize_expected_fail(
    expected: &mut crate::assertion_expectation::ExpectedFail,
    substitution: &StaticSubstitution,
) {
    if let crate::assertion_expectation::ExpectedFail::Variants(keys) = expected {
        for part in keys.iter_mut().flatten() {
            if let crate::assertion_expectation::ExpectedFailKeyPart::Named { index, .. } = part
                && let Some(source) = index.declared_resolved()
                && let Some(replacement) = substitution.indexes.get(source)
            {
                *index = match replacement {
                    InstanceIndexBindingTarget::Declared(target) => {
                        IndexTypeRef::from_resolved(target.clone())
                    }
                    InstanceIndexBindingTarget::Finite(target) => {
                        IndexTypeRef::from_finite_index(*target)
                    }
                };
            }
        }
    }
}

fn compose_index_targets<'a>(
    targets: impl Iterator<Item = &'a mut InstanceIndexBindingTarget>,
    substitution: &StaticSubstitution,
) {
    for target in targets {
        if let InstanceIndexBindingTarget::Declared(source) = target
            && let Some(replacement) = substitution.indexes.get(source)
        {
            *target = replacement.clone();
        }
    }
}

fn compose_type_targets<'a>(
    targets: impl Iterator<Item = &'a mut ResolvedStructTypeName>,
    substitution: &StaticSubstitution,
) {
    for target in targets {
        if let Some(replacement) = substitution.types.get(target) {
            *target = replacement.clone();
        }
    }
}

fn compose_dimension_targets<'a>(
    targets: impl Iterator<Item = &'a mut ResolvedDimName>,
    substitution: &StaticSubstitution,
) {
    for target in targets {
        if let Some(replacement) = substitution.dimensions.get(target) {
            *target = replacement.clone();
        }
    }
}

/// Re-parent one instance edge of the template under the concrete `owner`.
///
/// Every edge in a DAG's instance list is parented by that DAG, so the
/// template's edges are parented by the template and move to `owner`.
fn rebase_nested_instance(
    mut nested: HirInstanceRecord,
    owner: &crate::dag_id::DagId,
    substitution: &StaticSubstitution,
) -> HirInstanceRecord {
    nested
        .instance
        .rebase(owner.clone(), |nested_substitution| {
            compose_index_targets(nested_substitution.indexes.values_mut(), substitution);
            compose_type_targets(nested_substitution.types.values_mut(), substitution);
            compose_dimension_targets(nested_substitution.dimensions.values_mut(), substitution);
        });
    nested
        .assertion_projections
        .iter_mut()
        .filter_map(|projection| projection.expected_fail.as_mut())
        .for_each(|expected| specialize_expected_fail(expected, substitution));
    nested
}

/// Give `instance` (a copy of `template`) the identity and frame of the
/// instance `edge` allocates in the DAG that runs in `parent`.
fn initialize_instance_identity(
    instance: &mut DagTIR,
    template: &DagTIR,
    edge: &HirInstanceRecord,
    parent: &InstanceFrame,
    runtime_units: impl IntoIterator<Item = UnitName>,
) {
    let owner = edge.instance.id().owner();
    instance.dag_id = owner.clone();
    instance.frame = edge.instance.frame(
        super::frame_mint::InstanceFrameMint(()),
        parent,
        template
            .semantic_instances
            .iter()
            .map(|nested| nested.instance.id()),
        runtime_units,
    );
    instance.static_ports.clear();
    instance.semantic_instances = template
        .semantic_instances
        .iter()
        .cloned()
        .map(|nested| rebase_nested_instance(nested, owner, edge.instance.substitution()))
        .collect();
}

fn specialize_instance_declarations(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
    src: SourceId,
) -> Result<(), SemanticError> {
    let owner = edge.instance.id().owner();
    let specialization = edge.instance.specialization();
    // Bound value ports replace the template default of the parameter they
    // name; every declaration moves to the instance owner.
    instance.decls = std::mem::take(&mut instance.decls)
        .rebase(owner, |mut decl| {
            if let crate::ir::entry::Decl::Param(entry) = &mut decl
                && let Some(binding) = edge.value_bindings.get(&entry.identity())
            {
                entry.default = Some(binding.clone());
            }
            decl
        })
        .map_err(|error| {
            SemanticError::internal_error(
                format!("failed to rebase semantic instance `{owner}`: {error}"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    // Attribute tables are keyed by template-owned identities; the instance
    // addresses the same declarations under its own identities.
    let frame = &instance.frame;
    let assumes_map = std::mem::take(&mut instance.assumes_map)
        .into_iter()
        .map(|(assertion, assumers)| {
            (
                frame.rebase(&assertion),
                assumers
                    .iter()
                    .map(|assumer| frame.rebase(assumer))
                    .collect(),
            )
        })
        .collect();
    let expected_fail = std::mem::take(&mut instance.expected_fail)
        .into_iter()
        .map(|(assertion, mut expected)| {
            specialize_expected_fail(&mut expected.expected, &specialization.substitution);
            (frame.rebase(&assertion), expected)
        })
        .collect();
    instance.assumes_map = assumes_map;
    instance.expected_fail = expected_fail;
    Ok(())
}

/// Rebase every dynamic unit scale of `instance` onto the instance frame.
fn rebase_dynamic_unit_scales(instance: &mut DagTIR) {
    instance.semantic.dynamic_unit_scales = instance
        .semantic
        .dynamic_unit_scales
        .iter()
        .map(|(unit, entry)| {
            let unit = instance.frame.rebase(unit);
            let mut entry = entry.clone();
            entry.unit = unit.clone();
            (unit, entry)
        })
        .collect();
}

fn specialize_instance_semantics(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
    substitution: &CompleteSubstitution<'_>,
    src: SourceId,
) -> Result<(), SemanticError> {
    let specialized = instance
        .value_decl_types()
        .map(|(identity, annotation)| {
            specialize_type(annotation.checked().resolved(), substitution, src)
                .and_then(|resolved| super::CheckedDeclType::new(resolved, src))
                .map(|checked| (identity, checked))
        })
        .collect::<Result<_, _>>()?;
    instance.replace_value_decl_types(specialized);
    // The instance's own frame, borrowed beside its semantics: the facts are
    // re-keyed by the DAG that runs them, never by a detached copy.
    let frame = &instance.frame;
    let semantic = &mut instance.semantic;
    semantic.decl_bindings = semantic
        .decl_bindings
        .iter()
        .map(|(name, target)| (name.clone(), frame.rebase(target)))
        .collect();
    specialize_dependencies(&mut semantic.dependencies, frame);
    for (template_port, binding) in &edge.value_bindings {
        // A binding is lowered in the including template and runs here, in
        // the frame that also re-owns the including template's declarations.
        let dependencies = crate::hir::expr::collect_expr_dependencies(binding)
            .graph_refs
            .iter()
            .map(|dependency| frame.resolve(dependency))
            .collect();
        semantic
            .dependencies
            .runtime_deps
            .insert(frame.rebase(template_port), dependencies);
    }
    semantic.domain_bounds = semantic
        .domain_bounds
        .iter()
        .map(|(target, bounds)| (frame.rebase(target), bounds.clone()))
        .collect();
    install_override_reconciliations(instance, edge);
    Ok(())
}

fn clone_checked_instance(
    template_position: super::dag_position::DagPosition,
    template: &DagTIR,
    edge: &HirInstanceRecord,
    parent: &InstanceFrame,
    runtime_units: impl IntoIterator<Item = UnitName>,
    tir: &UncheckedTir,
    src: SourceId,
) -> Result<DagTIR, SemanticError> {
    // An instance rebinding a defaulted dimension port is built from the
    // template's view where that port is rigid, then specialized like a
    // required port.
    let ports = tir
        .project_type_store()
        .bound_defaulted_dimension_ports(&edge.instance.specialization().substitution);
    let rigid_tir;
    let template = if ports.is_empty() {
        template
    } else {
        rigid_tir = super::rigid_dimension_view(tir, template_position, &ports, src)?;
        rigid_tir.dags.at(template_position)
    };
    let mut instance = template.clone();
    initialize_instance_identity(&mut instance, template, edge, parent, runtime_units);
    for (_, dag) in tir.dags.iter() {
        instance
            .semantic
            .type_defs
            .extend_from(&dag.semantic.type_defs);
    }
    specialize_instance_declarations(&mut instance, edge, src)?;
    rebase_dynamic_unit_scales(&mut instance);
    Ok(instance)
}

/// Checked channel shapes of a DAG's plots.
pub type PlotChannels =
    HashMap<ResolvedDeclName, HashMap<crate::syntax::ast::EncodingChannel, PlotChannelShape>>;

/// The presentation facts of one semantic instance: its template's plot
/// channel shapes (or, when it rebinds a defaulted dimension port, the
/// template's shapes in the view where that port is rigid), specialized with
/// its substitution and rebased into its frame.
pub fn instance_presentation_facts(
    frame: &InstanceFrame,
    substitution: &CompleteSubstitution<'_>,
    template_channels: &PlotChannels,
    src: SourceId,
) -> Result<DagPresentationFacts, SemanticError> {
    let plot_channels = template_channels
        .iter()
        .map(|(plot, channels)| {
            channels
                .iter()
                .map(|(encoding, channel)| {
                    specialize_plot_channel(channel, substitution, src)
                        .map(|channel| (*encoding, channel))
                })
                .collect::<Result<_, _>>()
                .map(|channels| (frame.rebase(plot), channels))
        })
        .collect::<Result<_, SemanticError>>()?;
    Ok(DagPresentationFacts { plot_channels })
}

/// The plots each local body projects from its instances, by body, with the
/// projections of each local instance computed first.
#[derive(Default)]
struct PlotProjections {
    projected: HashMap<crate::dag_id::DagId, PlotChannels>,
    visiting: HashSet<crate::dag_id::DagId>,
    complete: HashSet<crate::dag_id::DagId>,
}

impl PlotProjections {
    /// Collect the plots the local body `parent` requests from its
    /// instances, instances first.
    fn collect(
        &mut self,
        tir: &UncheckedTir,
        presentation: &LocalDagFacts<DagPresentationFacts>,
        parent_dag: &DagTIR,
        src: SourceId,
    ) -> Result<(), SemanticError> {
        let parent = parent_dag.dag_id();
        if self.complete.contains(parent) {
            return Ok(());
        }
        if !self.visiting.insert(parent.clone()) {
            return Err(SemanticError::internal_error(
                format!("semantic plot projection cycle reached `{parent}`"),
                src,
                DiagnosticAnchor::WholeFile,
            ));
        }
        let projections = parent_dag
            .semantic_instances()
            .iter()
            .flat_map(|edge| {
                edge.plot_projections.iter().map(|projection| {
                    let instance_owner = edge.instance.id().owner().clone();
                    let target =
                        instance_declaration(edge.instance.id(), projection.target.leaf().clone());
                    let exposed = projection_alias(parent, projection.alias.clone());
                    (instance_owner, target, exposed)
                })
            })
            .collect::<Vec<_>>();
        for (instance_owner, target, exposed) in projections {
            // Imported instances already carry their complete checked
            // projections.
            let local = tir.dags.local_with_fact(presentation, &instance_owner);
            if let Some((instance, _)) = local {
                self.collect(tir, presentation, instance, src)?;
            }
            let local = local.map(|(_, facts)| facts);
            let channels = self
                .projected
                .get(&instance_owner)
                .and_then(|projected| projected.get(&target))
                .or_else(|| {
                    local
                        .or_else(|| {
                            tir.dags
                                .shared(&instance_owner)
                                .map(super::CheckedDag::presentation)
                        })
                        .and_then(|instance| instance.plot_channels.get(&target))
                })
                .cloned()
                .ok_or_else(|| {
                    SemanticError::internal_error(
                        format!(
                            "semantic plot projection `{target}` has no checked presentation facts"
                        ),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            let plot_channels = self.projected.entry(parent.clone()).or_default();
            plot_channels.insert(target, channels.clone());
            plot_channels.insert(exposed, channels);
        }
        self.visiting.remove(parent);
        self.complete.insert(parent.clone());
        Ok(())
    }
}

/// Add the plots each local body projects from its instances to its
/// presentation facts.
///
/// Requested instance plots are copied into their semantic parents from
/// leaves upward, so a body also projects the plots its instances project.
pub fn add_plot_projections(
    tir: &UncheckedTir,
    presentation: LocalDagFacts<DagPresentationFacts>,
    src: SourceId,
) -> Result<LocalDagFacts<DagPresentationFacts>, SemanticError> {
    let mut projections = PlotProjections::default();
    for (_, parent) in tir.dags.local_iter() {
        projections.collect(tir, &presentation, parent, src)?;
    }
    let mut projected = projections.projected;
    Ok(tir.dags.map_local_facts(presentation, |dag, mut facts| {
        if let Some(plots) = projected.remove(dag.dag_id()) {
            facts.plot_channels.extend(plots);
        }
        facts
    }))
}

/// Bind the names each body exposes from its instances' projections.
fn install_semantic_projection_bindings(tir: &mut UncheckedTir) {
    for dag in tir.dags.locals_mut() {
        for edge in dag.semantic_instances.clone() {
            for projection in &edge.output_projections {
                match projection.body() {
                    crate::ir::instance::ExposedValueBody::LocalAlias => {}
                    crate::ir::instance::ExposedValueBody::Instance => {
                        dag.semantic.decl_bindings.insert(
                            edge.instance.exposed_name(projection),
                            instance_declaration(
                                edge.instance.id(),
                                crate::ir::instance::InstanceProjection::target(projection)
                                    .leaf()
                                    .clone(),
                            ),
                        );
                    }
                }
            }
            for projection in &edge.assertion_projections {
                dag.semantic.decl_bindings.insert(
                    edge.instance.exposed_name(projection),
                    instance_declaration(edge.instance.id(), projection.target.leaf().clone()),
                );
            }
        }
    }
}

/// Materialize the instance `edge` allocates in the DAG at `parent`, which
/// holds the edge, and return its position.
fn instantiate_semantic_edge(
    tir: &mut UncheckedTir,
    graph: &mut InstanceGraph,
    parent: DagPosition,
    edge: &HirInstanceRecord,
    src: SourceId,
) -> Result<DagPosition, SemanticError> {
    let owner = edge.instance.id().owner();
    let template_body = graph
        .template(&tir.dags, edge.instance.id().template())
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "checked template `{}` is unavailable for semantic instance `{owner}`",
                    edge.instance.id().template()
                ),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    let template_position = template_body.0;
    let template = tir.dags.at(template_position).clone();
    let substitution =
        CompleteSubstitution::try_new(edge.instance.substitution(), tir.project_type_store())
            .map_err(|error| error.into_graphcal(src))?;
    let runtime_unit_names = edge
        .runtime_unit_names
        .iter()
        .cloned()
        .chain(
            template
                .semantic
                .dynamic_unit_scales
                .keys()
                .map(crate::resolved_name::ResolvedName::to_unowned_def_name),
        )
        .collect::<BTreeSet<_>>();
    let runtime_unit_infos = runtime_unit_names
        .iter()
        .cloned()
        .map(|name| {
            let source: ResolvedUnitName = template_declaration(edge.instance.id(), name.clone());
            let mut info = tir.unit_info(&source).cloned().ok_or_else(|| {
                SemanticError::internal_error(
                    format!("template runtime unit `{source}` has no checked definition"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            info.dimension = specialize_dimension(&info.dimension, &substitution, src)?;
            Ok((instance_declaration(edge.instance.id(), name), info))
        })
        .collect::<Result<Vec<_>, SemanticError>>()?;
    // The edge is materialized in the frame of the DAG that includes it: the
    // instance's parent, whose record list holds the edge.
    let parent_frame = tir.dags.at(parent).frame();
    let mut instance = clone_checked_instance(
        template_position,
        &template,
        edge,
        parent_frame,
        runtime_unit_names,
        tir,
        src,
    )?;
    specialize_instance_semantics(&mut instance, edge, &substitution, src)?;
    for (unit, info) in runtime_unit_infos {
        tir.insert_runtime_unit(unit, info).map_err(|error| {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
    }
    graph
        .push_instance(
            &mut tir.dags,
            instance,
            template_body,
            edge.instance.specialization().clone(),
        )
        .map_err(|error| {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })
}

/// Materialize checked semantic include edges as concrete TIR DAG instances,
/// and return the graph of every DAG's edges by position.
///
/// Template bodies are reused after the Option A closure check; only checked
/// signatures, concrete owners, and value-binding environments are specialized.
///
/// Every DAG's edges are followed once: first those of every DAG of the
/// program in visiting order, then, round by round, those of the instances
/// the previous round materialized, in identity order.
pub fn instantiate_semantic_edges(
    tir: &mut UncheckedTir,
    src: SourceId,
) -> Result<InstanceGraph, SemanticError> {
    let mut graph = InstanceGraph::new(&tir.dags);
    let mut holders = tir.dags.iter_positions().collect::<Vec<_>>();
    while !holders.is_empty() {
        let edges = holders
            .iter()
            .flat_map(|&holder| {
                tir.dags
                    .at(holder)
                    .semantic_instances()
                    .iter()
                    .cloned()
                    .map(move |edge| (holder, edge))
            })
            .collect::<Vec<_>>();
        let mut materialized = Vec::new();
        for (holder, edge) in edges {
            // An instance an imported store or an earlier edge already holds
            // is reused.
            if let Some(instance) = tir.dags.position(edge.instance.id().owner()) {
                graph.record_edge(holder, instance);
                continue;
            }
            let instance = instantiate_semantic_edge(tir, &mut graph, holder, &edge, src)?;
            materialized.push(instance);
            graph.record_edge(holder, instance);
        }
        materialized.sort_by(|left, right| {
            tir.dags
                .at(*left)
                .dag_id()
                .cmp(tir.dags.at(*right).dag_id())
        });
        holders = materialized;
    }
    install_semantic_projection_bindings(tir);
    Ok(graph)
}

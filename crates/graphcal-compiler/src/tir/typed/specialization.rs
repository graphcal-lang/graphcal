//! Checked-DAG specialization for semantic include instances.

use crate::semantic_error::dimension::DimensionError;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::{
    DagTIR, ProjectTypeStore, ResolvedDeclType, ResolvedDim, ResolvedDimTerm, ResolvedGenericArg,
    ResolvedIndex, ResolvedValueType, UncheckedTir,
};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::{BaseDimId, Dimension};
use crate::ir::instance::HirInstanceRecord;
use crate::ir::instance::frame::InstanceFrame;
use crate::ir::instance::identity::{instance_declaration, projection_alias, template_declaration};
use crate::ir::static_substitution::{
    InstanceIndexBindingTarget, StaticSpecializationId, StaticSubstitution,
};
use crate::nat::NatPolyForm;
use crate::plot_shape::PlotChannelShape;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
use crate::semantic::checked_type::{Concreteness, IndexTypeRef, StructTypeRef};
use crate::semantic_error::SemanticError;
use crate::source_id::SourceId;
use crate::syntax::dimension::UnitName;
use crate::tir::presentation::DagPresentationFacts;

fn dimension_substitution<'a>(
    substitution: &'a StaticSubstitution,
    source: &ResolvedDimName,
) -> Option<&'a ResolvedDimName> {
    substitution.dimensions.get(source)
}

fn index_substitution<'a>(
    substitution: &'a StaticSubstitution,
    source: &ResolvedIndexName,
) -> Option<&'a InstanceIndexBindingTarget> {
    substitution.indexes.get(source)
}

fn type_substitution<'a>(
    substitution: &'a StaticSubstitution,
    source: &ResolvedStructTypeName,
) -> Option<&'a ResolvedStructTypeName> {
    substitution.types.get(source)
}

fn specialize_dimension(
    dimension: &Dimension,
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: SourceId,
) -> Result<Dimension, SemanticError> {
    dimension.iter().try_fold(
        Dimension::dimensionless(),
        |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => dimension_substitution(substitution, name).map_or_else(
                    || Ok(Dimension::base(base.clone())),
                    |target| {
                        types.get_dimension(target).cloned().ok_or_else(|| {
                            SemanticError::internal_error(
                                format!(
                                    "semantic specialization dimension target `{target}` is unavailable"
                                ),
                                src,
                                DiagnosticAnchor::WholeFile,
                            )
                        })
                    },
                )?,
                BaseDimId::Prelude(_) => Dimension::base(base.clone()),
            };
            // A bound dimension raised to the template's exponents can
            // leave the exponent range, like any other dimension arithmetic.
            let overflow =
                |_| SemanticError::located(src, src.whole_span(), DimensionError::DimensionOverflow);
            let factor = factor.pow(*exponent).map_err(overflow)?;
            acc.checked_mul(&factor).map_err(overflow)
        },
    )
}

fn specialize_index(index: &ResolvedIndex, substitution: &StaticSubstitution) -> ResolvedIndex {
    match index {
        ResolvedIndex::Concrete(name, span) => index_substitution(substitution, name).map_or_else(
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
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: SourceId,
) -> Result<ResolvedDim, SemanticError> {
    match dimension {
        ResolvedDim::Concrete(dimension) => {
            specialize_dimension(dimension, substitution, types, src).map(ResolvedDim::Concrete)
        }
        ResolvedDim::Symbolic { terms, span } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    specialize_dimension(dim, substitution, types, src).map(|dim| {
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
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: SourceId,
) -> Result<ResolvedValueType, SemanticError> {
    match resolved {
        ResolvedValueType::Quantity(dimension) => {
            specialize_dim_arg(dimension, substitution, types, src).map(ResolvedValueType::Quantity)
        }
        ResolvedValueType::Complex { dimension, span } => {
            specialize_dim_arg(dimension, substitution, types, src).map(|dimension| {
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
                        specialize_dim_arg(dimension, substitution, types, src)
                            .map(ResolvedGenericArg::Dim)
                    }
                    ResolvedGenericArg::Index(index) => Ok(ResolvedGenericArg::Index(
                        specialize_index(index, substitution),
                    )),
                    ResolvedGenericArg::Nat(_, _) => Ok(argument.clone()),
                    ResolvedGenericArg::Type(resolved) => {
                        specialize_value_type(resolved, substitution, types, src)
                            .map(ResolvedGenericArg::Type)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedValueType::Struct {
                name: type_substitution(substitution, name)
                    .unwrap_or(name)
                    .clone(),
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
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: SourceId,
) -> Result<ResolvedDeclType, SemanticError> {
    match resolved {
        ResolvedDeclType::Value(value_type) => {
            specialize_value_type(value_type, substitution, types, src).map(ResolvedDeclType::Value)
        }
        ResolvedDeclType::Indexed { element, indexes } => Ok(ResolvedDeclType::Indexed {
            element: specialize_value_type(element, substitution, types, src)?,
            indexes: indexes.map_ref(|index| specialize_index(index, substitution)),
        }),
    }
}

pub fn specialize_index_ref<V: Concreteness>(
    index: &IndexTypeRef<V>,
    substitution: &StaticSubstitution,
) -> IndexTypeRef<V> {
    let (Some(source), Some(leaf)) = (index.declared_resolved(), index.declared_name()) else {
        return index.clone();
    };
    match index_substitution(substitution, source) {
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
    substitution: &StaticSubstitution,
) -> StructTypeRef {
    type_substitution(substitution, struct_type.resolved()).map_or_else(
        || struct_type.clone(),
        |target| StructTypeRef::with_display_leaf(struct_type.name().clone(), target.clone()),
    )
}

pub fn specialize_expression_type<V: Concreteness>(
    ty: &crate::semantic::checked_type::CheckedType<V>,
    substitution: &StaticSubstitution,
    tir: &dyn super::TirRead,
    src: SourceId,
) -> Result<crate::semantic::checked_type::CheckedType<V>, SemanticError> {
    use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};
    let recurse = |ty: &CheckedType<V>| specialize_expression_type(ty, substitution, tir, src);
    Ok(match ty {
        CheckedType::Quantity(dimension) => CheckedType::Quantity(specialize_dimension(
            dimension,
            substitution,
            tir.project_type_store(),
            src,
        )?),
        CheckedType::Complex(dimension) => CheckedType::Complex(specialize_dimension(
            dimension,
            substitution,
            tir.project_type_store(),
            src,
        )?),
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
                        CheckedGenericArg::Dim(dimension) => {
                            CheckedGenericArg::Dim(specialize_dimension(
                                dimension,
                                substitution,
                                tir.project_type_store(),
                                src,
                            )?)
                        }
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
    substitution: &StaticSubstitution,
    tir: &UncheckedTir,
    src: SourceId,
) -> Result<PlotChannelShape, SemanticError> {
    let leaf = match channel.leaf() {
        crate::plot_shape::PlotLeafKind::Quantity(dimension) => {
            crate::plot_shape::PlotLeafKind::Quantity(specialize_dimension(
                dimension,
                substitution,
                tir.project_type_store(),
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
                && let Some(replacement) = index_substitution(substitution, source)
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
            && let Some(replacement) = index_substitution(substitution, source)
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
        if let Some(replacement) = type_substitution(substitution, target) {
            *target = replacement.clone();
        }
    }
}

fn compose_dimension_targets<'a>(
    targets: impl Iterator<Item = &'a mut ResolvedDimName>,
    substitution: &StaticSubstitution,
) {
    for target in targets {
        if let Some(replacement) = dimension_substitution(substitution, target) {
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

fn specialize_dynamic_unit_scales(
    instance: &mut DagTIR,
    specialization: &StaticSpecializationId,
    tir: &UncheckedTir,
    src: SourceId,
) -> Result<(), SemanticError> {
    instance.semantic.dynamic_unit_scales = instance
        .semantic
        .dynamic_unit_scales
        .iter()
        .map(|(unit, entry)| {
            let unit = instance.frame.rebase(unit);
            let mut entry = entry.clone();
            entry.unit = unit.clone();
            entry.declared_dimension = specialize_dimension(
                &entry.declared_dimension,
                &specialization.substitution,
                tir.project_type_store(),
                src,
            )?;
            entry.base_unit_dimension = specialize_dimension(
                &entry.base_unit_dimension,
                &specialization.substitution,
                tir.project_type_store(),
                src,
            )?;
            Ok((unit, entry))
        })
        .collect::<Result<_, SemanticError>>()?;
    Ok(())
}

fn specialize_instance_semantics(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
    tir: &UncheckedTir,
    src: SourceId,
) -> Result<(), SemanticError> {
    let specialization = edge.instance.specialization();
    let specialized = instance
        .value_decl_types()
        .map(|(identity, annotation)| {
            specialize_type(
                annotation.checked().resolved(),
                &specialization.substitution,
                tir.project_type_store(),
                src,
            )
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
        rigid_tir = super::rigid_dimension_view(tir, template.dag_id(), &ports, src)?;
        rigid_tir.dags.get(template.dag_id()).ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "rigid template `{}` is unavailable for semantic instance",
                    template.dag_id()
                ),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?
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
    specialize_dynamic_unit_scales(&mut instance, edge.instance.specialization(), tir, src)?;
    specialize_instance_semantics(&mut instance, edge, tir, src)?;
    Ok(instance)
}

/// Checked channel shapes of a DAG's plots.
pub type PlotChannels =
    HashMap<ResolvedDeclName, HashMap<crate::syntax::ast::EncodingChannel, PlotChannelShape>>;

/// The checked presentation facts of `dag_id`: a local body's from
/// `presentation`, an imported body's from its checked handle.
fn checked_presentation<'a>(
    tir: &'a UncheckedTir,
    presentation: &'a HashMap<crate::dag_id::DagId, DagPresentationFacts>,
    dag_id: &crate::dag_id::DagId,
) -> Option<&'a DagPresentationFacts> {
    presentation
        .get(dag_id)
        .or_else(|| tir.dags.shared(dag_id).map(super::CheckedDag::presentation))
}

fn specialize_instance_presentation_facts(
    tir: &UncheckedTir,
    presentation: &HashMap<crate::dag_id::DagId, DagPresentationFacts>,
    port_generic_plot_channels: &HashMap<crate::dag_id::DagId, PlotChannels>,
    src: SourceId,
) -> Result<Vec<(crate::dag_id::DagId, DagPresentationFacts)>, SemanticError> {
    tir.dags
        .local_iter()
        .filter_map(|(owner, dag)| {
            dag.frame()
                .specialization()
                .map(|specialization| (owner, specialization, dag.frame()))
        })
        .map(|(owner, specialization, frame)| {
            let template = checked_presentation(tir, presentation, &specialization.template)
                .ok_or_else(|| {
                    SemanticError::internal_error(
                        format!(
                            "semantic instance `{owner}` has no presentation template `{}`",
                            specialization.template
                        ),
                        src,
                        DiagnosticAnchor::WholeFile,
                    )
                })?;
            let plot_channels = port_generic_plot_channels
                .get(owner)
                .unwrap_or(&template.plot_channels)
                .iter()
                .map(|(plot, channels)| {
                    channels
                        .iter()
                        .map(|(encoding, channel)| {
                            specialize_plot_channel(channel, &specialization.substitution, tir, src)
                                .map(|channel| (*encoding, channel))
                        })
                        .collect::<Result<_, _>>()
                        .map(|channels| (frame.rebase(plot), channels))
                })
                .collect::<Result<_, SemanticError>>()?;
            Ok((owner.clone(), DagPresentationFacts { plot_channels }))
        })
        .collect()
}

/// Copy the plots `parent` requests from its instances into its presentation,
/// instances first.
fn add_plot_projections_for_dag(
    tir: &UncheckedTir,
    presentation: &mut HashMap<crate::dag_id::DagId, DagPresentationFacts>,
    parent: &crate::dag_id::DagId,
    visiting: &mut HashSet<crate::dag_id::DagId>,
    complete: &mut HashSet<crate::dag_id::DagId>,
    src: SourceId,
) -> Result<(), SemanticError> {
    if complete.contains(parent) {
        return Ok(());
    }
    if !visiting.insert(parent.clone()) {
        return Err(SemanticError::internal_error(
            format!("semantic plot projection cycle reached `{parent}`"),
            src,
            DiagnosticAnchor::WholeFile,
        ));
    }
    let projections = tir
        .dags
        .get(parent)
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!("semantic plot projection parent `{parent}` is unavailable"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?
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
        // Imported instances already carry their complete checked projections.
        if presentation.contains_key(&instance_owner) {
            add_plot_projections_for_dag(
                tir,
                presentation,
                &instance_owner,
                visiting,
                complete,
                src,
            )?;
        }
        let channels = checked_presentation(tir, presentation, &instance_owner)
            .and_then(|instance| instance.plot_channels.get(&target))
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
        let plot_channels = &mut presentation
            .entry(parent.clone())
            .or_default()
            .plot_channels;
        plot_channels.insert(target, channels.clone());
        plot_channels.insert(exposed, channels);
    }
    visiting.remove(parent);
    complete.insert(parent.clone());
    Ok(())
}

/// Add semantic instances' specialized presentation facts, and the plots each
/// body projects from its instances, to the canonical local `presentation`.
///
/// An instance specializes its template's plot shapes, or, when it rebinds a
/// defaulted dimension port, the template's shapes in the view where that
/// port is rigid (`port_generic_plot_channels`, keyed by instance). Requested
/// instance plots are then copied into their semantic parents from leaves
/// upward.
pub fn add_semantic_presentation_facts(
    tir: &UncheckedTir,
    presentation: &mut HashMap<crate::dag_id::DagId, DagPresentationFacts>,
    port_generic_plot_channels: &HashMap<crate::dag_id::DagId, PlotChannels>,
    src: SourceId,
) -> Result<(), SemanticError> {
    let instances =
        specialize_instance_presentation_facts(tir, presentation, port_generic_plot_channels, src)?;
    presentation.extend(instances);
    let mut visiting = HashSet::new();
    let mut complete = HashSet::new();
    tir.dags.local_iter().try_for_each(|(parent, _)| {
        add_plot_projections_for_dag(tir, presentation, parent, &mut visiting, &mut complete, src)
    })
}

/// Bind the names each body exposes from its instances' projections.
fn install_semantic_projection_bindings(tir: &mut UncheckedTir) {
    for dag in tir.dags.values_mut() {
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

fn instantiate_semantic_edge(
    tir: &mut UncheckedTir,
    edge: &HirInstanceRecord,
    src: SourceId,
) -> Result<(), SemanticError> {
    let owner = edge.instance.id().owner();
    let parent = edge.instance.id().parent();
    let template = tir
        .dags
        .get(edge.instance.id().template())
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!(
                    "checked template `{}` is unavailable for semantic instance `{owner}`",
                    edge.instance.id().template()
                ),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?
        .clone();
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
            info.dimension = specialize_dimension(
                &info.dimension,
                edge.instance.substitution(),
                tir.project_type_store(),
                src,
            )?;
            Ok((instance_declaration(edge.instance.id(), name), info))
        })
        .collect::<Result<Vec<_>, SemanticError>>()?;
    // The edge is materialized in the frame of the DAG that includes it: the
    // instance's parent, whose record list holds the edge.
    let parent_frame = tir
        .dags
        .get(parent)
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!("semantic instance `{owner}` has no including DAG `{parent}`"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?
        .frame();
    let instance =
        clone_checked_instance(&template, edge, parent_frame, runtime_unit_names, tir, src)?;
    for (unit, info) in runtime_unit_infos {
        tir.insert_runtime_unit(unit, info).map_err(|error| {
            SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
    }
    tir.insert_materialized_dag(instance).map_err(|error| {
        SemanticError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;
    Ok(())
}

/// Materialize checked semantic include edges as concrete TIR DAG instances.
///
/// Template bodies are reused after the Option A closure check; only checked
/// signatures, concrete owners, and value-binding environments are specialized.
pub fn instantiate_semantic_edges(
    tir: &mut UncheckedTir,
    src: SourceId,
) -> Result<(), SemanticError> {
    loop {
        let edges = tir
            .dags
            .iter()
            .flat_map(|(_, dag)| dag.semantic_instances().iter().cloned())
            .filter(|edge| tir.dags.get(edge.instance.id().owner()).is_none())
            .collect::<Vec<_>>();
        if edges.is_empty() {
            install_semantic_projection_bindings(tir);
            return Ok(());
        }
        for edge in edges {
            instantiate_semantic_edge(tir, &edge, src)?;
        }
    }
}

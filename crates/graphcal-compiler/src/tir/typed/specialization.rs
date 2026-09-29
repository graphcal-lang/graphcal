//! Checked-DAG specialization for semantic include instances.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use super::{
    DagTIR, ProjectTypeStore, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex,
    ResolvedTypeExpr, TIR,
};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::{BaseDimId, Dimension};
use crate::ir::instance::HirInstanceRecord;
use crate::ir::static_substitution::{
    InstanceIndexBindingTarget, StaticSpecializationId, StaticSubstitution,
};
use crate::nat::NatPolyForm;
use crate::plot_shape::PlotChannelShape;
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::error::GraphcalError;
use crate::resolved_name::{
    ResolvedDeclName, ResolvedDimName, ResolvedIndexName, ResolvedStructTypeName, ResolvedUnitName,
};
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
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    dimension.iter().try_fold(
        Dimension::dimensionless(),
        |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => dimension_substitution(substitution, name).map_or_else(
                    || Ok(Dimension::base(base.clone())),
                    |target| {
                        types.get_dimension(target).cloned().ok_or_else(|| {
                            GraphcalError::internal_error(
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
            let factor = factor.pow(*exponent).map_err(|error| {
                GraphcalError::internal_error(
                    format!("semantic dimension substitution overflowed: {error}"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            acc.checked_mul(&factor).map_err(|error| {
                GraphcalError::internal_error(
                    format!("semantic dimension substitution overflowed: {error}"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })
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
    dimension: &ResolvedDimArg,
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDimArg, GraphcalError> {
    match dimension {
        ResolvedDimArg::Concrete(dimension) => {
            specialize_dimension(dimension, substitution, types, src).map(ResolvedDimArg::Concrete)
        }
        ResolvedDimArg::Expr { terms, span } => terms
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
            .map(|terms| ResolvedDimArg::Expr { terms, span: *span }),
        ResolvedDimArg::Dimensionless | ResolvedDimArg::GenericParam(_, _) => Ok(dimension.clone()),
    }
}

pub fn specialize_type(
    resolved: &ResolvedTypeExpr,
    substitution: &StaticSubstitution,
    types: &ProjectTypeStore,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let recurse = |resolved: &ResolvedTypeExpr| specialize_type(resolved, substitution, types, src);
    match resolved {
        ResolvedTypeExpr::Quantity(dimension) => {
            specialize_dimension(dimension, substitution, types, src)
                .map(ResolvedTypeExpr::Quantity)
        }
        ResolvedTypeExpr::Complex { dimension, span } => {
            specialize_dim_arg(dimension, substitution, types, src).map(|dimension| {
                ResolvedTypeExpr::Complex {
                    dimension,
                    span: *span,
                }
            })
        }
        ResolvedTypeExpr::Key { index, span } => Ok(ResolvedTypeExpr::Key {
            index: specialize_index(index, substitution),
            span: *span,
        }),
        ResolvedTypeExpr::Struct(name, span) => Ok(ResolvedTypeExpr::Struct(
            type_substitution(substitution, name)
                .unwrap_or(name)
                .clone(),
            *span,
        )),
        ResolvedTypeExpr::GenericStruct {
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
                        recurse(resolved).map(ResolvedGenericArg::Type)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericStruct {
                name: type_substitution(substitution, name)
                    .unwrap_or(name)
                    .clone(),
                generic_args,
                span: *span,
            })
        }
        ResolvedTypeExpr::GenericDimExpr { terms, span } => {
            let terms = terms
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
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResolvedTypeExpr::GenericDimExpr { terms, span: *span })
        }
        ResolvedTypeExpr::Indexed { base, indexes } => Ok(ResolvedTypeExpr::Indexed {
            base: Box::new(recurse(base)?),
            indexes: indexes
                .iter()
                .map(|index| specialize_index(index, substitution))
                .collect(),
        }),
        ResolvedTypeExpr::Dimensionless
        | ResolvedTypeExpr::Bool
        | ResolvedTypeExpr::Int
        | ResolvedTypeExpr::Datetime(_)
        | ResolvedTypeExpr::GenericDimParam(_, _)
        | ResolvedTypeExpr::GenericTypeParam(_, _) => Ok(resolved.clone()),
    }
}

pub fn specialize_index_ref(
    index: &IndexTypeRef,
    substitution: &StaticSubstitution,
) -> IndexTypeRef {
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

pub fn specialize_expression_type(
    ty: &crate::registry::declared_type::DeclaredType,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::registry::declared_type::DeclaredType, GraphcalError> {
    use crate::registry::declared_type::{DeclaredGenericArg, DeclaredType};
    let recurse = |ty: &DeclaredType| specialize_expression_type(ty, substitution, tir, src);
    Ok(match ty {
        DeclaredType::Quantity(dimension) => DeclaredType::Quantity(specialize_dimension(
            dimension,
            substitution,
            tir.project_type_store(),
            src,
        )?),
        DeclaredType::Complex(dimension) => DeclaredType::Complex(specialize_dimension(
            dimension,
            substitution,
            tir.project_type_store(),
            src,
        )?),
        DeclaredType::Key(index) => DeclaredType::Key(specialize_index_ref(index, substitution)),
        DeclaredType::Indexed { element, index } => DeclaredType::Indexed {
            element: Box::new(recurse(element)?),
            index: specialize_index_ref(index, substitution),
        },
        DeclaredType::Struct(name, args) => DeclaredType::Struct(
            specialize_struct_ref(name, substitution),
            args.iter()
                .map(|arg| {
                    Ok(match arg {
                        DeclaredGenericArg::Dim(dimension) => {
                            DeclaredGenericArg::Dim(specialize_dimension(
                                dimension,
                                substitution,
                                tir.project_type_store(),
                                src,
                            )?)
                        }
                        DeclaredGenericArg::Index(index) => {
                            DeclaredGenericArg::Index(specialize_index_ref(index, substitution))
                        }
                        DeclaredGenericArg::Type(ty) => DeclaredGenericArg::Type(recurse(ty)?),
                        DeclaredGenericArg::Nat(_) => arg.clone(),
                    })
                })
                .collect::<Result<_, GraphcalError>>()?,
        ),
        DeclaredType::Bool | DeclaredType::Int | DeclaredType::Datetime(_) => ty.clone(),
    })
}

fn rebase_runtime_decl(
    declaration: &ResolvedDeclName,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> ResolvedDeclName {
    runtime_owner_rebases.get(declaration.owner()).map_or_else(
        || declaration.clone(),
        |owner| ResolvedDeclName::from_def(owner.clone(), declaration.to_unowned_def_name()),
    )
}

fn specialize_plot_channel(
    channel: &PlotChannelShape,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<PlotChannelShape, GraphcalError> {
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

fn instance_decl(
    target: &ResolvedDeclName,
    specialization: &StaticSpecializationId,
    owner: &crate::dag_id::DagId,
) -> ResolvedDeclName {
    if target.owner() == &specialization.template {
        local_instance_decl(target, owner)
    } else {
        target.clone()
    }
}

fn local_instance_decl(
    target: &ResolvedDeclName,
    owner: &crate::dag_id::DagId,
) -> ResolvedDeclName {
    ResolvedDeclName::from_def(owner.clone(), target.to_unowned_def_name())
}

fn specialize_dependencies(
    dependencies: &mut super::ResolvedDagDependencies,
    specialization: &StaticSpecializationId,
    owner: &crate::dag_id::DagId,
) {
    let remap = |values: &HashMap<ResolvedDeclName, BTreeSet<ResolvedDeclName>>| {
        values
            .iter()
            .map(|(declaration, dependencies)| {
                (
                    local_instance_decl(declaration, owner),
                    dependencies
                        .iter()
                        .map(|dependency| instance_decl(dependency, specialization, owner))
                        .collect(),
                )
            })
            .collect()
    };
    dependencies.runtime_deps = remap(&dependencies.runtime_deps);
    dependencies.const_deps = remap(&dependencies.const_deps);
}

fn install_override_reconciliations(instance: &mut DagTIR, edge: &HirInstanceRecord) {
    let owner = edge.instance.id().owner();
    instance.semantic.override_reconciliations = edge
        .override_reconciliations
        .iter()
        .map(|(template_port, reconciliations)| {
            (
                ResolvedDeclName::from_def(owner.clone(), template_port.to_unowned_def_name()),
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
    runtime_owner_rebases: &mut HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> HirInstanceRecord {
    let template_nested_owner = nested.instance.id().owner().clone();
    nested
        .instance
        .rebase(owner.clone(), |nested_substitution| {
            compose_index_targets(nested_substitution.indexes.values_mut(), substitution);
            compose_type_targets(nested_substitution.types.values_mut(), substitution);
            compose_dimension_targets(nested_substitution.dimensions.values_mut(), substitution);
        });
    runtime_owner_rebases.insert(template_nested_owner, nested.instance.id().owner().clone());
    nested
        .assertion_projections
        .iter_mut()
        .filter_map(|projection| projection.expected_fail.as_mut())
        .for_each(|expected| specialize_expected_fail(expected, substitution));
    nested.owner_rebases.extend(runtime_owner_rebases.clone());
    nested
}

fn initialize_instance_identity(
    instance: &mut DagTIR,
    template: &DagTIR,
    edge: &HirInstanceRecord,
) {
    let owner = edge.instance.id().owner();
    let specialization = edge.instance.specialization();
    instance.dag_id = owner.clone();
    instance.begin_checking_revision();
    instance.semantic_specialization = Some(specialization.clone());
    instance.static_ports.clear();
    instance
        .runtime_owner_rebases
        .clone_from(&edge.owner_rebases);
    instance
        .runtime_owner_rebases
        .insert(specialization.template.clone(), owner.clone());
    for declaration_owner in template
        .decls()
        .iter()
        .map(crate::ir::entry::Decl::declaration_owner)
    {
        instance
            .runtime_owner_rebases
            .insert(declaration_owner.clone(), owner.clone());
    }
    instance.semantic_instances = template
        .semantic_instances
        .iter()
        .cloned()
        .map(|nested| {
            rebase_nested_instance(
                nested,
                owner,
                &specialization.substitution,
                &mut instance.runtime_owner_rebases,
            )
        })
        .collect();
}

fn specialize_instance_declarations(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
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
            GraphcalError::internal_error(
                format!("failed to rebase semantic instance `{owner}`: {error}"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
    // Attribute tables are keyed by template-owned identities; the instance
    // addresses the same declarations under its runtime identities.
    instance.assumes_map = std::mem::take(&mut instance.assumes_map)
        .into_iter()
        .map(|(assertion, assumers)| {
            (
                instance.runtime_decl_identity(&assertion),
                assumers
                    .iter()
                    .map(|assumer| instance.runtime_decl_identity(assumer))
                    .collect(),
            )
        })
        .collect();
    instance.expected_fail = std::mem::take(&mut instance.expected_fail)
        .into_iter()
        .map(|(assertion, mut expected)| {
            specialize_expected_fail(&mut expected.expected, &specialization.substitution);
            (instance.runtime_decl_identity(&assertion), expected)
        })
        .collect();
    Ok(())
}

fn specialize_dynamic_unit_scales(
    instance: &mut DagTIR,
    specialization: &StaticSpecializationId,
    owner: &crate::dag_id::DagId,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    instance.semantic.dynamic_unit_scales = instance
        .semantic
        .dynamic_unit_scales
        .iter()
        .map(|(unit, entry)| {
            let unit_owner = if unit.owner() == &specialization.template {
                owner.clone()
            } else {
                instance
                    .runtime_owner_rebases
                    .get(unit.owner())
                    .cloned()
                    .unwrap_or_else(|| unit.owner().clone())
            };
            let unit = ResolvedUnitName::from_def(unit_owner, unit.to_unowned_def_name());
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
        .collect::<Result<_, GraphcalError>>()?;
    Ok(())
}

fn specialize_instance_semantics(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let owner = edge.instance.id().owner();
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
    instance.semantic.decl_bindings = instance
        .semantic
        .decl_bindings
        .iter()
        .map(|(name, target)| (name.clone(), instance_decl(target, specialization, owner)))
        .collect();
    specialize_dependencies(&mut instance.semantic.dependencies, specialization, owner);
    for (template_port, binding) in &edge.value_bindings {
        let instance_port = instance_decl(template_port, specialization, owner);
        let dependencies = crate::hir::collect_expr_dependencies(binding)
            .graph_refs
            .iter()
            .map(|dependency| instance_decl(dependency, specialization, owner))
            .collect();
        instance
            .semantic
            .dependencies
            .runtime_deps
            .insert(instance_port, dependencies);
    }
    instance.semantic.presentation.plot_channels.clear();
    // Concrete Static index bindings can change cardinality. Instance checking
    // specializes the canonical template's retained facts before publication;
    // interpretation must not reconstruct axes from the source defaults.
    instance.semantic.expression_facts = None;
    instance.semantic.domain_bounds = instance
        .semantic
        .domain_bounds
        .iter()
        .map(|(target, bounds)| (local_instance_decl(target, owner), bounds.clone()))
        .collect();
    install_override_reconciliations(instance, edge);
    Ok(())
}

fn clone_checked_instance(
    template: &DagTIR,
    edge: &HirInstanceRecord,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<DagTIR, GraphcalError> {
    let mut instance = template.clone();
    initialize_instance_identity(&mut instance, template, edge);
    for (_, dag) in tir.dags.iter() {
        instance
            .semantic
            .type_defs
            .extend_from(&dag.semantic.type_defs);
    }
    specialize_instance_declarations(&mut instance, edge, src)?;
    specialize_dynamic_unit_scales(
        &mut instance,
        edge.instance.specialization(),
        edge.instance.id().owner(),
        tir,
        src,
    )?;
    specialize_instance_semantics(&mut instance, edge, tir, src)?;
    Ok(instance)
}

fn specialize_instance_presentation_facts(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<(crate::dag_id::DagId, DagPresentationFacts)>, GraphcalError> {
    tir.dags
        .local_iter()
        .filter_map(|(owner, dag)| {
            dag.semantic_specialization
                .as_ref()
                .map(|specialization| (owner, specialization, &dag.runtime_owner_rebases))
        })
        .map(|(owner, specialization, runtime_owner_rebases)| {
            let template = tir.dags.get(&specialization.template).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic instance `{owner}` has no presentation template `{}`",
                        specialization.template
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            let plot_channels = template
                .semantic
                .presentation
                .plot_channels
                .iter()
                .map(|(plot, channels)| {
                    channels
                        .iter()
                        .map(|(encoding, channel)| {
                            specialize_plot_channel(channel, &specialization.substitution, tir, src)
                                .map(|channel| (*encoding, channel))
                        })
                        .collect::<Result<_, _>>()
                        .map(|channels| {
                            (rebase_runtime_decl(plot, runtime_owner_rebases), channels)
                        })
                })
                .collect::<Result<_, GraphcalError>>()?;
            Ok((owner.clone(), DagPresentationFacts { plot_channels }))
        })
        .collect()
}

fn install_plot_projections_for_dag(
    tir: &mut TIR,
    parent: &crate::dag_id::DagId,
    visiting: &mut HashSet<crate::dag_id::DagId>,
    complete: &mut HashSet<crate::dag_id::DagId>,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    if complete.contains(parent) {
        return Ok(());
    }
    if !visiting.insert(parent.clone()) {
        return Err(GraphcalError::internal_error(
            format!("semantic plot projection cycle reached `{parent}`"),
            src,
            DiagnosticAnchor::WholeFile,
        ));
    }
    let projections = tir
        .dags
        .get(parent)
        .ok_or_else(|| {
            GraphcalError::internal_error(
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
                let target = ResolvedDeclName::from_def(
                    instance_owner.clone(),
                    projection.target.to_unowned_def_name(),
                );
                let exposed = ResolvedDeclName::from_def(
                    parent.clone(),
                    projection.exposed_name.leaf().clone(),
                );
                (instance_owner, target, exposed)
            })
        })
        .collect::<Vec<_>>();
    for (instance_owner, target, exposed) in projections {
        install_plot_projections_for_dag(tir, &instance_owner, visiting, complete, src)?;
        let channels = tir
            .dags
            .get(&instance_owner)
            .and_then(|instance| instance.plot_channel_presentations(&target))
            .cloned()
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic plot projection `{target}` has no checked presentation facts"
                    ),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        let dag = tir.dags.get_mut(parent).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("semantic plot projection parent `{parent}` is unavailable"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        dag.semantic
            .presentation
            .plot_channels
            .insert(target, channels.clone());
        dag.semantic
            .presentation
            .plot_channels
            .insert(exposed, channels);
    }
    visiting.remove(parent);
    complete.insert(parent.clone());
    Ok(())
}

fn dag_identity_snapshot(tir: &TIR) -> Vec<crate::dag_id::DagId> {
    // Imported bodies are immutable handles. Only local assembly bodies can
    // receive instance/projection facts.
    tir.dags.local_keys().cloned().collect()
}

/// Copy requested instance plots into their semantic parents from leaves upward.
pub fn install_semantic_plot_projection_facts(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    // Projection installation mutates the registry, so traversal owns a stable
    // identity snapshot rather than holding an iterator borrow across mutation.
    let dag_ids = dag_identity_snapshot(tir);
    let mut visiting = HashSet::new();
    let mut complete = HashSet::new();
    dag_ids.into_iter().try_for_each(|dag_id| {
        install_plot_projections_for_dag(tir, &dag_id, &mut visiting, &mut complete, src)
    })
}

/// Bootstrap semantic instances with specialized checked presentation facts.
///
/// The dimension checker subsequently recomputes concrete provenance from each
/// instance body while retaining these already-checked plot shapes.
pub fn install_semantic_presentation_facts(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (owner, facts) in specialize_instance_presentation_facts(tir, src)? {
        let instance = tir.dags.get_mut(&owner).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("semantic presentation instance `{owner}` is unavailable"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        instance.semantic.presentation = facts;
    }
    install_semantic_plot_projection_facts(tir, src)
}

fn install_semantic_projection_bindings(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let dag_ids = tir.dags.local_keys().cloned().collect::<Vec<_>>();
    for dag_id in dag_ids {
        let dag = tir.dags.get_mut(&dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("checked DAG `{dag_id}` disappeared during instantiation"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        for edge in dag.semantic_instances.clone() {
            for projection in edge.output_projections {
                let has_local_body = dag
                    .bound_decl_identity(&projection.exposed_name)
                    .and_then(|identity| dag.value_expr(identity))
                    .is_some();
                if !has_local_body {
                    dag.semantic.decl_bindings.insert(
                        projection.exposed_name,
                        ResolvedDeclName::from_def(
                            edge.instance.id().owner().clone(),
                            projection.target.to_unowned_def_name(),
                        ),
                    );
                }
            }
            for projection in edge.assertion_projections {
                dag.semantic.decl_bindings.insert(
                    projection.exposed_name,
                    ResolvedDeclName::from_def(
                        edge.instance.id().owner().clone(),
                        projection.target.to_unowned_def_name(),
                    ),
                );
            }
        }
    }
    Ok(())
}

fn instantiate_semantic_edge(
    tir: &mut TIR,
    edge: &HirInstanceRecord,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let owner = edge.instance.id().owner();
    let template = tir
        .dags
        .get(edge.instance.id().template())
        .ok_or_else(|| {
            GraphcalError::internal_error(
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
        .into_iter()
        .map(|name| {
            let source =
                ResolvedUnitName::from_def(edge.instance.id().template().clone(), name.clone());
            let mut info = tir.unit_info(&source).cloned().ok_or_else(|| {
                GraphcalError::internal_error(
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
            Ok((ResolvedUnitName::from_def(owner.clone(), name), info))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let instance = clone_checked_instance(&template, edge, tir, src)?;
    for (unit, info) in runtime_unit_infos {
        tir.insert_runtime_unit(unit, info).map_err(|error| {
            GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
        })?;
    }
    tir.insert_materialized_dag(instance).map_err(|error| {
        GraphcalError::internal_error(error.to_string(), src, DiagnosticAnchor::WholeFile)
    })?;
    Ok(())
}

/// Materialize checked semantic include edges as concrete TIR DAG instances.
///
/// Template bodies are reused after the Option A closure check; only checked
/// signatures, concrete owners, and value-binding environments are specialized.
pub fn instantiate_semantic_edges(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    loop {
        let edges = tir
            .dags
            .iter()
            .flat_map(|(_, dag)| dag.semantic_instances().iter().cloned())
            .filter(|edge| tir.dags.get(edge.instance.id().owner()).is_none())
            .collect::<Vec<_>>();
        if edges.is_empty() {
            return install_semantic_projection_bindings(tir, src);
        }
        for edge in edges {
            instantiate_semantic_edge(tir, &edge, src)?;
        }
    }
}

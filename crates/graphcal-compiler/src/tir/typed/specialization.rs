//! Checked-DAG specialization for semantic include instances.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use miette::NamedSource;

use super::{
    DagTIR, ResolvedDimArg, ResolvedDimTerm, ResolvedGenericArg, ResolvedIndex, ResolvedTypeExpr,
    TIR,
};
use crate::dag_id::{DescendantRebase, InstanceId};
use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::dimension::{BaseDimId, Dimension};
use crate::ir::instance::{
    HirInstanceRecord, InstanceIndexBindingTarget, StaticSpecializationId, StaticSubstitution,
};
use crate::nat::NatPolyForm;
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::error::GraphcalError;
use crate::syntax::ast::EncodingChannel;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::dimension::{ResolvedDimName, ResolvedUnitName};
use crate::syntax::index_name::ResolvedIndexName;
use crate::syntax::type_name::ResolvedStructTypeName;
use crate::tir::presentation::{
    IndexedPresentation, LeafPresentation, PlotChannelPresentation, PresentationCallKey,
    PresentationIndexSubstitution, PresentationLocalBinder, PresentationMatchArm,
    PresentationMatchPattern, PresentationProvenance, PresentationSelection, UnitPresentation,
};

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
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Dimension, GraphcalError> {
    dimension.iter().try_fold(
        Dimension::dimensionless(),
        |acc, (base, exponent)| {
            let factor = match base {
                BaseDimId::UserDefined(name) => dimension_substitution(substitution, name).map_or_else(
                    || Ok(Dimension::base(base.clone())),
                    |target| {
                        tir.dimension(target).cloned().ok_or_else(|| {
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
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedDimArg, GraphcalError> {
    match dimension {
        ResolvedDimArg::Concrete(dimension) => {
            specialize_dimension(dimension, substitution, tir, src).map(ResolvedDimArg::Concrete)
        }
        ResolvedDimArg::Expr { terms, span } => terms
            .iter()
            .map(|term| match term {
                ResolvedDimTerm::Concrete { dim, power, op } => {
                    specialize_dimension(dim, substitution, tir, src).map(|dim| {
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

fn specialize_type(
    resolved: &ResolvedTypeExpr,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    let recurse = |resolved: &ResolvedTypeExpr| specialize_type(resolved, substitution, tir, src);
    match resolved {
        ResolvedTypeExpr::Quantity(dimension) => {
            specialize_dimension(dimension, substitution, tir, src).map(ResolvedTypeExpr::Quantity)
        }
        ResolvedTypeExpr::Complex { dimension, span } => {
            specialize_dim_arg(dimension, substitution, tir, src).map(|dimension| {
                ResolvedTypeExpr::Complex {
                    dimension,
                    span: *span,
                }
            })
        }
        ResolvedTypeExpr::IndexArg(index) => Ok(ResolvedTypeExpr::IndexArg(specialize_index(
            index,
            substitution,
        ))),
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
                        specialize_dim_arg(dimension, substitution, tir, src)
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
                        specialize_dimension(dim, substitution, tir, src).map(|dim| {
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

fn specialize_index_ref(index: &IndexTypeRef, substitution: &StaticSubstitution) -> IndexTypeRef {
    let Some(source) = index.declared_resolved() else {
        return index.clone();
    };
    match index_substitution(substitution, source) {
        Some(InstanceIndexBindingTarget::Declared(target)) => {
            IndexTypeRef::with_display_leaf(index.display_name(), target.clone())
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

fn rebase_runtime_dag(
    dag: &crate::dag_id::DagId,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> crate::dag_id::DagId {
    runtime_owner_rebases
        .get(dag)
        .cloned()
        .unwrap_or_else(|| dag.clone())
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

fn specialize_unit_expr(
    unit: &crate::hir::ResolvedUnitExpr,
    tir: &TIR,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> crate::hir::ResolvedUnitExpr {
    crate::hir::ResolvedUnitExpr {
        terms: unit
            .terms
            .iter()
            .map(|term| {
                let source = term.name.value.resolved();
                let resolved = tir.unit_info(source).map_or_else(
                    || source.clone(),
                    |info| {
                        if info.scale.is_dynamic() {
                            ResolvedUnitName::from_def(
                                rebase_runtime_dag(source.owner(), runtime_owner_rebases),
                                source.to_unowned_def_name(),
                            )
                        } else {
                            source.clone()
                        }
                    },
                );
                crate::hir::ResolvedUnitExprItem {
                    op: term.op,
                    name: crate::syntax::span::Spanned::new(
                        crate::hir::ResolvedUnitRef::new(
                            term.name.value.spelling().clone(),
                            resolved,
                        ),
                        term.name.span,
                    ),
                    power: term.power,
                }
            })
            .collect(),
        span: unit.span,
    }
}

fn specialize_selection(
    selection: &PresentationSelection,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> Result<PresentationSelection, GraphcalError> {
    let recurse = |presentation: &PresentationProvenance| {
        crate::stack::with_stack_growth(|| {
            specialize_presentation(presentation, substitution, tir, src, runtime_owner_rebases)
        })
    };
    Ok(match selection {
        PresentationSelection::If {
            defining_dag,
            owner,
            condition,
            then_presentation,
            else_presentation,
        } => PresentationSelection::If {
            defining_dag: rebase_runtime_dag(defining_dag, runtime_owner_rebases),
            owner: rebase_runtime_decl(owner, runtime_owner_rebases),
            condition: condition.clone(),
            then_presentation: Box::new(recurse(then_presentation)?),
            else_presentation: Box::new(recurse(else_presentation)?),
        },
        PresentationSelection::Match {
            defining_dag,
            owner,
            scrutinee,
            arms,
        } => PresentationSelection::Match {
            defining_dag: rebase_runtime_dag(defining_dag, runtime_owner_rebases),
            owner: rebase_runtime_decl(owner, runtime_owner_rebases),
            scrutinee: scrutinee.clone(),
            arms: arms
                .iter()
                .map(|arm| {
                    let pattern = match &arm.pattern {
                        PresentationMatchPattern::IndexLabel(variant) => {
                            PresentationMatchPattern::IndexLabel(variant.clone())
                        }
                        PresentationMatchPattern::Constructor {
                            owning_type,
                            constructor,
                        } => PresentationMatchPattern::Constructor {
                            owning_type: specialize_struct_ref(owning_type, substitution),
                            constructor: constructor.clone(),
                        },
                    };
                    recurse(&arm.presentation).map(|presentation| PresentationMatchArm {
                        pattern,
                        presentation,
                    })
                })
                .collect::<Result<_, _>>()?,
        },
    })
}

fn specialize_presentation(
    presentation: &PresentationProvenance,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> Result<PresentationProvenance, GraphcalError> {
    let recurse = |presentation: &PresentationProvenance| {
        crate::stack::with_stack_growth(|| {
            specialize_presentation(presentation, substitution, tir, src, runtime_owner_rebases)
        })
    };
    match presentation {
        PresentationProvenance::None => Ok(PresentationProvenance::None),
        PresentationProvenance::Leaf(LeafPresentation::Timezone(timezone)) => Ok(
            PresentationProvenance::Leaf(LeafPresentation::Timezone(timezone.clone())),
        ),
        PresentationProvenance::Leaf(LeafPresentation::Unit(unit)) => Ok(
            PresentationProvenance::Leaf(LeafPresentation::Unit(UnitPresentation::new(
                rebase_runtime_dag(unit.defining_dag(), runtime_owner_rebases),
                rebase_runtime_decl(unit.owner(), runtime_owner_rebases),
                specialize_dimension(unit.dimension(), substitution, tir, src)?,
                specialize_unit_expr(unit.unit(), tir, runtime_owner_rebases),
                unit.requires_runtime_values(),
            ))),
        ),
        PresentationProvenance::Struct {
            owning_type,
            fields,
        } => Ok(PresentationProvenance::Struct {
            owning_type: specialize_struct_ref(owning_type, substitution),
            fields: fields
                .iter()
                .map(|(field, presentation)| recurse(presentation).map(|p| (field.clone(), p)))
                .collect::<Result<_, _>>()?,
        }),
        PresentationProvenance::Indexed { index, elements } => {
            let elements = match elements {
                IndexedPresentation::Uniform { binder, element } => IndexedPresentation::Uniform {
                    binder: binder.as_ref().map(|binder| {
                        PresentationLocalBinder::new(
                            rebase_runtime_decl(binder.owner(), runtime_owner_rebases),
                            binder.local(),
                        )
                    }),
                    element: Box::new(recurse(element)?),
                },
                IndexedPresentation::Entries(entries) => IndexedPresentation::Entries(
                    entries
                        .iter()
                        .map(|(key, presentation)| recurse(presentation).map(|p| (key.clone(), p)))
                        .collect::<Result<_, _>>()?,
                ),
            };
            Ok(PresentationProvenance::Indexed {
                index: specialize_index_ref(index, substitution),
                elements,
            })
        }
        PresentationProvenance::DagCall { key, output } => Ok(PresentationProvenance::DagCall {
            key: PresentationCallKey::new(
                rebase_runtime_decl(key.owner(), runtime_owner_rebases),
                key.span(),
            ),
            output: Box::new(recurse(output)?),
        }),
        PresentationProvenance::IndexProjection {
            defining_dag,
            owner,
            substitutions,
            output,
        } => Ok(PresentationProvenance::IndexProjection {
            defining_dag: rebase_runtime_dag(defining_dag, runtime_owner_rebases),
            owner: rebase_runtime_decl(owner, runtime_owner_rebases),
            substitutions: substitutions
                .iter()
                .map(|item| {
                    PresentationIndexSubstitution::new(
                        specialize_index_ref(item.index(), substitution),
                        PresentationLocalBinder::new(
                            rebase_runtime_decl(item.binder().owner(), runtime_owner_rebases),
                            item.binder().local(),
                        ),
                        item.argument().clone(),
                    )
                })
                .collect(),
            output: Box::new(recurse(output)?),
        }),
        PresentationProvenance::Select(selection) => Ok(PresentationProvenance::Select(Box::new(
            specialize_selection(selection, substitution, tir, src, runtime_owner_rebases)?,
        ))),
    }
}

fn specialize_plot_channel(
    channel: &PlotChannelPresentation,
    substitution: &StaticSubstitution,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
    runtime_owner_rebases: &HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> Result<PlotChannelPresentation, GraphcalError> {
    let leaf = match channel.leaf() {
        crate::plot_shape::PlotLeafKind::Quantity(dimension) => {
            crate::plot_shape::PlotLeafKind::Quantity(specialize_dimension(
                dimension,
                substitution,
                tir,
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
            .shape()
            .axes()
            .iter()
            .map(|index| specialize_index_ref(index, substitution))
            .collect(),
        leaf,
    );
    Ok(PlotChannelPresentation::new(
        shape,
        specialize_presentation(
            channel.provenance(),
            substitution,
            tir,
            src,
            runtime_owner_rebases,
        )?,
    ))
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

fn extend_binding_constructor_refs(
    instance: &mut DagTIR,
    binding: &crate::hir::Expr,
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let mut result = Ok(());
    crate::hir::visit_expr(binding, &mut |expr| {
        if result.is_err() {
            return;
        }
        let constructors = match &expr.kind {
            crate::hir::ExprKind::ConstructorCall { callee, .. } => vec![callee.value.clone()],
            crate::hir::ExprKind::ConstRef(target) => match &target.value {
                crate::hir::ConstRef::Constructor(constructor) => vec![constructor.clone()],
                _ => Vec::new(),
            },
            crate::hir::ExprKind::Match { arms, .. } => arms
                .iter()
                .filter_map(|arm| match &arm.pattern {
                    crate::hir::expr::MatchPattern::Constructor { constructor, .. } => {
                        Some(constructor.value.clone())
                    }
                    crate::hir::expr::MatchPattern::IndexLabel { .. } => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for constructor in constructors {
            let Some(definition) = tir.project_types.lookup_constructor(&constructor) else {
                result = Err(GraphcalError::internal_error(
                    format!("semantic binding constructor `{constructor}` is unavailable"),
                    src,
                    DiagnosticAnchor::WholeFile,
                ));
                return;
            };
            instance.semantic.constructor_refs.constructor_defs.insert(
                constructor,
                super::ResolvedConstructorTarget {
                    owning_type: definition.owning_type.clone(),
                    type_def: Arc::clone(&definition.type_def),
                    variant: definition.variant.clone(),
                },
            );
        }
    });
    result
}

fn resolve_instance_override_target(
    target: &crate::ir::override_reconciliation::PendingOverrideTarget,
    substitution: &StaticSubstitution,
    src: &NamedSource<Arc<String>>,
    include_span: crate::syntax::span::Span,
) -> Result<super::ResolvedOverrideTarget, GraphcalError> {
    use crate::ir::override_reconciliation::PendingOverrideTarget;

    match target {
        PendingOverrideTarget::Index {
            overridden,
            source_owner,
            ..
        } => {
            let source = ResolvedIndexName::from_def(source_owner.clone(), overridden.clone());
            let replacement = substitution.indexes.get(&source).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic override reconciliation for index `{source}` has no canonical Static substitution"
                    ),
                    src,
                    DiagnosticAnchor::Source(include_span),
                )
            })?;
            Ok(super::ResolvedOverrideTarget::Index {
                overridden: overridden.clone(),
                source,
                replacement: match replacement {
                    InstanceIndexBindingTarget::Declared(name) => {
                        IndexTypeRef::from_resolved(name.clone())
                    }
                    InstanceIndexBindingTarget::Finite(index) => {
                        IndexTypeRef::from_finite_index(*index)
                    }
                },
            })
        }
        PendingOverrideTarget::Type {
            overridden,
            source_owner,
            ..
        } => {
            let source = ResolvedStructTypeName::from_def(source_owner.clone(), overridden.clone());
            let replacement = substitution.types.get(&source).cloned().ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "semantic override reconciliation for type `{source}` has no canonical Static substitution"
                    ),
                    src,
                    DiagnosticAnchor::Source(include_span),
                )
            })?;
            Ok(super::ResolvedOverrideTarget::Type {
                overridden: overridden.clone(),
                source,
                replacement,
            })
        }
    }
}

fn install_override_reconciliations(
    instance: &mut DagTIR,
    edge: &HirInstanceRecord,
) -> Result<(), GraphcalError> {
    let owner = edge.instance.id.owner();
    instance.semantic.override_reconciliations = edge
        .override_reconciliations
        .iter()
        .map(|(template_port, pending)| {
            let instance_port =
                ResolvedDeclName::from_def(owner.clone(), template_port.to_unowned_def_name());
            let reconciliations = pending
                .iter()
                .map(|pending| {
                    let targets = pending
                        .targets
                        .iter()
                        .map(|target| {
                            resolve_instance_override_target(
                                target,
                                &edge.instance.specialization.substitution,
                                &pending.src,
                                pending.include_span,
                            )
                        })
                        .collect::<Result<_, GraphcalError>>()?;
                    Ok(super::OverrideReconciliation {
                        source_decl: pending.source_decl.clone(),
                        orphan_decl: pending.orphan_decl.clone(),
                        targets,
                        src: pending.src.clone(),
                        include_span: pending.include_span,
                    })
                })
                .collect::<Result<_, GraphcalError>>()?;
            Ok((instance_port, reconciliations))
        })
        .collect::<Result<_, GraphcalError>>()?;
    Ok(())
}

fn specialize_expected_fail(
    expected: &mut crate::ir::resolve::ExpectedFail,
    substitution: &StaticSubstitution,
) {
    if let crate::ir::resolve::ExpectedFail::Variants(keys) = expected {
        for part in keys.iter_mut().flatten() {
            if let crate::ir::resolve::ExpectedFailKeyPart::Named { index, .. } = part
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

fn rebase_nested_instance(
    mut nested: HirInstanceRecord,
    template_owner: &crate::dag_id::DagId,
    owner: &crate::dag_id::DagId,
    substitution: &StaticSubstitution,
    runtime_owner_rebases: &mut HashMap<crate::dag_id::DagId, crate::dag_id::DagId>,
) -> HirInstanceRecord {
    let template_nested_owner = nested.instance.id.owner().clone();
    let nested_owner = match template_nested_owner.rebase_descendant(template_owner, owner) {
        DescendantRebase::Rebased(rebased) => rebased,
        DescendantRebase::OutsideSubtree => owner.instance_child(template_nested_owner.name()),
    };
    runtime_owner_rebases.insert(template_nested_owner, nested_owner.clone());
    compose_index_targets(
        nested
            .instance
            .specialization
            .substitution
            .indexes
            .values_mut()
            .chain(nested.instance.bindings.indexes.values_mut()),
        substitution,
    );
    compose_type_targets(
        nested
            .instance
            .specialization
            .substitution
            .types
            .values_mut()
            .chain(nested.instance.bindings.types.values_mut()),
        substitution,
    );
    compose_dimension_targets(
        nested
            .instance
            .specialization
            .substitution
            .dimensions
            .values_mut()
            .chain(nested.instance.bindings.dimensions.values_mut()),
        substitution,
    );
    nested
        .assertion_projections
        .iter_mut()
        .filter_map(|projection| projection.expected_fail.as_mut())
        .for_each(|expected| specialize_expected_fail(expected, substitution));
    nested.instance.id = InstanceId::new(
        nested_owner,
        nested.instance.specialization.template.clone(),
    );
    nested.instance.parent_owner = owner.clone();
    nested.owner_rebases.extend(runtime_owner_rebases.clone());
    nested
}

fn initialize_instance_identity(
    instance: &mut DagTIR,
    template: &DagTIR,
    edge: &HirInstanceRecord,
) {
    let owner = edge.instance.id.owner();
    let specialization = &edge.instance.specialization;
    instance.dag_id = owner.clone();
    instance.semantic_specialization = Some(specialization.clone());
    instance.static_ports.clear();
    instance
        .runtime_owner_rebases
        .clone_from(&edge.owner_rebases);
    instance
        .runtime_owner_rebases
        .insert(specialization.template.clone(), owner.clone());
    for declaration_owner in template
        .consts
        .iter()
        .map(|entry| &entry.declaration_owner)
        .chain(template.params.iter().map(|entry| &entry.declaration_owner))
        .chain(template.nodes.iter().map(|entry| &entry.declaration_owner))
        .chain(
            template
                .asserts
                .iter()
                .map(|entry| &entry.declaration_owner),
        )
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
                template.dag_id(),
                owner,
                &specialization.substitution,
                &mut instance.runtime_owner_rebases,
            )
        })
        .collect();
    instance.instances = instance
        .semantic_instances
        .iter()
        .map(|nested| nested.instance.clone())
        .collect();
}

fn specialize_instance_declarations(instance: &mut DagTIR, edge: &HirInstanceRecord) {
    let owner = edge.instance.id.owner();
    let specialization = &edge.instance.specialization;
    instance
        .consts
        .iter_mut()
        .for_each(|entry| entry.declaration_owner = owner.clone());
    for entry in &mut instance.params {
        let template_port = ResolvedDeclName::from_def(
            specialization.template.clone(),
            entry.name.member().clone(),
        );
        entry.declaration_owner = owner.clone();
        if let Some(binding) = edge.value_bindings.get(&template_port) {
            entry.default = Some(crate::ir::lower::ParamDefault {
                expr: binding.clone(),
                src: crate::ir::lower::BodySource::own(),
            });
        }
    }
    instance
        .nodes
        .iter_mut()
        .for_each(|entry| entry.declaration_owner = owner.clone());
    instance
        .asserts
        .iter_mut()
        .for_each(|entry| entry.declaration_owner = owner.clone());
    instance.expected_fail.values_mut().for_each(|expected| {
        specialize_expected_fail(&mut expected.expected, &specialization.substitution);
    });
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
                tir,
                src,
            )?;
            entry.base_unit_dimension = specialize_dimension(
                &entry.base_unit_dimension,
                &specialization.substitution,
                tir,
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
    let owner = edge.instance.id.owner();
    let specialization = &edge.instance.specialization;
    instance.resolved_decl_types = instance
        .resolved_decl_types
        .iter()
        .map(|(name, resolved)| {
            specialize_type(resolved, &specialization.substitution, tir, src)
                .map(|resolved| (name.clone(), resolved))
        })
        .collect::<Result<_, _>>()?;
    instance.semantic.decl_bindings = instance
        .semantic
        .decl_bindings
        .iter()
        .map(|(name, target)| (name.clone(), instance_decl(target, specialization, owner)))
        .collect();
    specialize_dependencies(&mut instance.semantic.dependencies, specialization, owner);
    for (template_port, binding) in &edge.value_bindings {
        extend_binding_constructor_refs(instance, binding, tir, src)?;
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
    instance.semantic.presentation.declarations.clear();
    instance.semantic.presentation.plot_channels.clear();
    // Concrete Static index bindings can change cardinality. The runtime
    // revalidates semantic-instance comprehensions from their specialized axes
    // rather than accepting a shape fact checked against an optional default.
    instance.semantic.materialized_shapes.clear();
    instance.semantic.domain_bounds = instance
        .semantic
        .domain_bounds
        .iter()
        .map(|(target, bounds)| (local_instance_decl(target, owner), bounds.clone()))
        .collect();
    install_override_reconciliations(instance, edge)?;
    instance.declaration_index = super::DagDeclarationIndex::default();
    instance.index_declaration_records().map_err(|error| {
        GraphcalError::internal_error(
            format!("failed to index semantic instance `{owner}`: {error:?}"),
            src,
            DiagnosticAnchor::WholeFile,
        )
    })
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
    specialize_instance_declarations(&mut instance, edge);
    specialize_dynamic_unit_scales(
        &mut instance,
        &edge.instance.specialization,
        edge.instance.id.owner(),
        tir,
        src,
    )?;
    specialize_instance_semantics(&mut instance, edge, tir, src)?;
    Ok(instance)
}

type PlotPresentationFacts =
    HashMap<ResolvedDeclName, HashMap<EncodingChannel, PlotChannelPresentation>>;

type ProjectedPlotPresentation = (
    crate::dag_id::DagId,
    ResolvedDeclName,
    HashMap<EncodingChannel, PlotChannelPresentation>,
);

fn specialize_instance_presentation_facts(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<(crate::dag_id::DagId, PlotPresentationFacts)>, GraphcalError> {
    tir.dags
        .iter()
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
            let facts = template
                .semantic
                .presentation
                .plot_channels
                .iter()
                .map(|(plot, channels)| {
                    channels
                        .iter()
                        .map(|(encoding, channel)| {
                            specialize_plot_channel(
                                channel,
                                &specialization.substitution,
                                tir,
                                src,
                                runtime_owner_rebases,
                            )
                            .map(|channel| (*encoding, channel))
                        })
                        .collect::<Result<_, _>>()
                        .map(|channels| (local_instance_decl(plot, owner), channels))
                })
                .collect::<Result<_, GraphcalError>>()?;
            Ok((owner.clone(), facts))
        })
        .collect()
}

fn collect_projected_plot_facts(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<Vec<ProjectedPlotPresentation>, GraphcalError> {
    tir.dags
        .iter()
        .flat_map(|(parent, dag)| {
            dag.semantic_instances().iter().flat_map(move |edge| {
                edge.plot_projections.iter().map(move |projection| {
                    (
                        parent.clone(),
                        edge.instance.id.owner().clone(),
                        ResolvedDeclName::from_def(
                            edge.instance.id.owner().clone(),
                            projection.target.to_unowned_def_name(),
                        ),
                    )
                })
            })
        })
        .map(|(parent, instance_owner, target)| {
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
            Ok((parent, target, channels))
        })
        .collect()
}

/// Specialize checked plot presentation facts and project requested plots.
///
/// Same-file inline templates receive their checked presentation facts only
/// after semantic instances have been materialized. Installing these facts as
/// a separate post-check step keeps instance construction independent of DAG
/// declaration order while preserving canonical plot and Static identities.
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
        instance.semantic.presentation.plot_channels = facts;
    }
    for (parent, target, channels) in collect_projected_plot_facts(tir, src)? {
        let dag = tir.dags.get_mut(&parent).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("semantic plot projection parent `{parent}` is unavailable"),
                src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        dag.semantic
            .presentation
            .plot_channels
            .insert(target, channels);
    }
    Ok(())
}

fn install_semantic_projection_bindings(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    let dag_ids = tir.dags.keys().cloned().collect::<Vec<_>>();
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
                            edge.instance.id.owner().clone(),
                            projection.target.to_unowned_def_name(),
                        ),
                    );
                }
            }
            for projection in edge.assertion_projections {
                dag.semantic.decl_bindings.insert(
                    projection.exposed_name,
                    ResolvedDeclName::from_def(
                        edge.instance.id.owner().clone(),
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
    let owner = edge.instance.id.owner();
    let template = tir
        .dags
        .get(edge.instance.id.template())
        .ok_or_else(|| {
            GraphcalError::internal_error(
                format!(
                    "checked template `{}` is unavailable for semantic instance `{owner}`",
                    edge.instance.id.template()
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
                .map(crate::syntax::names::ResolvedName::to_unowned_def_name),
        )
        .collect::<BTreeSet<_>>();
    let runtime_unit_infos = runtime_unit_names
        .into_iter()
        .map(|name| {
            let source =
                ResolvedUnitName::from_def(edge.instance.id.template().clone(), name.clone());
            let mut info = tir.unit_info(&source).cloned().ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("template runtime unit `{source}` has no checked definition"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            info.dimension = specialize_dimension(
                &info.dimension,
                &edge.instance.specialization.substitution,
                tir,
                src,
            )?;
            Ok((ResolvedUnitName::from_def(owner.clone(), name), info))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let instance = clone_checked_instance(&template, edge, tir, src)?;
    for (unit, info) in runtime_unit_infos {
        tir.project_types
            .insert_unit_definition(unit, &info)
            .map_err(|error| {
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
            .filter(|edge| tir.dags.get(edge.instance.id.owner()).is_none())
            .collect::<Vec<_>>();
        if edges.is_empty() {
            return install_semantic_projection_bindings(tir, src);
        }
        for edge in edges {
            instantiate_semantic_edge(tir, &edge, src)?;
        }
    }
}

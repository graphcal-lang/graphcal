//! Derive checked, structured presentation facts from typed HIR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use indexmap::IndexMap;
use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::hir::{self, ExprKind};
use crate::registry::declared_type::{IndexTypeRef, StructTypeRef};
use crate::registry::error::GraphcalError;
use crate::syntax::decl_name::ResolvedDeclName;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::span::Span;
use crate::tir::presentation::{
    DagPresentationFacts, IndexedPresentation, LeafPresentation, PlotChannelPresentation,
    PresentationCallKey, PresentationMatchArm, PresentationMatchPattern, PresentationProvenance,
    PresentationSelection, UnitPresentation,
};

use super::infer;

/// Derive presentation for an already type-checked standalone HIR expression.
pub fn checked_expression_presentation(
    tir: &crate::tir::typed::TIR,
    owner: &ResolvedDeclName,
    expr: &hir::Expr,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<PresentationProvenance, GraphcalError> {
    let mut resolver = PresentationResolver {
        tir,
        fallback_src: src,
        cancellation,
        memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    let dag_id = tir
        .dag_containing_declaration(owner)
        .map_or_else(|| owner.owner(), crate::tir::typed::DagTIR::dag_id);
    resolver.expression(owner, dag_id, expr, src)
}

/// Collect presentation facts for every DAG checked in this physical file.
pub(super) fn collect_presentation_facts(
    tir: &crate::tir::typed::TIR,
    plot_shapes: &HashMap<crate::dag_id::DagId, super::plot::CheckedPlotChannelShapes>,
    src: &NamedSource<Arc<String>>,
    cancellation: &crate::cancellation::CancellationToken,
) -> Result<HashMap<crate::dag_id::DagId, DagPresentationFacts>, GraphcalError> {
    let mut resolver = PresentationResolver {
        tir,
        fallback_src: src,
        cancellation,
        memo: HashMap::new(),
        visiting: HashSet::new(),
    };
    tir.local_dags()
        .map(|(dag_id, _)| dag_id.clone())
        .map(|dag_id| {
            cancellation.checkpoint()?;
            let shapes = plot_shapes.get(&dag_id).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("checked plot shapes are missing for presentation DAG `{dag_id}`"),
                    src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            resolver
                .facts_for_dag(&dag_id, shapes)
                .map(|facts| (dag_id, facts))
        })
        .collect()
}

struct PresentationResolver<'a> {
    tir: &'a crate::tir::typed::TIR,
    fallback_src: &'a NamedSource<Arc<String>>,
    cancellation: &'a crate::cancellation::CancellationToken,
    memo: HashMap<ResolvedDeclName, PresentationProvenance>,
    visiting: HashSet<ResolvedDeclName>,
}

impl PresentationResolver<'_> {
    fn facts_for_dag(
        &mut self,
        dag_id: &crate::dag_id::DagId,
        checked_plot_shapes: &super::plot::CheckedPlotChannelShapes,
    ) -> Result<DagPresentationFacts, GraphcalError> {
        let dag = self.dag(dag_id, DiagnosticAnchor::WholeFile)?;
        // Materialize keys to release the immutable DAG borrow before the
        // recursive resolver mutates its memo/visiting sets.
        let declaration_keys = presentation_declaration_keys(dag);
        let plot_entries = dag
            .plots()
            .iter()
            .map(|entry| {
                (
                    dag.resolved_decl_key_for_local(&entry.name),
                    entry.body.clone(),
                    entry.body_src.resolve(self.fallback_src).clone(),
                )
            })
            .collect::<Vec<_>>();

        let declarations = declaration_keys
            .into_iter()
            .map(|key| self.declaration(&key).map(|fact| (key, fact)))
            .collect::<Result<HashMap<_, _>, _>>()?;

        let mut plot_channels = HashMap::new();
        for (plot, body, body_src) in plot_entries {
            let shapes = checked_plot_shapes.get(&plot).ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("checked channel shapes are missing for plot `{plot}`"),
                    &body_src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
            let channels = body
                .encodings
                .into_iter()
                .map(|(channel, expr)| {
                    let shape = shapes.get(&channel).cloned().ok_or_else(|| {
                        GraphcalError::InternalError {
                            message: format!(
                                "checked shape is missing for plot channel `{channel}`"
                            ),
                            src: body_src.clone(),
                            span: expr.span.into(),
                        }
                    })?;
                    let provenance = self.expression(&plot, dag_id, &expr, &body_src)?;
                    Ok((channel, PlotChannelPresentation::new(shape, provenance)))
                })
                .collect::<Result<HashMap<_, _>, GraphcalError>>()?;
            plot_channels.insert(plot, channels);
        }

        Ok(DagPresentationFacts {
            declarations,
            plot_channels,
        })
    }

    fn dag(
        &self,
        dag_id: &crate::dag_id::DagId,
        anchor: DiagnosticAnchor,
    ) -> Result<&crate::tir::typed::DagTIR, GraphcalError> {
        self.tir.dag_registry().get(dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("presentation owner `{dag_id}` has no checked DAG"),
                self.fallback_src,
                anchor,
            )
        })
    }

    fn declaration_dag(
        &self,
        declaration: &ResolvedDeclName,
    ) -> Result<&crate::tir::typed::DagTIR, GraphcalError> {
        self.tir
            .dag_containing_declaration(declaration)
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!(
                        "presentation declaration `{declaration}` has no containing checked DAG"
                    ),
                    self.fallback_src,
                    DiagnosticAnchor::WholeFile,
                )
            })
    }

    fn declaration(
        &mut self,
        key: &ResolvedDeclName,
    ) -> Result<PresentationProvenance, GraphcalError> {
        self.cancellation.checkpoint()?;
        if let Some(presentation) = self.memo.get(key) {
            return Ok(presentation.clone());
        }
        let (checked, body, dag_id) = {
            let dag = self.declaration_dag(key)?;
            (
                dag.declaration_presentation(key).cloned(),
                declaration_body(dag, key, self.fallback_src),
                dag.dag_id().clone(),
            )
        };
        if let Some(presentation) = checked {
            return Ok(presentation);
        }
        if !self.visiting.insert(key.clone()) {
            return Err(GraphcalError::internal_error(
                format!("presentation dependency cycle reached `{key}` after value-cycle checking"),
                self.fallback_src,
                DiagnosticAnchor::WholeFile,
            ));
        }
        let presentation = match body {
            Some((expr, body_src)) => self.expression(key, &dag_id, &expr, &body_src)?,
            None => PresentationProvenance::None,
        };
        self.visiting.remove(key);
        self.memo.insert(key.clone(), presentation.clone());
        Ok(presentation)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive HIR presentation transfer keeps one auditable arm per expression kind"
    )]
    fn expression(
        &mut self,
        owner: &ResolvedDeclName,
        dag_id: &crate::dag_id::DagId,
        expr: &hir::Expr,
        src: &NamedSource<Arc<String>>,
    ) -> Result<PresentationProvenance, GraphcalError> {
        self.cancellation.checkpoint()?;
        match &expr.kind {
            ExprKind::QuantityLiteral { unit, .. } | ExprKind::Convert { target: unit, .. } => {
                let dimension = infer::resolve_unit_dimension_or_diagnose(unit, self.tir, src)?;
                let requires_runtime_values = unit.terms.iter().any(|term| {
                    self.tir
                        .unit_info(term.name.value.resolved())
                        .is_some_and(|info| info.scale.is_dynamic())
                });
                Ok(PresentationProvenance::Leaf(LeafPresentation::Unit(
                    UnitPresentation::new(
                        dag_id.clone(),
                        owner.clone(),
                        dimension,
                        unit.clone(),
                        requires_runtime_values,
                    ),
                )))
            }
            ExprKind::DisplayTimezone { timezone, .. } => Ok(PresentationProvenance::Leaf(
                LeafPresentation::Timezone(timezone.clone()),
            )),
            ExprKind::GraphRef(target) => self.declaration(&target.value),
            ExprKind::ConstRef(target) => match &target.value {
                hir::ConstRef::Decl(target) => self.declaration(target),
                hir::ConstRef::Builtin(_)
                | hir::ConstRef::Constructor(_)
                | hir::ConstRef::TimeScale(_)
                | hir::ConstRef::GenericNatParam(_) => Ok(PresentationProvenance::None),
            },
            ExprKind::DagCall { output, .. } => {
                let output = self.declaration(&output.value)?;
                if output.requires_runtime_values() {
                    Ok(PresentationProvenance::DagCall {
                        key: PresentationCallKey::new(owner.clone(), expr.span),
                        output: Box::new(output),
                    })
                } else {
                    Ok(output)
                }
            }
            ExprKind::ConstructorCall { callee, fields, .. } => {
                let owning_type = {
                    let dag = self.dag(dag_id, DiagnosticAnchor::Source(expr.span))?;
                    let target = dag
                        .semantic()
                        .constructor_refs
                        .constructor_defs
                        .get(&callee.value)
                        .ok_or_else(|| GraphcalError::InternalError {
                            message: format!(
                                "presentation metadata cannot resolve constructor `{}`",
                                callee.value
                            ),
                            src: src.clone(),
                            span: callee.span.into(),
                        })?;
                    StructTypeRef::from_resolved(target.owning_type.clone())
                };
                let fields = fields
                    .iter()
                    .map(|field| {
                        self.expression(owner, dag_id, &field.value, src)
                            .map(|presentation| (field.name.value.clone(), presentation))
                    })
                    .collect::<Result<HashMap<_, _>, _>>()?;
                Ok(PresentationProvenance::Struct {
                    owning_type,
                    fields,
                })
            }
            ExprKind::FieldAccess { expr: inner, field } => {
                let inner = self.expression(owner, dag_id, inner, src)?;
                Ok(project_field(inner, &field.value))
            }
            ExprKind::MapLiteral { entries } => self.map_literal(owner, dag_id, entries, src),
            ExprKind::ForComp { bindings, body } => {
                let body = self.expression(owner, dag_id, body, src)?;
                bindings.iter().rev().try_fold(body, |inner, binding| {
                    presentation_index(&binding.index, src).map(|index| {
                        PresentationProvenance::uniform_index_for_local(
                            index,
                            binding.local.id,
                            inner,
                        )
                    })
                })
            }
            ExprKind::IndexAccess { expr: inner, args } => {
                let inner = self.expression(owner, dag_id, inner, src)?;
                Ok(project_indexes(inner, args.as_slice()))
            }
            ExprKind::Scan { source, init, .. } => {
                let source = self.expression(owner, dag_id, source, src)?;
                let init = self.expression(owner, dag_id, init, src)?;
                Ok(
                    outer_index(&source).map_or(PresentationProvenance::None, |index| {
                        PresentationProvenance::uniform_index(index.clone(), init)
                    }),
                )
            }
            ExprKind::Unfold {
                recurrence, init, ..
            } => {
                let init = self.expression(owner, dag_id, init, src)?;
                Ok(PresentationProvenance::uniform_index(
                    IndexTypeRef::from_resolved(recurrence.axis.value.clone()),
                    init,
                ))
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let then_presentation = self.expression(owner, dag_id, then_branch, src)?;
                let else_presentation = self.expression(owner, dag_id, else_branch, src)?;
                if is_none(&then_presentation) && is_none(&else_presentation) {
                    Ok(PresentationProvenance::None)
                } else {
                    Ok(PresentationProvenance::Select(Box::new(
                        PresentationSelection::If {
                            defining_dag: dag_id.clone(),
                            owner: owner.clone(),
                            condition: condition.as_ref().clone(),
                            then_presentation: Box::new(then_presentation),
                            else_presentation: Box::new(else_presentation),
                        },
                    )))
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let arms = arms
                    .iter()
                    .map(|arm| {
                        let pattern = match &arm.pattern {
                            hir::expr::MatchPattern::IndexLabel { variant, .. } => {
                                PresentationMatchPattern::IndexLabel(variant.variant.clone())
                            }
                            hir::expr::MatchPattern::Constructor { constructor, .. } => {
                                let (owning_type, constructor) = {
                                    let dag = self.dag(dag_id, DiagnosticAnchor::Source(expr.span))?;
                                    let target = dag
                                        .semantic()
                                        .constructor_refs
                                        .constructor_defs
                                        .get(&constructor.value)
                                        .ok_or_else(|| GraphcalError::InternalError {
                                            message: format!(
                                                "presentation metadata cannot resolve constructor `{}`",
                                                constructor.value
                                            ),
                                            src: src.clone(),
                                            span: constructor.span.into(),
                                        })?;
                                    (
                                        StructTypeRef::from_resolved(target.owning_type.clone()),
                                        target.variant.name(),
                                    )
                                };
                                PresentationMatchPattern::Constructor {
                                    owning_type,
                                    constructor,
                                }
                            }
                        };
                        self.expression(owner, dag_id, &arm.body, src)
                            .map(|presentation| PresentationMatchArm {
                                pattern,
                                presentation,
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if arms.iter().all(|arm| is_none(&arm.presentation)) {
                    Ok(PresentationProvenance::None)
                } else {
                    Ok(PresentationProvenance::Select(Box::new(
                        PresentationSelection::Match {
                            defining_dag: dag_id.clone(),
                            owner: owner.clone(),
                            scrutinee: scrutinee.as_ref().clone(),
                            arms,
                        },
                    )))
                }
            }
            ExprKind::Error { .. }
            | ExprKind::Number(_)
            | ExprKind::Integer(_)
            | ExprKind::Bool(_)
            | ExprKind::StringLiteral(_)
            | ExprKind::OffsetDateTimeLiteral(_)
            | ExprKind::CivilDateTimeLiteral(_)
            | ExprKind::ZonedDateTimeLiteral(_)
            | ExprKind::IanaTimeZoneLiteral(_)
            | ExprKind::TypeSystemRef(_)
            | ExprKind::LocalRef(_)
            | ExprKind::BinOp { .. }
            | ExprKind::UnaryOp { .. }
            | ExprKind::FnCall { .. }
            | ExprKind::KeyForm { .. }
            | ExprKind::VariantLiteral(_) => Ok(PresentationProvenance::None),
        }
    }

    fn map_literal(
        &mut self,
        owner: &ResolvedDeclName,
        dag_id: &crate::dag_id::DagId,
        entries: &[hir::expr::MapEntry],
        src: &NamedSource<Arc<String>>,
    ) -> Result<PresentationProvenance, GraphcalError> {
        let mut result = None;
        for entry in entries {
            let path = entry
                .keys
                .iter()
                .map(map_key_part)
                .collect::<Result<Vec<_>, _>>()?;
            let leaf = self.expression(owner, dag_id, &entry.value, src)?;
            let nested = singleton_indexed_path(&path, leaf);
            merge_indexed_presentation(&mut result, nested, src, entry.value.span)?;
        }
        Ok(result.unwrap_or_default())
    }
}

fn presentation_declaration_keys(dag: &crate::tir::typed::DagTIR) -> Vec<ResolvedDeclName> {
    dag.consts()
        .iter()
        .map(|entry| dag.resolved_decl_key_for_local(&entry.name))
        .chain(
            dag.params()
                .iter()
                .map(|entry| dag.resolved_decl_key_for_local(&entry.name)),
        )
        .chain(
            dag.nodes()
                .iter()
                .map(|entry| dag.resolved_decl_key_for_local(&entry.name)),
        )
        .collect()
}

fn declaration_body(
    dag: &crate::tir::typed::DagTIR,
    key: &ResolvedDeclName,
    fallback_src: &NamedSource<Arc<String>>,
) -> Option<(hir::Expr, NamedSource<Arc<String>>)> {
    dag.consts()
        .iter()
        .find(|entry| dag.resolved_decl_key_for_local(&entry.name) == *key)
        .map(|entry| {
            (
                entry.expr.clone(),
                entry.body_src.resolve(fallback_src).clone(),
            )
        })
        .or_else(|| {
            dag.params()
                .iter()
                .find(|entry| dag.resolved_decl_key_for_local(&entry.name) == *key)
                .and_then(|entry| {
                    entry.default_expr.as_ref().map(|expr| {
                        (
                            expr.clone(),
                            entry
                                .default_src
                                .as_ref()
                                .map_or(fallback_src, |source| source.resolve(fallback_src))
                                .clone(),
                        )
                    })
                })
        })
        .or_else(|| {
            dag.nodes()
                .iter()
                .find(|entry| dag.resolved_decl_key_for_local(&entry.name) == *key)
                .map(|entry| {
                    (
                        entry.expr.clone(),
                        entry.body_src.resolve(fallback_src).clone(),
                    )
                })
        })
}

fn presentation_index(
    index: &hir::expr::ForBindingIndex,
    src: &NamedSource<Arc<String>>,
) -> Result<IndexTypeRef, GraphcalError> {
    match index {
        hir::expr::ForBindingIndex::Named(index) => {
            Ok(IndexTypeRef::from_resolved(index.value.clone()))
        }
        hir::expr::ForBindingIndex::Finite { cardinality, span } => {
            let form = infer::hir::hir_nat_to_linear_form(cardinality).map_err(|error| {
                GraphcalError::EvalError {
                    message: error.to_string(),
                    src: src.clone(),
                    span: (*span).into(),
                }
            })?;
            IndexTypeRef::from_finite_index_form(form).map_err(|error| GraphcalError::EvalError {
                message: error.to_string(),
                src: src.clone(),
                span: (*span).into(),
            })
        }
    }
}

fn map_key_part(
    key: &hir::expr::MapEntryKey,
) -> Result<(IndexTypeRef, IndexEntryKey), GraphcalError> {
    match key {
        hir::expr::MapEntryKey::IndexVariant(variant) => Ok((
            IndexTypeRef::from_resolved(variant.variant.index().clone()),
            IndexEntryKey::named(variant.variant.variant().clone()),
        )),
        hir::expr::MapEntryKey::FinitePosition { size, position } => {
            let finite =
                crate::registry::types::FiniteIndex::try_from_u64(*size).map_err(|error| {
                    GraphcalError::InternalError {
                        message: format!("checked map presentation has invalid Fin axis: {error}"),
                        src: NamedSource::new("<presentation>", Arc::new(String::new())),
                        span: position.span.into(),
                    }
                })?;
            Ok((
                IndexTypeRef::from_finite_index(finite),
                IndexEntryKey::position(position.value),
            ))
        }
    }
}

fn singleton_indexed_path(
    path: &[(IndexTypeRef, IndexEntryKey)],
    leaf: PresentationProvenance,
) -> PresentationProvenance {
    path.iter().rev().fold(leaf, |inner, (index, key)| {
        PresentationProvenance::Indexed {
            index: index.clone(),
            elements: IndexedPresentation::Entries(IndexMap::from([(key.clone(), inner)])),
        }
    })
}

fn merge_indexed_presentation(
    target: &mut Option<PresentationProvenance>,
    incoming: PresentationProvenance,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    let Some(existing) = target else {
        *target = Some(incoming);
        return Ok(());
    };
    match (existing, incoming) {
        (
            PresentationProvenance::Indexed {
                index: existing_index,
                elements: IndexedPresentation::Entries(existing_entries),
            },
            PresentationProvenance::Indexed {
                index: incoming_index,
                elements: IndexedPresentation::Entries(incoming_entries),
            },
        ) if existing_index.matches_ref(&incoming_index) => {
            for (key, incoming) in incoming_entries {
                match existing_entries.entry(key) {
                    indexmap::map::Entry::Vacant(entry) => {
                        entry.insert(incoming);
                    }
                    indexmap::map::Entry::Occupied(mut entry) => {
                        let mut nested = Some(entry.get().clone());
                        merge_indexed_presentation(&mut nested, incoming, src, span)?;
                        *entry.get_mut() = nested.unwrap_or_default();
                    }
                }
            }
            Ok(())
        }
        _ => Err(GraphcalError::InternalError {
            message: "checked map presentation has inconsistent index structure".to_string(),
            src: src.clone(),
            span: span.into(),
        }),
    }
}

fn project_field(
    provenance: PresentationProvenance,
    field: &crate::syntax::type_name::FieldName,
) -> PresentationProvenance {
    match provenance {
        PresentationProvenance::Struct { mut fields, .. } => {
            fields.remove(field).unwrap_or_default()
        }
        PresentationProvenance::DagCall { key, output } => PresentationProvenance::DagCall {
            key,
            output: Box::new(project_field(*output, field)),
        },
        PresentationProvenance::Select(selection) => {
            PresentationProvenance::Select(Box::new(match *selection {
                PresentationSelection::If {
                    defining_dag,
                    owner,
                    condition,
                    then_presentation,
                    else_presentation,
                } => PresentationSelection::If {
                    defining_dag,
                    owner,
                    condition,
                    then_presentation: Box::new(project_field(*then_presentation, field)),
                    else_presentation: Box::new(project_field(*else_presentation, field)),
                },
                PresentationSelection::Match {
                    defining_dag,
                    owner,
                    scrutinee,
                    arms,
                } => PresentationSelection::Match {
                    defining_dag,
                    owner,
                    scrutinee,
                    arms: arms
                        .into_iter()
                        .map(|arm| PresentationMatchArm {
                            pattern: arm.pattern,
                            presentation: project_field(arm.presentation, field),
                        })
                        .collect(),
                },
            }))
        }
        PresentationProvenance::None
        | PresentationProvenance::Leaf(_)
        | PresentationProvenance::Indexed { .. } => PresentationProvenance::None,
    }
}

fn project_indexes(
    provenance: PresentationProvenance,
    args: &[hir::expr::IndexArg],
) -> PresentationProvenance {
    let mut provenance = match provenance {
        PresentationProvenance::DagCall { key, output } => {
            return PresentationProvenance::DagCall {
                key,
                output: Box::new(project_indexes(*output, args)),
            };
        }
        other => other,
    };
    for arg in args {
        provenance = match provenance {
            PresentationProvenance::Indexed { elements, .. } => match elements {
                IndexedPresentation::Uniform { element, .. } => *element,
                IndexedPresentation::Entries(mut entries) => static_index_key(arg)
                    .and_then(|key| entries.swap_remove(&key))
                    .unwrap_or_default(),
            },
            _ => return PresentationProvenance::None,
        };
    }
    provenance
}

fn static_index_key(arg: &hir::expr::IndexArg) -> Option<IndexEntryKey> {
    match arg {
        hir::expr::IndexArg::Variant(variant) => {
            Some(IndexEntryKey::named(variant.variant.variant().clone()))
        }
        hir::expr::IndexArg::Expr(expr) => match expr.kind {
            ExprKind::Integer(position) => {
                u64::try_from(position).ok().map(IndexEntryKey::position)
            }
            _ => None,
        },
        hir::expr::IndexArg::Var(_) => None,
    }
}

const fn outer_index(provenance: &PresentationProvenance) -> Option<&IndexTypeRef> {
    match provenance {
        PresentationProvenance::Indexed { index, .. } => Some(index),
        PresentationProvenance::DagCall { output, .. } => outer_index(output),
        PresentationProvenance::None
        | PresentationProvenance::Leaf(_)
        | PresentationProvenance::Struct { .. }
        | PresentationProvenance::Select(_) => None,
    }
}

const fn is_none(provenance: &PresentationProvenance) -> bool {
    matches!(provenance, PresentationProvenance::None)
}

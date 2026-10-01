//! Inference and coverage checking of map literals.

use crate::hir::expr::{Expr, MapEntry, MapEntryKey};
use crate::outcome::Outcome;
use crate::resolved_name::ResolvedIndexVariant;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::evaluation::EvaluationError;
use crate::semantic_error::index::IndexError;
use crate::source_id::SourceId;

use crate::semantic::checked_type::{IndexDisplayName, IndexTypeRef, Symbolic};
use crate::semantic_error::SemanticError;
use crate::syntax::index_name::IndexEntryKey;
use crate::syntax::span::Span;
use crate::tir::typed::NatPolyForm;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::format_checked_type;

use super::context::Infer;
use super::nat_forms::finite_index_error;
use super::override_deps::IndexNominalUse;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MapLiteralVariantKey {
    Declared(ResolvedIndexVariant),
    Finite { form: NatPolyForm, position: u64 },
}

impl MapLiteralVariantKey {
    fn entry_key(&self) -> IndexEntryKey {
        match self {
            Self::Declared(resolved) => IndexEntryKey::named(resolved.variant().clone()),
            Self::Finite { position, .. } => IndexEntryKey::position(*position),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Declared(resolved) => resolved.to_string(),
            Self::Finite { form, position } => {
                format!("{}.#{position}", IndexDisplayName::Finite(form.clone()))
            }
        }
    }
}

#[derive(Debug, Clone)]
struct MapLiteralAxis {
    index: IndexTypeRef<Symbolic>,
    entry_keys: Vec<IndexEntryKey>,
}

/// Checked size of a map literal's Cartesian key space.
///
/// Construction performs checked multiplication, so downstream coverage logic
/// cannot accidentally compare against a wrapped cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MapCoverageCardinality(usize);

impl MapCoverageCardinality {
    fn checked_from_axes(
        axes: &[Vec<MapLiteralVariantKey>],
        src: SourceId,
        span: Span,
    ) -> Result<Self, SemanticError> {
        axes.iter()
            .try_fold(1_usize, |product, axis| {
                product.checked_mul(axis.len()).ok_or_else(|| {
                    SemanticError::located(
                        src,
                        span,
                        EvaluationError::Failed {
                            message: "map literal key-space cardinality exceeds supported size"
                                .to_string(),
                        },
                    )
                })
            })
            .map(Self)
    }

    const fn get(self) -> usize {
        self.0
    }
}

/// Find one absent Cartesian tuple without materializing the product.
///
/// Among the first `provided.len() + 1` tuples at least one must be absent, so
/// this walk is bounded by source-sized input even when the declared product is
/// enormous.
fn first_missing_map_tuple(
    axes: &[Vec<MapLiteralVariantKey>],
    provided: &std::collections::HashSet<Vec<MapLiteralVariantKey>>,
) -> Option<Vec<MapLiteralVariantKey>> {
    let mut offsets = vec![0_usize; axes.len()];
    for _ in 0..=provided.len() {
        let candidate = axes
            .iter()
            .zip(&offsets)
            .map(|(axis, offset)| axis[*offset].clone())
            .collect::<Vec<_>>();
        if !provided.contains(&candidate) {
            return Some(candidate);
        }

        let mut advanced = false;
        for axis in (0..axes.len()).rev() {
            offsets[axis] = offsets[axis].checked_add(1)?;
            if offsets[axis] < axes[axis].len() {
                advanced = true;
                break;
            }
            offsets[axis] = 0;
        }
        if !advanced {
            return None;
        }
    }
    None
}

impl MapLiteralAxis {
    fn variant_key(&self, key: IndexEntryKey) -> Result<MapLiteralVariantKey, IndexEntryKey> {
        match (&self.index, key) {
            (IndexTypeRef::Declared(reference), IndexEntryKey::Named(variant)) => {
                Ok(MapLiteralVariantKey::Declared(ResolvedIndexVariant::new(
                    reference.resolved().clone(),
                    variant,
                )))
            }
            (IndexTypeRef::Finite(reference), IndexEntryKey::Position(position)) => {
                Ok(MapLiteralVariantKey::Finite {
                    form: reference.form(),
                    position,
                })
            }
            (_, incompatible) => Err(incompatible),
        }
    }
}

fn inferred_index_for_hir_map_key(
    key: &MapEntryKey,
    src: SourceId,
) -> Result<IndexTypeRef<Symbolic>, SemanticError> {
    match key {
        MapEntryKey::IndexVariant(variant) => {
            Ok(IndexTypeRef::from_resolved(variant.variant.index().clone()))
        }
        MapEntryKey::FinitePosition { size, position } => {
            IndexTypeRef::from_finite_index_form(NatPolyForm::from_constant(*size))
                .map_err(|err| finite_index_error(err, src, position.span))
        }
    }
}

fn hir_map_entry_key(key: &MapEntryKey) -> IndexEntryKey {
    match key {
        MapEntryKey::IndexVariant(variant) => {
            IndexEntryKey::named(variant.variant.variant().clone())
        }
        MapEntryKey::FinitePosition { position, .. } => IndexEntryKey::position(position.value),
    }
}

impl Infer<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive validation of map literal entries"
    )]
    pub(super) fn infer_hir_map_literal(
        &self,
        expr: &Expr,
        entries: &[MapEntry],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        for entry in entries {
            for key in &entry.keys {
                if let MapEntryKey::IndexVariant(variant) = key {
                    self.check_index_override_dependency(
                        &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                        IndexNominalUse::Label(variant.variant.variant()),
                    )?;
                }
            }
        }
        let Some(first_entry) = entries.first() else {
            return Err(SemanticError::located(
                self.env.src,
                expr.span,
                EvaluationError::Failed {
                    message: "empty map literal".to_string(),
                },
            )
            .into());
        };
        let arity = first_entry.keys.len();
        for entry in entries.iter().skip(1) {
            if entry.keys.len() != arity {
                return Err(SemanticError::located(self.env.src, expr.span, EvaluationError::Failed { message: format!(
                        "map literal entries have inconsistent key arity: expected {arity}, found {}",
                        entry.keys.len()
                    ) }).into());
            }
        }

        let mut axes = Vec::with_capacity(arity);
        for key in &first_entry.keys {
            let index = inferred_index_for_hir_map_key(key, self.env.src)?;
            let idx_def =
                crate::tir::dim_check::infer::index_def_for_inferred(&index, self.env.tir)
                    .ok_or_else(|| {
                        SemanticError::located(
                            self.env.src,
                            expr.span,
                            IndexError::UnknownIndex {
                                name: index.display_name(),
                            },
                        )
                    })?;
            if idx_def.is_coordinate() {
                return Err(SemanticError::located(self.env.src, expr.span, EvaluationError::Failed { message: format!(
                        "coordinate index `{index}` cannot be used as a map/table literal key; use a `for` comprehension instead"
                    ) }).into());
            }
            axes.push(MapLiteralAxis {
                index,
                entry_keys: idx_def.entry_keys(),
            });
        }
        for entry in entries.iter().skip(1) {
            for (i, key) in entry.keys.iter().enumerate() {
                let key_index = inferred_index_for_hir_map_key(key, self.env.src)?;
                if key_index != axes[i].index {
                    return Err(SemanticError::located(
                        self.env.src,
                        expr.span,
                        IndexError::IndexMismatch {
                            expected: axes[i].index.display_name(),
                            found: key_index.display_name(),
                        },
                    )
                    .into());
                }
            }
        }

        let incompatible_key_error = |key: IndexEntryKey| {
            SemanticError::located(
                self.env.src,
                expr.span,
                EvaluationError::Failed {
                    message: format!("map entry key `{key}` does not match its index category"),
                },
            )
        };
        let axes_variant_keys: Vec<Vec<MapLiteralVariantKey>> = axes
            .iter()
            .map(|axis| {
                axis.entry_keys
                    .iter()
                    .cloned()
                    .map(|key| axis.variant_key(key).map_err(&incompatible_key_error))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected_cardinality =
            MapCoverageCardinality::checked_from_axes(&axes_variant_keys, self.env.src, expr.span)?;
        let mut provided_tuples = std::collections::HashSet::new();
        for entry in entries {
            self.control.checkpoint()?;
            let tuple: Vec<MapLiteralVariantKey> = entry
                .keys
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    let entry_key = hir_map_entry_key(key);
                    if !axes[i].entry_keys.contains(&entry_key) {
                        return match (arity, entry_key) {
                            (1, extra) => Err(SemanticError::located(
                                self.env.src,
                                expr.span,
                                IndexError::ExtraVariants {
                                    index_name: axes[0].index.display_name(),
                                    extra: vec![extra],
                                },
                            )),
                            (_, IndexEntryKey::Named(variant_name)) => Err(SemanticError::located(
                                self.env.src,
                                expr.span,
                                IndexError::UnknownVariant {
                                    index_name: axes[i].index.display_name(),
                                    variant_name,
                                },
                            )),
                            (_, IndexEntryKey::Position(position)) => Err(SemanticError::located(
                                self.env.src,
                                expr.span,
                                EvaluationError::Failed {
                                    message: format!(
                                        "position #{position} is outside index `{}`",
                                        axes[i].index
                                    ),
                                },
                            )),
                        };
                    }
                    axes[i]
                        .variant_key(entry_key)
                        .map_err(&incompatible_key_error)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !provided_tuples.insert(tuple) {
                return Err(SemanticError::located(
                    self.env.src,
                    expr.span,
                    EvaluationError::Failed {
                        message: "duplicate map literal entry".to_string(),
                    },
                )
                .into());
            }
        }

        if let Some(missing_count) = std::num::NonZeroUsize::new(
            expected_cardinality
                .get()
                .saturating_sub(provided_tuples.len()),
        ) {
            if arity == 1 {
                let missing = axes_variant_keys[0]
                    .iter()
                    .filter(|key| !provided_tuples.contains(&vec![(*key).clone()]))
                    .map(MapLiteralVariantKey::entry_key)
                    .collect();
                return Err(SemanticError::located(
                    self.env.src,
                    expr.span,
                    IndexError::MissingVariants {
                        index_name: axes[0].index.display_name(),
                        missing,
                    },
                )
                .into());
            }
            let first_missing = first_missing_map_tuple(&axes_variant_keys, &provided_tuples)
                .ok_or_else(|| {
                    SemanticError::internal_error(
                        "map coverage count and tuple membership disagree".to_string(),
                        self.env.src,
                        crate::diagnostic_anchor::DiagnosticAnchor::Source(expr.span),
                    )
                })?;
            let witness = first_missing
                .iter()
                .map(MapLiteralVariantKey::display)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(SemanticError::located(self.env.src, expr.span, EvaluationError::Failed { message: format!(
                    "non-exhaustive map literal: missing {missing_count} entries; first missing entry is ({witness})"
                ) }).into());
        }

        let first_type = self.infer_hir_type(&first_entry.value)?;
        if let CheckedType::Indexed { index, .. } = &first_type {
            let inner_is_label =
                crate::tir::dim_check::infer::index_def_for_inferred(index, self.env.tir)
                    .is_some_and(|def| !def.is_coordinate());
            if inner_is_label {
                return Err(SemanticError::located(self.env.src, first_entry.value.span, EvaluationError::Failed { message: "map literal element type must be a value type, not an indexed type; use tuple keys for multi-axis map literals".to_string() }).into());
            }
        }
        for entry in entries.iter().skip(1) {
            let entry_type = self.infer_hir_type(&entry.value)?;
            if entry_type != first_type {
                return Err(SemanticError::located(
                    self.env.src,
                    entry.value.span,
                    DimensionError::DimensionMismatchInAnnotation {
                        declared: format_checked_type(&first_type, self.env.registry),
                        inferred: format_checked_type(&entry_type, self.env.registry),
                    },
                )
                .into());
            }
        }
        let mut result = first_type;
        for axis in axes.iter().rev() {
            result = CheckedType::Indexed {
                element: Box::new(result),
                index: axis.index.clone(),
            };
        }
        Ok(result)
    }
}

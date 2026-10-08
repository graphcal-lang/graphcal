//! Inference of key forms, for-comprehensions, and index access.

use crate::hir::expr::{Expr, ForBinding, ForBindingIndex, IndexArg};
use crate::outcome::Outcome;
use crate::semantic_error::dimension::DimensionError;
use crate::semantic_error::dimension_mismatch::{
    MismatchOperand, MismatchRule, OperandExpectation,
};
use crate::semantic_error::index::IndexError;
use crate::semantic_error::structure::StructError;
use crate::semantic_error::structure::UnknownLocal;
use crate::source_id::SourceId;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::semantic::checked_type::{IndexDisplayName, IndexTypeRef, Symbolic};
use crate::semantic_error::SemanticError;
use crate::syntax::span::Span;
use crate::tir::typed::NatPolyForm;

use crate::semantic::checked_type::CheckedType;
use crate::tir::dim_check::helpers::expect_quantity;

use super::context::Infer;
use super::nat_forms::finite_index_error;
use super::operators::try_const_int;
use super::override_deps::IndexNominalUse;

impl Infer<'_> {
    /// Infer a key introduction form.
    ///
    /// `key(Fin(N), c)` is compile-time membership-checked and infallible;
    /// `fin_key(Fin(N), e)` is the explicit runtime-checked constructor; the
    /// coordinate searches require a coordinate axis and a matching-dimension
    /// quantity argument.
    pub(super) fn infer_hir_key_form(
        &self,
        expr: &Expr,
        kind: crate::syntax::ast::KeyFormKind,
        axis: &ForBindingIndex,
        axis_span: Span,
        arg: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        use crate::syntax::ast::KeyFormKind;

        let arg_type = self.infer_hir_type(arg)?;
        // Resolve the axis identity and, for Fin axes, its cardinality form.
        let (index_identity, finite_form) = match axis {
            ForBindingIndex::Named(index) => {
                let identity = IndexTypeRef::from_resolved(index.value.clone());
                let idx_def =
                    crate::tir::dim_check::infer::index_def_for_inferred(&identity, self.env.tir)
                        .ok_or_else(|| {
                        SemanticError::located(
                            self.env.src,
                            index.span,
                            IndexError::UnknownIndex {
                                name: identity.display_name(),
                            },
                        )
                    })?;
                let finite_form = idx_def.finite_index_size().map(NatPolyForm::from_constant);
                (identity, finite_form)
            }
            ForBindingIndex::Finite { cardinality, span } => {
                let form = cardinality.value.clone();
                let identity = IndexTypeRef::from_finite_index_form(form.clone())
                    .map_err(|err| finite_index_error(err, self.env.src, *span))?;
                (identity, Some(form))
            }
        };
        match kind {
            KeyFormKind::Static => {
                let Some(form) = &finite_form else {
                    return Err(SemanticError::located(
                        self.env.src,
                        axis_span,
                        IndexError::KeyRequiresFiniteAxis,
                    )
                    .into());
                };
                if arg_type != CheckedType::Int {
                    return Err(SemanticError::located(
                        self.env.src,
                        arg.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(
                                OperandExpectation::StaticNatPosition,
                            )),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::KeyStaticPosition),
                        },
                    )
                    .into());
                }
                let Some(position) = try_const_int(arg) else {
                    return Err(SemanticError::located(
                        self.env.src,
                        arg.span,
                        IndexError::KeyPositionNotStatic,
                    )
                    .into());
                };
                let Ok(position_u64) = u64::try_from(position) else {
                    return Err(SemanticError::located(
                        self.env.src,
                        arg.span,
                        IndexError::NegativeKeyPosition { position },
                    )
                    .into());
                };
                if form.is_constant() {
                    let size = form.constant();
                    if position_u64 >= size {
                        return Err(SemanticError::located(
                            self.env.src,
                            arg.span,
                            IndexError::KeyPositionOutOfBounds {
                                position,
                                axis: IndexDisplayName::Finite(form.clone()),
                            },
                        )
                        .into());
                    }
                }
                self.control.retain_static_index(
                    expr,
                    arg,
                    &index_identity,
                    position_u64,
                    crate::tir::static_index::StaticIndexUse::Key,
                );
            }
            KeyFormKind::Fin => {
                if finite_form.is_none() {
                    return Err(SemanticError::located(
                        self.env.src,
                        axis_span,
                        IndexError::FinKeyRequiresFiniteAxis {
                            axis: index_identity.display_name(),
                        },
                    )
                    .into());
                }
                if arg_type != CheckedType::Int {
                    return Err(SemanticError::located(
                        self.env.src,
                        arg.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Expected(OperandExpectation::Int)),
                            found: Box::new(MismatchOperand::Type(
                                arg_type.spelling(&self.env.registry.dimensions),
                            )),
                            help: Box::new(MismatchRule::FinKeyIntPosition),
                        },
                    )
                    .into());
                }
            }
            KeyFormKind::Floor | KeyFormKind::Ceil | KeyFormKind::Nearest => {
                let idx_def = crate::tir::dim_check::infer::index_def_for_inferred(
                    &index_identity,
                    self.env.tir,
                );
                let dimension = match idx_def
                    .as_deref()
                    .and_then(crate::semantic::index_def::IndexDef::coordinate_dimension)
                {
                    Some(dimension) => dimension.clone(),
                    None => {
                        return Err(SemanticError::located(
                            self.env.src,
                            axis_span,
                            IndexError::CoordinateSearchRequiresCoordinateAxis {
                                function: kind,
                                axis: index_identity.display_name(),
                            },
                        )
                        .into());
                    }
                };
                let arg_dim =
                    expect_quantity(&arg_type, self.env.registry, self.env.src, arg.span)?;
                if arg_dim != dimension {
                    return Err(SemanticError::located(
                        self.env.src,
                        arg.span,
                        DimensionError::DimensionMismatch {
                            expected: Box::new(MismatchOperand::Dimension(
                                self.env.registry.dimensions.dimension_spelling(&dimension),
                            )),
                            found: Box::new(MismatchOperand::Dimension(
                                self.env.registry.dimensions.dimension_spelling(&arg_dim),
                            )),
                            help: Box::new(MismatchRule::CoordinateSearchAxisDimension(kind)),
                        },
                    )
                    .into());
                }
            }
        }
        Ok(CheckedType::Key(index_identity))
    }

    pub(super) fn infer_hir_for_comp(
        &self,
        bindings: &[ForBinding],
        body: &Expr,
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let mut inner_locals = self.locals.child(Vec::new());
        for binding in bindings {
            // Every loop variable is a key of its axis. Coordinate arithmetic
            // goes through coord(), integer use of a Fin key through to_int().
            let var_type = match &binding.index {
                ForBindingIndex::Named(index) => {
                    let index_identity = IndexTypeRef::from_resolved(index.value.clone());
                    crate::tir::dim_check::infer::index_def_for_inferred(
                        &index_identity,
                        self.env.tir,
                    )
                    .ok_or_else(|| {
                        SemanticError::located(
                            self.env.src,
                            index.span,
                            IndexError::UnknownIndex {
                                name: index_identity.display_name(),
                            },
                        )
                    })?;
                    CheckedType::Key(index_identity)
                }
                ForBindingIndex::Finite { cardinality, span } => {
                    let form = cardinality.value.clone();
                    CheckedType::Key(
                        IndexTypeRef::from_finite_index_form(form)
                            .map_err(|err| finite_index_error(err, self.env.src, *span))?,
                    )
                }
            };
            inner_locals.bind(binding.local.id, var_type);
        }
        let mut result = self.with_locals(&inner_locals).infer_hir_type(body)?;
        for binding in bindings.iter().rev() {
            let index = match &binding.index {
                ForBindingIndex::Named(index) => IndexTypeRef::from_resolved(index.value.clone()),
                ForBindingIndex::Finite { cardinality, span } => {
                    let form = cardinality.value.clone();
                    IndexTypeRef::from_finite_index_form(form)
                        .map_err(|err| finite_index_error(err, self.env.src, *span))?
                }
            };
            result = CheckedType::Indexed {
                element: Box::new(result),
                index,
            };
        }
        Ok(result)
    }
}

fn finite_axis_form(
    index: &IndexTypeRef<Symbolic>,
    declared_definition: Option<&crate::semantic::index_def::IndexDef>,
    src: SourceId,
    span: Span,
) -> Result<Option<NatPolyForm>, SemanticError> {
    match index {
        IndexTypeRef::Finite(reference) => Ok(Some(reference.form())),
        IndexTypeRef::Declared(reference) => {
            let definition = declared_definition.ok_or_else(|| {
                SemanticError::internal_error(
                    format!(
                        "declared indexed axis `{}` has no semantic index definition",
                        reference.resolved()
                    ),
                    src,
                    DiagnosticAnchor::Source(span),
                )
            })?;
            Ok(definition
                .finite_index_size()
                .map(NatPolyForm::from_constant))
        }
    }
}

impl Infer<'_> {
    pub(super) fn infer_hir_index_access(
        &self,
        expr: &Expr,
        inner: &Expr,
        args: &[IndexArg],
    ) -> Result<CheckedType<Symbolic>, Outcome<SemanticError>> {
        let mut current = self.infer_hir_type(inner)?;
        for arg in args {
            let CheckedType::Indexed { element, index } = current else {
                return Err(SemanticError::located(
                    self.env.src,
                    expr.span,
                    IndexError::IndexingNonIndexedValue,
                )
                .into());
            };
            match arg {
                IndexArg::Variant(variant) => {
                    self.check_index_override_dependency(
                        &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                        IndexNominalUse::Label(variant.variant.variant()),
                    )?;
                    let arg_index = IndexTypeRef::from_resolved(variant.variant.index().clone());
                    if arg_index != index {
                        return Err(SemanticError::located(
                            self.env.src,
                            variant.path_span(),
                            IndexError::IndexMismatch {
                                expected: index.display_name(),
                                found: arg_index.display_name(),
                            },
                        )
                        .into());
                    }
                }
                IndexArg::Var(local) => {
                    let Some(var_type) = self.locals.get(local.value) else {
                        return Err(SemanticError::located(
                            self.env.src,
                            local.span,
                            StructError::UnknownLocalRef {
                                name: UnknownLocal::Slot(local.value),
                            },
                        )
                        .into());
                    };
                    match var_type {
                        // Loop variables are keys of their axes: accept on axis
                        // identity, with Fin widening (`N <= M`).
                        CheckedType::Key(key_index) => {
                            let axis_form = finite_axis_form(
                                &index,
                                crate::tir::dim_check::infer::index_def_for_inferred(
                                    &index,
                                    self.env.tir,
                                )
                                .as_deref(),
                                self.env.src,
                                local.span,
                            )?;
                            let accepted = match (key_index.finite_index_form(), &axis_form) {
                                (Some(key_form), Some(axis_form)) => key_form.is_leq(axis_form),
                                _ => key_index == &index,
                            };
                            if !accepted {
                                return Err(SemanticError::located(
                                    self.env.src,
                                    local.span,
                                    IndexError::IndexMismatch {
                                        expected: index.display_name(),
                                        found: key_index.display_name(),
                                    },
                                )
                                .into());
                            }
                        }
                        CheckedType::Quantity(_) => {
                            return Err(SemanticError::located(
                                self.env.src,
                                local.span,
                                IndexError::QuantityLocalIndexesCoordinateIndex {
                                    index: index.display_name(),
                                },
                            )
                            .into());
                        }
                        _ => {
                            return Err(SemanticError::located(
                                self.env.src,
                                local.span,
                                IndexError::InvalidIndexVariable { local: local.value },
                            )
                            .into());
                        }
                    }
                }
                IndexArg::Expr(index_expr) => {
                    let expr_type = self.infer_hir_type(index_expr)?;
                    let index_form = finite_axis_form(
                        &index,
                        crate::tir::dim_check::infer::index_def_for_inferred(&index, self.env.tir)
                            .as_deref(),
                        self.env.src,
                        index_expr.span,
                    )?;
                    // A key-typed expression selects by axis identity: exact for
                    // named and coordinate axes, widening (`N <= M`) for Fin.
                    if let CheckedType::Key(key_index) = &expr_type {
                        let accepted = match (key_index.finite_index_form(), &index_form) {
                            (Some(key_form), Some(axis_form)) => key_form.is_leq(axis_form),
                            _ => *key_index == index,
                        };
                        if !accepted {
                            return Err(SemanticError::located(
                                self.env.src,
                                index_expr.span,
                                IndexError::IndexMismatch {
                                    expected: index.display_name(),
                                    found: key_index.display_name(),
                                },
                            )
                            .into());
                        }
                        current = *element;
                        continue;
                    }
                    let Some(index_form) = index_form else {
                        return Err(SemanticError::located(
                            self.env.src,
                            index_expr.span,
                            IndexError::IntegerIndexIntoNonFiniteIndex {
                                index: index.display_name(),
                            },
                        )
                        .into());
                    };
                    match expr_type {
                        CheckedType::Int => {
                            // Runtime-checked Int indexing was removed: only a
                            // statically discharged constant selects implicitly;
                            // a runtime Int goes through the explicit fin_key().
                            let Some(constant) = try_const_int(index_expr) else {
                                return Err(SemanticError::located(
                                    self.env.src,
                                    index_expr.span,
                                    IndexError::ImplicitRuntimeIntIndex {
                                        index: index.display_name(),
                                    },
                                )
                                .into());
                            };
                            let position = check_constant_finite_index_index(
                                constant,
                                index_expr.span,
                                &index_form,
                                self.env.src,
                            )?;
                            self.control.retain_static_index(
                                expr,
                                index_expr,
                                &index,
                                position,
                                crate::tir::static_index::StaticIndexUse::Selection,
                            );
                        }
                        _ => {
                            return Err(SemanticError::located(
                                self.env.src,
                                index_expr.span,
                                IndexError::NonIntegerIndexExpression {
                                    found: expr_type.spelling(&self.env.registry.dimensions),
                                },
                            )
                            .into());
                        }
                    }
                }
            }
            current = *element;
        }
        Ok(current)
    }
}

fn check_constant_finite_index_index(
    index: i64,
    index_span: Span,
    index_form: &NatPolyForm,
    src: SourceId,
) -> Result<u64, SemanticError> {
    let Ok(index_u64) = u64::try_from(index) else {
        return Err(SemanticError::located(
            src,
            index_span,
            IndexError::NegativeIndex { index },
        ));
    };
    if !index_form.is_constant() {
        return Ok(index_u64);
    }
    let size = index_form.constant();
    if index_u64 >= size {
        return Err(SemanticError::located(
            src,
            index_span,
            IndexError::IndexOutOfBounds {
                index,
                axis: IndexDisplayName::Finite(index_form.clone()),
            },
        ));
    }
    Ok(index_u64)
}

#[cfg(test)]
mod finite_axis_form_tests {
    use super::*;
    use crate::dag_id::DagId;
    use crate::resolved_name::ResolvedIndexName;
    use crate::syntax::index_name::IndexName;
    use std::path::Path;

    #[test]
    fn structural_finite_axis_does_not_require_a_registry_definition() {
        let source = crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new(String::new()));
        let form = NatPolyForm::from_constant(5);
        let index = IndexTypeRef::from_finite_index_form(form.clone()).unwrap();

        assert_eq!(
            finite_axis_form(&index, None, source, Span::new(0, 0)).unwrap(),
            Some(form)
        );
    }

    #[test]
    fn declared_axis_without_a_semantic_definition_is_an_internal_error() {
        let source = crate::source_registry::SourceRegistry::new()
            .register("test.gcl", std::sync::Arc::new("values[key]".to_string()));
        let owner = DagId::from_virtual_relative_path(Path::new("test.gcl")).unwrap();
        let resolved = ResolvedIndexName::for_test(owner, IndexName::expect_valid("Missing"));
        let index = IndexTypeRef::from_resolved(resolved);

        let error = finite_axis_form(&index, None, source, Span::new(7, 3)).unwrap_err();
        match error {
            SemanticError::Internal(internal) => assert!(
                internal.message().contains(
                    "declared indexed axis `test.Missing` has no semantic index definition"
                ),
                "{}",
                internal.message()
            ),
            other @ SemanticError::Located(_) => panic!("expected internal error, got {other:?}"),
        }
    }
}

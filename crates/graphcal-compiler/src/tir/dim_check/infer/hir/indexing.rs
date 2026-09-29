//! Inference of key forms, for-comprehensions, and index access.

use crate::hir::expr::{Expr, ForBinding, ForBindingIndex, IndexArg};
use std::sync::Arc;

use miette::NamedSource;

use crate::diagnostic_anchor::DiagnosticAnchor;
use crate::registry::checked_type::{IndexDisplayName, IndexTypeRef, Symbolic};
use crate::registry::error::GraphcalError;
use crate::syntax::span::Span;
use crate::tir::typed::NatPolyForm;

use crate::registry::checked_type::CheckedType;
use crate::tir::dim_check::helpers::{expect_quantity, format_checked_type};

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
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        use crate::syntax::ast::KeyFormKind;

        let arg_type = self.infer_hir_type(arg)?;
        // Resolve the axis identity and, for Fin axes, its cardinality form.
        let (index_identity, finite_form) = match axis {
            ForBindingIndex::Named(index) => {
                let identity = IndexTypeRef::from_resolved(index.value.clone());
                let idx_def =
                    crate::tir::dim_check::infer::index_def_for_inferred(&identity, self.env.tir)
                        .ok_or_else(|| GraphcalError::UnknownIndex {
                        name: identity.display_name(),
                        src: self.env.src.clone(),
                        span: index.span.into(),
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
                    return Err(GraphcalError::EvalError {
                        message: "key() constructs Fin-axis keys; named-axis keys are written as \
                              qualified labels and coordinate keys come from argmax/argmin or \
                              the coordinate searches"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: axis_span.into(),
                    });
                };
                if arg_type != CheckedType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "a static Nat position".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "key(Fin(N), position) takes an integer position".to_string(),
                        src: self.env.src.clone(),
                        span: arg.span.into(),
                    });
                }
                let Some(position) = try_const_int(arg) else {
                    return Err(GraphcalError::EvalError {
                        message: "key() requires a static position; use fin_key() for a \
                              runtime-checked position"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: arg.span.into(),
                    });
                };
                if position < 0 {
                    return Err(GraphcalError::EvalError {
                        message: format!("key() position evaluated to negative value: {position}"),
                        src: self.env.src.clone(),
                        span: arg.span.into(),
                    });
                }
                if form.is_constant() {
                    let size = form.constant();
                    let position_u64 = u64::try_from(position).unwrap_or(u64::MAX);
                    if position_u64 >= size {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "key() position {position} is out of bounds for {}",
                                IndexDisplayName::Finite(form.clone())
                            ),
                            src: self.env.src.clone(),
                            span: arg.span.into(),
                        });
                    }
                }
                self.control.retain_static_index(
                    expr,
                    arg,
                    &index_identity,
                    u64::try_from(position).map_err(|_| {
                        GraphcalError::internal_error(
                            "checked position is negative",
                            self.env.src,
                            DiagnosticAnchor::Source(arg.span),
                        )
                    })?,
                    crate::tir::static_index::StaticIndexUse::Key,
                );
                Ok(CheckedType::Key(index_identity))
            }
            KeyFormKind::Fin => {
                if finite_form.is_none() {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "fin_key() requires a Fin(...) axis, got `{index_identity}`"
                        ),
                        src: self.env.src.clone(),
                        span: axis_span.into(),
                    });
                }
                if arg_type != CheckedType::Int {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "Int".to_string(),
                        found: format_checked_type(&arg_type, self.env.registry),
                        help: "fin_key(Fin(N), position) takes an Int position, checked at \
                           runtime"
                            .to_string(),
                        src: self.env.src.clone(),
                        span: arg.span.into(),
                    });
                }
                Ok(CheckedType::Key(index_identity))
            }
            KeyFormKind::Floor | KeyFormKind::Ceil | KeyFormKind::Nearest => {
                let idx_def = crate::tir::dim_check::infer::index_def_for_inferred(
                    &index_identity,
                    self.env.tir,
                );
                let dimension = match idx_def
                    .as_deref()
                    .and_then(crate::registry::types::IndexDef::coordinate_dimension)
                {
                    Some(dimension) => dimension.clone(),
                    None => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "{}() requires a coordinate axis, got `{}`",
                                kind.as_str(),
                                index_identity
                            ),
                            src: self.env.src.clone(),
                            span: axis_span.into(),
                        });
                    }
                };
                let arg_dim =
                    expect_quantity(&arg_type, self.env.registry, self.env.src, arg.span)?;
                if arg_dim != dimension {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: self.env.registry.dimensions.format_dimension(&dimension),
                        found: self.env.registry.dimensions.format_dimension(&arg_dim),
                        help: format!("{}() takes a quantity in the axis dimension", kind.as_str()),
                        src: self.env.src.clone(),
                        span: arg.span.into(),
                    });
                }
                Ok(CheckedType::Key(index_identity))
            }
        }
    }

    pub(super) fn infer_hir_for_comp(
        &self,
        bindings: &[ForBinding],
        body: &Expr,
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
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
                    .ok_or_else(|| GraphcalError::UnknownIndex {
                        name: index_identity.display_name(),
                        src: self.env.src.clone(),
                        span: index.span.into(),
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
    declared_definition: Option<&crate::registry::types::IndexDef>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Option<NatPolyForm>, GraphcalError> {
    match index {
        IndexTypeRef::Finite(reference) => Ok(Some(reference.form())),
        IndexTypeRef::Declared(reference) => {
            let definition = declared_definition.ok_or_else(|| {
                GraphcalError::internal_error(
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
    ) -> Result<CheckedType<Symbolic>, GraphcalError> {
        let mut current = self.infer_hir_type(inner)?;
        for arg in args {
            let CheckedType::Indexed { element, index } = current else {
                return Err(GraphcalError::EvalError {
                    message: "indexing a non-indexed value".to_string(),
                    src: self.env.src.clone(),
                    span: expr.span.into(),
                });
            };
            match arg {
                IndexArg::Variant(variant) => {
                    self.check_index_override_dependency(
                        &IndexTypeRef::from_resolved(variant.variant.index().clone()),
                        IndexNominalUse::Label(variant.variant.variant()),
                    )?;
                    let arg_index = IndexTypeRef::from_resolved(variant.variant.index().clone());
                    if arg_index != index {
                        return Err(GraphcalError::IndexMismatch {
                            expected: index.display_name(),
                            found: arg_index.display_name(),
                            src: self.env.src.clone(),
                            span: variant.path_span().into(),
                        });
                    }
                }
                IndexArg::Var(local) => {
                    let Some(var_type) = self.locals.get(local.value) else {
                        return Err(GraphcalError::UnknownLocalRef {
                            name: format!("#{}", local.value.index()),
                            src: self.env.src.clone(),
                            span: local.span.into(),
                        });
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
                                return Err(GraphcalError::IndexMismatch {
                                    expected: index.display_name(),
                                    found: key_index.display_name(),
                                    src: self.env.src.clone(),
                                    span: local.span.into(),
                                });
                            }
                        }
                        CheckedType::Quantity(_) => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "quantity local cannot index into coordinate index `{index}`; use that coordinate index's loop variable"
                                ),
                                src: self.env.src.clone(),
                                span: local.span.into(),
                            });
                        }
                        _ => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "`#{}` is not a valid index variable",
                                    local.value.index()
                                ),
                                src: self.env.src.clone(),
                                span: local.span.into(),
                            });
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
                            return Err(GraphcalError::IndexMismatch {
                                expected: index.display_name(),
                                found: key_index.display_name(),
                                src: self.env.src.clone(),
                                span: index_expr.span.into(),
                            });
                        }
                        current = *element;
                        continue;
                    }
                    let Some(index_form) = index_form else {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "integer expression cannot index into non-finite-index index `{index}`"
                            ),
                            src: self.env.src.clone(),
                            span: index_expr.span.into(),
                        });
                    };
                    match expr_type {
                        CheckedType::Int => {
                            // Runtime-checked Int indexing was removed: only a
                            // statically discharged constant selects implicitly;
                            // a runtime Int goes through the explicit fin_key().
                            let Some(constant) = try_const_int(index_expr) else {
                                return Err(GraphcalError::EvalError {
                                    message: format!(
                                        "a runtime Int cannot index `{index}` implicitly; write \
                                     `fin_key({index}, ...)` to make the range check explicit",
                                    ),
                                    src: self.env.src.clone(),
                                    span: index_expr.span.into(),
                                });
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
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "index expression must be an integer type, got {}",
                                    format_checked_type(&expr_type, self.env.registry)
                                ),
                                src: self.env.src.clone(),
                                span: index_expr.span.into(),
                            });
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
    src: &NamedSource<Arc<String>>,
) -> Result<u64, GraphcalError> {
    let Ok(index_u64) = u64::try_from(index) else {
        return Err(GraphcalError::EvalError {
            message: format!("index expression evaluated to negative value: {index}"),
            src: src.clone(),
            span: index_span.into(),
        });
    };
    if !index_form.is_constant() {
        return Ok(index_u64);
    }
    let size = index_form.constant();
    if index_u64 >= size {
        return Err(GraphcalError::EvalError {
            message: format!(
                "index {index} out of bounds for {}",
                IndexDisplayName::Finite(index_form.clone())
            ),
            src: src.clone(),
            span: index_span.into(),
        });
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
        let source = NamedSource::new("test.gcl", Arc::new(String::new()));
        let form = NatPolyForm::from_constant(5);
        let index = IndexTypeRef::from_finite_index_form(form.clone()).unwrap();

        assert_eq!(
            finite_axis_form(&index, None, &source, Span::new(0, 0)).unwrap(),
            Some(form)
        );
    }

    #[test]
    fn declared_axis_without_a_semantic_definition_is_an_internal_error() {
        let source = NamedSource::new("test.gcl", Arc::new("values[key]".to_string()));
        let owner = DagId::from_virtual_relative_path(Path::new("test.gcl")).unwrap();
        let resolved = ResolvedIndexName::for_test(owner, IndexName::expect_valid("Missing"));
        let index = IndexTypeRef::from_resolved(resolved);

        let error = finite_axis_form(&index, None, &source, Span::new(7, 3)).unwrap_err();
        match error {
            GraphcalError::InternalError { message, .. } => assert!(
                message.contains(
                    "declared indexed axis `test.Missing` has no semantic index definition"
                ),
                "{message}"
            ),
            other => panic!("expected internal error, got {other:?}"),
        }
    }
}

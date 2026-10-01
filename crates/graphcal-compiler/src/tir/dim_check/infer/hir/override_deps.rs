//! Override-reconciliation checks for nominal uses observed during inference.

use crate::hir::types::{GenericArg, IndexRef, ValueType, ValueTypeKind};
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};

use crate::graphcal_error::GraphcalError;
use crate::semantic::checked_type::{IndexTypeRef, Symbolic};
use crate::syntax::span::Span;
use crate::syntax::type_name::FieldName;
use crate::tir::texpr::NominalObservation;

use super::context::Infer;

#[derive(Debug, Clone, Copy)]
pub(super) enum TypeNominalUse<'a> {
    Field {
        field: &'a FieldName,
        span: Span,
    },
    Constructor {
        constructor: &'a ResolvedConstructorName,
        span: Span,
    },
    TypeArgument,
}

impl TypeNominalUse<'_> {
    const fn definition_span(self) -> Option<Span> {
        match self {
            Self::Field { span, .. } | Self::Constructor { span, .. } => Some(span),
            Self::TypeArgument => None,
        }
    }
}

impl Infer<'_> {
    pub(super) fn check_type_override_dependency(
        &self,
        actual: &ResolvedStructTypeName,
        nominal_use: TypeNominalUse<'_>,
    ) -> Result<(), GraphcalError> {
        self.control.observe_nominal(
            match nominal_use {
                TypeNominalUse::Field { field, .. } => NominalObservation::Field {
                    identity: actual.clone(),
                    field: field.clone(),
                },
                TypeNominalUse::Constructor { constructor, .. } => {
                    NominalObservation::Constructor {
                        identity: actual.clone(),
                        constructor: constructor.clone(),
                    }
                }
                TypeNominalUse::TypeArgument => NominalObservation::TypeArgument(actual.clone()),
            },
            nominal_use.definition_span(),
        );
        let Some(owner) = self.owner else {
            return Ok(());
        };
        let Some(reconciliations) = self.env.dag.semantic.override_reconciliations.get(owner)
        else {
            return Ok(());
        };

        for reconciliation in reconciliations {
            for target in &reconciliation.targets {
                let crate::tir::typed::OverrideTarget::Type {
                    overridden,
                    source,
                    replacement,
                } = target
                else {
                    continue;
                };
                if actual != source && actual != replacement {
                    continue;
                }
                let detail = match nominal_use {
                    TypeNominalUse::Field { field, .. } => {
                        format!("field `{field}` of type `{overridden}`")
                    }
                    TypeNominalUse::Constructor { constructor, .. } => format!(
                        "constructor `{}` of type `{overridden}`",
                        constructor.as_str()
                    ),
                    TypeNominalUse::TypeArgument => format!("type `{overridden}`"),
                };
                return Err(GraphcalError::IncludeMustReconcileOverride {
                    overridden: overridden.to_string(),
                    overridden_kind: "type".to_string(),
                    orphan_decl: reconciliation.orphan_decl().to_string(),
                    detail,
                    src: reconciliation.src,
                    span: reconciliation.include_span.into(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum IndexNominalUse<'a> {
    Label(&'a crate::syntax::index_name::IndexVariantName),
    TypeArgument,
}

impl Infer<'_> {
    pub(super) fn check_index_override_dependency(
        &self,
        actual: &IndexTypeRef<Symbolic>,
        nominal_use: IndexNominalUse<'_>,
    ) -> Result<(), GraphcalError> {
        self.control.observe_nominal(
            match nominal_use {
                IndexNominalUse::Label(variant) => NominalObservation::IndexLabel {
                    identity: actual.clone(),
                    variant: variant.clone(),
                },
                IndexNominalUse::TypeArgument => NominalObservation::IndexArgument(actual.clone()),
            },
            None,
        );
        let Some(owner) = self.owner else {
            return Ok(());
        };
        let Some(reconciliations) = self.env.dag.semantic.override_reconciliations.get(owner)
        else {
            return Ok(());
        };

        for reconciliation in reconciliations {
            for target in &reconciliation.targets {
                let crate::tir::typed::OverrideTarget::Index {
                    overridden,
                    source,
                    replacement,
                } = target
                else {
                    continue;
                };
                let source_matches = actual.declared_resolved() == Some(source);
                if !source_matches && !replacement.to_symbolic().matches_ref(actual) {
                    continue;
                }
                let detail = match nominal_use {
                    IndexNominalUse::Label(variant) => {
                        format!("index label `{overridden}#{variant}`")
                    }
                    IndexNominalUse::TypeArgument => format!("index `{overridden}`"),
                };
                return Err(GraphcalError::IncludeMustReconcileOverride {
                    overridden: overridden.to_string(),
                    overridden_kind: "index".to_string(),
                    orphan_decl: reconciliation.orphan_decl().to_string(),
                    detail,
                    src: reconciliation.src,
                    span: reconciliation.include_span.into(),
                });
            }
        }
        Ok(())
    }

    fn check_hir_index_ref_override_dependency(
        &self,
        index: &IndexRef,
    ) -> Result<(), GraphcalError> {
        let actual = match index {
            IndexRef::Concrete(index) => IndexTypeRef::from_resolved(index.value.clone()),
            IndexRef::Finite(cardinality) => {
                let Ok(index) = IndexTypeRef::from_finite_index_form(cardinality.value.clone())
                else {
                    return Ok(());
                };
                index
            }
            IndexRef::GenericParam(_) => return Ok(()),
        };
        self.check_index_override_dependency(&actual, IndexNominalUse::TypeArgument)
    }

    pub(super) fn check_hir_generic_arg_override_dependencies(
        &self,
        arg: &GenericArg,
    ) -> Result<(), GraphcalError> {
        match arg {
            GenericArg::Index(index) => self.check_hir_index_ref_override_dependency(index),
            GenericArg::Type(value_type) => self.check_hir_type_override_dependencies(value_type),
            GenericArg::Dim(_) | GenericArg::Nat(_) => Ok(()),
        }
    }

    fn check_hir_type_override_dependencies(
        &self,
        value_type: &ValueType,
    ) -> Result<(), GraphcalError> {
        match &value_type.kind {
            ValueTypeKind::Struct(name) => {
                self.check_type_override_dependency(&name.value, TypeNominalUse::TypeArgument)
            }
            ValueTypeKind::TypeApplication { name, generic_args } => {
                self.check_type_override_dependency(&name.value, TypeNominalUse::TypeArgument)?;
                generic_args
                    .iter()
                    .try_for_each(|arg| self.check_hir_generic_arg_override_dependencies(arg))
            }
            ValueTypeKind::Key(index) => self.check_hir_index_ref_override_dependency(index),
            ValueTypeKind::Builtin(_)
            | ValueTypeKind::DimExpr(_)
            | ValueTypeKind::GenericTypeParam(_)
            | ValueTypeKind::Complex(_) => Ok(()),
        }
    }
}

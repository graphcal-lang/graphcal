//! Concrete generic substitutions of nominal-type applications and the
//! field types they instantiate.

use crate::hir::nominal::{NominalGenericParam, NominalTypeDef, ResolvedConstructor};
use crate::hir::types::GenericParamId;
use crate::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use crate::semantic_error::evaluation::EvaluationError;
use crate::source_id::SourceId;
use std::collections::HashMap;

use crate::semantic::checked_type::Symbolic;
use crate::semantic_error::SemanticError;
use crate::syntax::ast::GenericConstraint;
use crate::syntax::span::Span;
use crate::syntax::type_name::GenericParamName;

use crate::semantic::checked_type::{CheckedGenericArg, CheckedType};
use crate::tir::typed::{NominalFieldSemantics, NominalMember, ResolvedNominal, Substitution};

pub(in crate::tir::dim_check) fn generic_substitution_prefix(
    type_def: &NominalTypeDef,
    type_args: &[CheckedGenericArg<Symbolic>],
    src: SourceId,
    span: Span,
) -> Result<Substitution, SemanticError> {
    if type_args.len() > type_def.generic_params().len() {
        return Err(SemanticError::located(
            src,
            span,
            EvaluationError::Failed {
                message: format!(
                    "type `{}` expects at most {} generic arguments, got {}",
                    type_def.name(),
                    type_def.generic_params().len(),
                    type_args.len()
                ),
            },
        ));
    }

    // A validated concrete argument, embedded into the symbolic form. An
    // indexed type is not a value type and so cannot bind a `Type` parameter.
    let bound = |param: &NominalGenericParam, arg: &CheckedGenericArg| {
        crate::tir::typed::declared_to_resolved_generic_arg(arg, span)
            .ok_or_else(|| generic_arg_internal_sort_error(param, src, span))
    };
    let mut subs = Substitution::default();
    for (param, arg) in type_def.generic_params().iter().zip(type_args) {
        let sorted = SortedGenericArg::of(param, arg)
            .ok_or_else(|| generic_arg_internal_sort_error(param, src, span))?;
        let concrete = match sorted {
            SortedGenericArg::Dim(dim) => CheckedGenericArg::Dim(dim.clone()),
            SortedGenericArg::Index(index) => {
                CheckedGenericArg::Index(index.to_concrete().ok_or_else(|| {
                    non_concrete_generic_argument(param.name(), &index.to_string(), src, span)
                })?)
            }
            SortedGenericArg::Nat(form) => {
                CheckedGenericArg::Nat(form.constant_value().ok_or_else(|| {
                    non_concrete_generic_argument(param.name(), &form.format(), src, span)
                })?)
            }
            SortedGenericArg::Type(type_expr) => {
                CheckedGenericArg::Type(type_expr.to_concrete().ok_or_else(|| {
                    non_concrete_generic_argument(
                        param.name(),
                        &format!("{type_expr:?}"),
                        src,
                        span,
                    )
                })?)
            }
        };
        subs.bind(param.id().clone(), bound(param, &concrete)?);
    }
    Ok(subs)
}

/// A generic argument of the sort its parameter declares.
#[derive(Debug, Clone, Copy)]
pub(in crate::tir::dim_check) enum SortedGenericArg<'a> {
    Dim(&'a crate::dimension::Dimension),
    Index(&'a crate::semantic::checked_type::IndexTypeRef<Symbolic>),
    Nat(&'a crate::nat::NatPolyForm),
    Type(&'a CheckedType<Symbolic>),
}

impl<'a> SortedGenericArg<'a> {
    /// `arg` as an argument of `param`, or `None` when its sort is not the
    /// one `param` declares.
    pub(in crate::tir::dim_check) const fn of(
        param: &NominalGenericParam,
        arg: &'a CheckedGenericArg<Symbolic>,
    ) -> Option<Self> {
        match (param.constraint(), arg) {
            (GenericConstraint::Dim, CheckedGenericArg::Dim(dim)) => Some(Self::Dim(dim)),
            (GenericConstraint::Index, CheckedGenericArg::Index(index)) => Some(Self::Index(index)),
            (GenericConstraint::Nat, CheckedGenericArg::Nat(form)) => Some(Self::Nat(form)),
            (GenericConstraint::Type, CheckedGenericArg::Type(ty)) => Some(Self::Type(ty)),
            (
                GenericConstraint::Dim
                | GenericConstraint::Index
                | GenericConstraint::Nat
                | GenericConstraint::Type,
                _,
            ) => None,
        }
    }
}

pub(in crate::tir::dim_check) fn concrete_generic_substitutions(
    type_def: &NominalTypeDef,
    type_args: &[CheckedGenericArg<Symbolic>],
    src: SourceId,
    span: Span,
) -> Result<ConcreteGenericSubstitutions, SemanticError> {
    if type_args.len() != type_def.generic_params().len() {
        return Err(SemanticError::located(
            src,
            span,
            EvaluationError::Failed {
                message: format!(
                    "concrete type `{}` requires exactly {} generic arguments, got {}",
                    type_def.name(),
                    type_def.generic_params().len(),
                    type_args.len()
                ),
            },
        ));
    }
    let substitution = generic_substitution_prefix(type_def, type_args, src, span)?;
    let nats = type_def
        .generic_params()
        .iter()
        .zip(type_args)
        .filter_map(|(parameter, arg)| match arg {
            CheckedGenericArg::Nat(form) => form
                .constant_value()
                .map(|value| (parameter.id().clone(), value)),
            CheckedGenericArg::Dim(_)
            | CheckedGenericArg::Index(_)
            | CheckedGenericArg::Type(_) => None,
        })
        .collect();
    Ok(ConcreteGenericSubstitutions { substitution, nats })
}

pub(in crate::tir::dim_check) fn non_concrete_generic_argument(
    parameter: &GenericParamName,
    argument: &str,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::located(
        src,
        span,
        EvaluationError::Failed {
            message: format!("generic argument `{argument}` for `{parameter}` is not concrete"),
        },
    )
}

pub(in crate::tir::dim_check) fn generic_arg_internal_sort_error(
    param: &NominalGenericParam,
    src: SourceId,
    span: Span,
) -> SemanticError {
    SemanticError::internal_error(
        format!(
            "generic argument for `{}` does not match its registered sort",
            param.name()
        ),
        src,
        crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
    )
}

/// Complete, sort-checked, concrete bindings for one nominal application.
///
/// This wrapper can only be constructed after exact arity, sort, and
/// concreteness validation.
#[derive(Clone)]
pub(in crate::tir::dim_check) struct ConcreteGenericSubstitutions {
    substitution: Substitution,
    nats: HashMap<GenericParamId, u64>,
}

impl ConcreteGenericSubstitutions {
    pub(in crate::tir::dim_check) const fn nats(&self) -> &HashMap<GenericParamId, u64> {
        &self.nats
    }

    pub(in crate::tir::dim_check) fn field_type(
        &self,
        resolved: &crate::tir::typed::ResolvedDeclType,
        src: SourceId,
    ) -> Result<CheckedType, SemanticError> {
        instantiate_concrete_type(resolved, &self.substitution, src)
    }
}

/// Instantiate a symbolic type whose every generic parameter `substitution`
/// binds to a concrete argument.
pub(in crate::tir::dim_check) fn instantiate_concrete_type(
    resolved: &crate::tir::typed::ResolvedDeclType,
    substitution: &Substitution,
    src: SourceId,
) -> Result<CheckedType, SemanticError> {
    let instantiated = substitution
        .apply(resolved)
        .map_err(|error| error.into_graphcal(src))?;
    instantiated.to_checked_type(src)
}

pub(in crate::tir::dim_check) fn instantiate_concrete_generic_arg(
    resolved: &crate::tir::typed::ResolvedGenericArg,
    substitution: &Substitution,
    src: SourceId,
) -> Result<CheckedGenericArg, SemanticError> {
    let instantiated = substitution
        .apply_generic_arg(resolved)
        .map_err(|error| error.into_graphcal(src))?;
    crate::tir::typed::resolved_generic_arg_to_declared(&instantiated, src)
}

/// The type of `field` in the application of its nominal type to
/// `type_args`.
pub(in crate::tir::dim_check) fn applied_field_type(
    field: NominalFieldSemantics<'_>,
    type_args: &[CheckedGenericArg<Symbolic>],
    src: SourceId,
    span: Span,
) -> Result<CheckedType, SemanticError> {
    concrete_generic_substitutions(field.member().nominal().definition(), type_args, src, span)?
        .field_type(field.semantics().resolved_type(), src)
}

/// The canonical constructor `constructor` names, with the nominal
/// definition that owns it.
pub(in crate::tir::dim_check) fn resolved_constructor<'t>(
    tir: &'t dyn crate::tir::typed::TirRead,
    constructor: &ResolvedConstructorName,
    src: SourceId,
    span: Span,
) -> Result<&'t ResolvedConstructor, SemanticError> {
    tir.project_type_store()
        .lookup_constructor(constructor)
        .ok_or_else(|| {
            SemanticError::internal_error(
                format!("project type store has no constructor `{constructor}`"),
                src,
                crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
            )
        })
}

/// The nominal type `identity` with its fields' semantics, as `dag` records
/// it.
pub(in crate::tir::dim_check) fn recorded_nominal<'d>(
    dag: &'d crate::tir::typed::DagTIR,
    identity: &ResolvedStructTypeName,
    src: SourceId,
    span: Span,
) -> Result<&'d ResolvedNominal, SemanticError> {
    dag.semantic.type_defs.nominal(identity).ok_or_else(|| {
        SemanticError::internal_error(
            format!("semantic type metadata missing nominal type `{identity}`"),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
        )
    })
}

/// The constructor `constructor` with its fields' semantics, as `dag`
/// records its type.
pub(in crate::tir::dim_check) fn recorded_member<'d>(
    dag: &'d crate::tir::typed::DagTIR,
    constructor: &ResolvedConstructor,
    src: SourceId,
    span: Span,
) -> Result<NominalMember<'d>, SemanticError> {
    dag.semantic.type_defs.member(constructor).ok_or_else(|| {
        SemanticError::internal_error(
            format!(
                "semantic type metadata missing constructor `{}`",
                constructor.identity()
            ),
            src,
            crate::diagnostic_anchor::DiagnosticAnchor::Source(span),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic_param::test_support::type_param;

    fn param(constraint: GenericConstraint) -> NominalGenericParam {
        NominalGenericParam::new(type_param("P"), constraint, None, Span::new(0, 1))
    }

    #[test]
    fn a_generic_argument_is_sorted_only_by_its_parameters_sort() {
        let args = [
            CheckedGenericArg::Dim(crate::dimension::Dimension::dimensionless()),
            CheckedGenericArg::Nat(crate::nat::NatPolyForm::from_constant(2)),
            CheckedGenericArg::Type(CheckedType::Bool),
        ];
        let constraints = [
            GenericConstraint::Dim,
            GenericConstraint::Nat,
            GenericConstraint::Type,
        ];
        for (arg_position, arg) in args.iter().enumerate() {
            for (param_position, constraint) in constraints.iter().enumerate() {
                let sorted = SortedGenericArg::of(&param(*constraint), arg);
                assert_eq!(
                    sorted.is_some(),
                    arg_position == param_position,
                    "{arg:?} for a {constraint:?} parameter"
                );
            }
            assert!(SortedGenericArg::of(&param(GenericConstraint::Index), arg).is_none());
        }
        assert!(matches!(
            SortedGenericArg::of(&param(GenericConstraint::Nat), &args[1]),
            Some(SortedGenericArg::Nat(form)) if form.constant_value() == Some(2)
        ));
    }
}

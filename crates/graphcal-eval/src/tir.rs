//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! The TIR layer resolves ambiguous AST names (`Ident` in `DimTerm::name` and
//! `TypeExprKind::Indexed::indexes`) into concrete dimensions, struct types,
//! generic dimension parameters, or generic index parameters.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{Expr, FnDecl, MulDivOp, TypeExpr, TypeExprKind};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{DimName, FnName, GenericParamName, IndexName, StructTypeName};
use graphcal_syntax::span::Span;

use crate::error::GraphcalError;
use crate::ir::IR;
use crate::registry::Registry;
use crate::resolve::DeclCategory;

// ---------------------------------------------------------------------------
// Resolved type types
// ---------------------------------------------------------------------------

/// A fully-resolved type expression.
///
/// Unlike the raw AST `TypeExpr`, every name here has been classified as a
/// concrete dimension, struct, generic dim param, or generic index param.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTypeExpr {
    /// `Dimensionless`
    Dimensionless,
    /// `Bool`
    Bool,
    /// `Int`
    Int,
    /// A concrete scalar dimension, e.g. `Length * Time^-2`
    Scalar(Dimension),
    /// A struct type name, e.g. `TransferResult`
    Struct(StructTypeName, Span),
    /// A single generic dimension parameter, e.g. `D`
    GenericDimParam(GenericParamName, Span),
    /// A compound dimension expression containing at least one generic param, e.g. `D^2`
    GenericDimExpr {
        terms: Vec<ResolvedDimTerm>,
        span: Span,
    },
    /// An indexed type, e.g. `Velocity[Maneuver]` or `D[I]`
    Indexed {
        base: Box<Self>,
        indexes: Vec<ResolvedIndex>,
    },
}

/// A single term in a resolved dimension expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDimTerm {
    /// A concrete dimension with power and combining operator.
    Concrete {
        dim: Dimension,
        power: i32,
        op: MulDivOp,
    },
    /// A generic dimension parameter with power and combining operator.
    GenericParam {
        name: GenericParamName,
        power: i32,
        op: MulDivOp,
        span: Span,
    },
}

impl ResolvedDimTerm {
    /// Get the combining operator for this term.
    #[must_use]
    pub const fn op(&self) -> MulDivOp {
        match self {
            Self::Concrete { op, .. } | Self::GenericParam { op, .. } => *op,
        }
    }
}

/// A resolved index in an indexed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIndex {
    /// A concrete index name, e.g. `Maneuver`
    Concrete(IndexName, Span),
    /// A generic index parameter, e.g. `I`
    GenericParam(GenericParamName, Span),
}

// ---------------------------------------------------------------------------
// Resolved function signature
// ---------------------------------------------------------------------------

/// A resolved function signature (with generic placeholders).
#[derive(Debug, Clone)]
pub struct ResolvedFnSig {
    pub generic_dim_params: Vec<GenericParamName>,
    pub generic_index_params: Vec<GenericParamName>,
    pub params: Vec<ResolvedFnParam>,
    pub return_type: ResolvedTypeExpr,
}

/// A resolved function parameter.
#[derive(Debug, Clone)]
pub struct ResolvedFnParam {
    pub name: String,
    pub resolved_type: ResolvedTypeExpr,
}

// ---------------------------------------------------------------------------
// TIR struct
// ---------------------------------------------------------------------------

/// Typed Intermediate Representation — the result of [`type_resolve`].
///
/// Contains everything from [`IR`] plus resolved type annotations for
/// every declaration and function signature.
#[derive(Debug)]
pub struct TIR {
    /// The type/unit/dimension/index/struct/function registry.
    pub registry: Registry,
    /// Const declarations in source order: (name, `type_ann`, expr, span).
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    /// Param declarations in source order: (name, `type_ann`, expr, span).
    pub params: Vec<(String, TypeExpr, Expr, Span)>,
    /// Node declarations in source order: (name, `type_ann`, expr, span).
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
    /// For each param/node, the set of `@`-references (runtime deps).
    pub runtime_deps: HashMap<String, std::collections::HashSet<String>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<String, std::collections::HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Resolved type for each const/param/node declaration.
    pub resolved_decl_types: HashMap<String, ResolvedTypeExpr>,
    /// Resolved function signatures (with generic placeholders).
    pub resolved_fn_sigs: HashMap<FnName, ResolvedFnSig>,
}

impl TIR {
    /// Build a concrete `DeclaredType` map from resolved types.
    ///
    /// Converts each entry in `resolved_decl_types` via [`resolved_to_declared_type`]
    /// and adds builtin constants as `Dimensionless`.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphcalError`] if any resolved type contains unresolved generic
    /// parameters.
    pub fn build_declared_types(
        &self,
        src: &NamedSource<Arc<String>>,
    ) -> Result<HashMap<String, crate::dim_check::DeclaredType>, GraphcalError> {
        let mut declared_types = HashMap::new();
        for name in crate::builtins::builtin_constants().keys() {
            declared_types.insert(
                (*name).to_string(),
                crate::dim_check::DeclaredType::Scalar(Dimension::DIMENSIONLESS),
            );
        }
        for (name, resolved) in &self.resolved_decl_types {
            let dt = resolved_to_declared_type(resolved, src)?;
            declared_types.insert(name.clone(), dt);
        }
        Ok(declared_types)
    }
}

/// Resolve all type annotations in an [`IR`], producing a [`TIR`].
///
/// For each const/param/node, resolves the type annotation with no generic
/// params in scope. For each function, resolves parameter types and return
/// type with the function's own generic params in scope.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if any type annotation references an unknown
/// dimension, struct, or index.
pub fn type_resolve(ir: IR, src: &NamedSource<Arc<String>>) -> Result<TIR, GraphcalError> {
    let mut resolved_decl_types = HashMap::new();
    let mut resolved_fn_sigs = HashMap::new();

    let no_dim_params: &[GenericParamName] = &[];
    let no_index_params: &[GenericParamName] = &[];

    // Resolve type annotations for all consts/params/nodes (no generic params in scope).
    for (name, type_ann, _, _) in ir
        .consts
        .iter()
        .chain(ir.params.iter())
        .chain(ir.nodes.iter())
    {
        let resolved =
            resolve_type_expr(type_ann, &ir.registry, no_dim_params, no_index_params, src)?;
        resolved_decl_types.insert(name.clone(), resolved);
    }

    // Resolve function signatures from the registry (which has FnDef with TypeExpr).
    for fn_def in ir.registry.all_functions() {
        let dim_params: Vec<GenericParamName> = fn_def
            .generic_params
            .iter()
            .filter(|g| g.constraint == crate::registry::FnGenericConstraint::Dim)
            .map(|g| g.name.clone())
            .collect();
        let index_params: Vec<GenericParamName> = fn_def
            .generic_params
            .iter()
            .filter(|g| g.constraint == crate::registry::FnGenericConstraint::Index)
            .map(|g| g.name.clone())
            .collect();

        let mut resolved_params = Vec::with_capacity(fn_def.params.len());
        for p in &fn_def.params {
            let resolved =
                resolve_type_expr(&p.type_expr, &ir.registry, &dim_params, &index_params, src)?;
            resolved_params.push(ResolvedFnParam {
                name: p.name.clone(),
                resolved_type: resolved,
            });
        }
        let return_type = resolve_type_expr(
            &fn_def.return_type_expr,
            &ir.registry,
            &dim_params,
            &index_params,
            src,
        )?;

        resolved_fn_sigs.insert(
            fn_def.name.clone(),
            ResolvedFnSig {
                generic_dim_params: dim_params,
                generic_index_params: index_params,
                params: resolved_params,
                return_type,
            },
        );
    }

    Ok(TIR {
        registry: ir.registry,
        consts: ir.consts,
        params: ir.params,
        nodes: ir.nodes,
        runtime_deps: ir.runtime_deps,
        const_deps: ir.const_deps,
        source_order: ir.source_order,
        functions: ir.functions,
        resolved_decl_types,
        resolved_fn_sigs,
    })
}

// ---------------------------------------------------------------------------
// Conversion to DeclaredType
// ---------------------------------------------------------------------------

/// Convert a non-generic [`ResolvedTypeExpr`] to a `DeclaredType`.
///
/// This is used by downstream stages (`dim_check`, `eval`) that work with concrete
/// types. Generic variants (`GenericDimParam`, `GenericDimExpr`, generic indexes)
/// cannot be converted and will return an error.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if the resolved type contains unresolved generic
/// parameters.
pub fn resolved_to_declared_type(
    resolved: &ResolvedTypeExpr,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::dim_check::DeclaredType, GraphcalError> {
    use crate::dim_check::DeclaredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(DeclaredType::Scalar(Dimension::DIMENSIONLESS)),
        ResolvedTypeExpr::Bool => Ok(DeclaredType::Bool),
        ResolvedTypeExpr::Int => Ok(DeclaredType::Int),
        ResolvedTypeExpr::Scalar(dim) => Ok(DeclaredType::Scalar(*dim)),
        ResolvedTypeExpr::Struct(name, _) => Ok(DeclaredType::Struct(name.clone())),
        ResolvedTypeExpr::GenericDimParam(name, span) => Err(GraphcalError::EvalError {
            message: format!("cannot use generic dimension parameter `{name}` as a concrete type"),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::GenericDimExpr { span, .. } => Err(GraphcalError::EvalError {
            message: "cannot use generic dimension expression as a concrete type".to_string(),
            src: src.clone(),
            span: (*span).into(),
        }),
        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = resolved_to_declared_type(base, src)?;
            for idx in indexes.iter().rev() {
                match idx {
                    ResolvedIndex::Concrete(name, _) => {
                        result = DeclaredType::Indexed {
                            element: Box::new(result),
                            index: name.clone(),
                        };
                    }
                    ResolvedIndex::GenericParam(name, span) => {
                        return Err(GraphcalError::EvalError {
                            message: format!(
                                "cannot use generic index parameter `{name}` as a concrete type"
                            ),
                            src: src.clone(),
                            span: (*span).into(),
                        });
                    }
                }
            }
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// Unification
// ---------------------------------------------------------------------------

/// Unify a resolved type expression against an actual inferred type,
/// binding generic dimension and index parameters.
///
/// For example, if `resolved` is `GenericDimParam("D")` and `actual` is
/// `Scalar(Length)`, binds `D = Length` in `dim_sub`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] on type mismatch or conflicting bindings.
#[expect(
    clippy::too_many_lines,
    reason = "complex generic unification requires many match arms"
)]
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
pub fn unify_resolved_type(
    resolved: &ResolvedTypeExpr,
    actual: &crate::dim_check::InferredType,
    dim_sub: &mut HashMap<GenericParamName, Dimension>,
    index_sub: &mut HashMap<GenericParamName, IndexName>,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<(), GraphcalError> {
    use crate::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Indexed { base, indexes } => {
            // Peel off index layers from actual type, binding index generics
            let mut current = actual;
            for idx in indexes.iter().rev() {
                let InferredType::Indexed {
                    element,
                    index: actual_idx,
                } = current
                else {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: "indexed type".to_string(),
                        found: format_inferred(current),
                        src: src.clone(),
                        span: span.into(),
                        help: "expected an indexed value".to_string(),
                    });
                };
                match idx {
                    ResolvedIndex::GenericParam(gp, _) => {
                        if let Some(prev) = index_sub.get(gp) {
                            if *prev != *actual_idx {
                                return Err(GraphcalError::IndexMismatch {
                                    expected: prev.clone(),
                                    found: actual_idx.clone(),
                                    src: src.clone(),
                                    span: span.into(),
                                });
                            }
                        } else {
                            index_sub.insert(gp.clone(), actual_idx.clone());
                        }
                    }
                    ResolvedIndex::Concrete(name, _) => {
                        if *name != *actual_idx {
                            return Err(GraphcalError::IndexMismatch {
                                expected: name.clone(),
                                found: actual_idx.clone(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                    }
                }
                current = element;
            }
            unify_resolved_type(base, current, dim_sub, index_sub, src, span)
        }

        ResolvedTypeExpr::Bool => {
            if *actual != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred(actual),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Bool argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Int => {
            if *actual != InferredType::Int {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Int".to_string(),
                    found: format_inferred(actual),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Int argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Dimensionless => {
            let actual_dim = expect_scalar_from_inferred(actual, src, span)?;
            if !actual_dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: format!("{actual_dim}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Dimensionless argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Scalar(expected_dim) => {
            let actual_dim = expect_scalar_from_inferred(actual, src, span)?;
            if *expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("{expected_dim}"),
                    found: format!("{actual_dim}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "dimension mismatch in function argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Struct(name, _) => {
            if *actual != InferredType::Struct(name.clone()) {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.to_string(),
                    found: format_inferred(actual),
                    src: src.clone(),
                    span: span.into(),
                    help: format!("expected struct type `{name}`"),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimParam(gp, _) => {
            let actual_dim = expect_scalar_from_inferred(actual, src, span)?;
            if let Some(prev) = dim_sub.get(gp) {
                if *prev != actual_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: format!("{prev}"),
                        found: format!("{actual_dim}"),
                        src: src.clone(),
                        span: span.into(),
                        help: format!(
                            "generic `{gp}` was bound to {prev} but this argument requires {actual_dim}"
                        ),
                    });
                }
            } else {
                dim_sub.insert(gp.clone(), actual_dim);
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            let actual_dim = expect_scalar_from_inferred(actual, src, span)?;

            // Single generic term with power: D^n means D = actual^(1/n)
            if terms.len() == 1
                && let ResolvedDimTerm::GenericParam {
                    name: gp,
                    power,
                    op: MulDivOp::Mul,
                    ..
                } = &terms[0]
            {
                let bound_dim = if *power == 1 {
                    actual_dim
                } else {
                    actual_dim.pow(Rational::new(1, *power))
                };
                if let Some(prev) = dim_sub.get(gp) {
                    if *prev != bound_dim {
                        return Err(GraphcalError::DimensionMismatch {
                            expected: format!("{prev}"),
                            found: format!("{bound_dim}"),
                            src: src.clone(),
                            span: span.into(),
                            help: format!(
                                "generic `{gp}` was bound to {prev} but this argument requires {bound_dim}"
                            ),
                        });
                    }
                } else {
                    dim_sub.insert(gp.clone(), bound_dim);
                }
                return Ok(());
            }

            // General case: compute expected dimension from already-bound generics + concrete terms
            let mut expected_dim = Dimension::DIMENSIONLESS;
            for term in terms {
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => {
                        dim.pow(Rational::from_int(*power))
                    }
                    ResolvedDimTerm::GenericParam {
                        name: gp, power, ..
                    } => {
                        if let Some(prev) = dim_sub.get(gp) {
                            prev.pow(Rational::from_int(*power))
                        } else {
                            return Err(GraphcalError::DimensionMismatch {
                                expected: format!("generic `{gp}` (unresolved)"),
                                found: format!("{actual_dim}"),
                                src: src.clone(),
                                span: span.into(),
                                help: format!(
                                    "generic `{gp}` could not be inferred from this argument"
                                ),
                            });
                        }
                    }
                };
                expected_dim = match term.op() {
                    MulDivOp::Mul => expected_dim * term_dim,
                    MulDivOp::Div => expected_dim / term_dim,
                };
            }

            if expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("{expected_dim}"),
                    found: format!("{actual_dim}"),
                    src: src.clone(),
                    span: span.into(),
                    help: "dimension mismatch in function argument".to_string(),
                });
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// Substitute generic parameters in a resolved type, producing an `InferredType`.
///
/// This replaces `resolve_type_with_substitution()` from `dim_check.rs`.
#[expect(
    clippy::implicit_hasher,
    reason = "always called with standard HashMap"
)]
pub fn substitute_resolved_type(
    resolved: &ResolvedTypeExpr,
    dim_sub: &HashMap<GenericParamName, Dimension>,
    index_sub: &HashMap<GenericParamName, IndexName>,
    src: &NamedSource<Arc<String>>,
) -> Result<crate::dim_check::InferredType, GraphcalError> {
    use crate::dim_check::InferredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(InferredType::Scalar(Dimension::DIMENSIONLESS)),
        ResolvedTypeExpr::Bool => Ok(InferredType::Bool),
        ResolvedTypeExpr::Int => Ok(InferredType::Int),
        ResolvedTypeExpr::Scalar(dim) => Ok(InferredType::Scalar(*dim)),
        ResolvedTypeExpr::Struct(name, _) => Ok(InferredType::Struct(name.clone())),

        ResolvedTypeExpr::GenericDimParam(gp, span) => dim_sub.get(gp).map_or_else(
            || {
                Err(GraphcalError::EvalError {
                    message: format!("generic `{gp}` not bound during substitution"),
                    src: src.clone(),
                    span: (*span).into(),
                })
            },
            |dim| Ok(InferredType::Scalar(*dim)),
        ),

        ResolvedTypeExpr::GenericDimExpr { terms, span: _ } => {
            let mut result = Dimension::DIMENSIONLESS;
            for term in terms {
                let term_dim = match term {
                    ResolvedDimTerm::Concrete { dim, power, .. } => {
                        dim.pow(Rational::from_int(*power))
                    }
                    ResolvedDimTerm::GenericParam {
                        name: gp,
                        power,
                        span: term_span,
                        ..
                    } => {
                        let base = dim_sub.get(gp).ok_or_else(|| GraphcalError::EvalError {
                            message: format!("generic `{gp}` not bound during substitution"),
                            src: src.clone(),
                            span: (*term_span).into(),
                        })?;
                        base.pow(Rational::from_int(*power))
                    }
                };
                result = match term.op() {
                    MulDivOp::Mul => result * term_dim,
                    MulDivOp::Div => result / term_dim,
                };
            }
            Ok(InferredType::Scalar(result))
        }

        ResolvedTypeExpr::Indexed { base, indexes } => {
            let mut result = substitute_resolved_type(base, dim_sub, index_sub, src)?;
            for idx in indexes.iter().rev() {
                let resolved_idx = match idx {
                    ResolvedIndex::Concrete(name, _) => name.clone(),
                    ResolvedIndex::GenericParam(gp, span) => index_sub
                        .get(gp)
                        .cloned()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!("generic index `{gp}` not bound during substitution"),
                            src: src.clone(),
                            span: (*span).into(),
                        })?,
                };
                result = InferredType::Indexed {
                    element: Box::new(result),
                    index: resolved_idx,
                };
            }
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract scalar dimension from an `InferredType`.
fn expect_scalar_from_inferred(
    inferred: &crate::dim_check::InferredType,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        crate::dim_check::InferredType::Scalar(d) => Ok(*d),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: format_inferred(other),
            src: src.clone(),
            span: span.into(),
            help: "expected a scalar value, not a struct or indexed type".to_string(),
        }),
    }
}

/// Format an inferred type for diagnostics.
fn format_inferred(it: &crate::dim_check::InferredType) -> String {
    use crate::dim_check::InferredType;
    match it {
        InferredType::Scalar(d) => format!("{d}"),
        InferredType::Bool => "Bool".to_string(),
        InferredType::Int => "Int".to_string(),
        InferredType::Struct(name) => name.to_string(),
        InferredType::Indexed { element, index } => {
            format!("{}[{index}]", format_inferred(element))
        }
        InferredType::LoopVar(idx) => format!("<loop var: {idx}>"),
    }
}

// ---------------------------------------------------------------------------
// Type resolution (single TypeExpr)
// ---------------------------------------------------------------------------

/// Resolve a `TypeExpr` into a `ResolvedTypeExpr`.
///
/// `dim_params` and `index_params` are the generic parameters in scope (empty
/// for top-level declarations, non-empty inside function signatures).
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a name cannot be resolved (not a known
/// dimension, struct, index, or in-scope generic parameter).
pub fn resolve_type_expr(
    type_ann: &TypeExpr,
    registry: &Registry,
    dim_params: &[GenericParamName],
    index_params: &[GenericParamName],
    src: &NamedSource<Arc<String>>,
) -> Result<ResolvedTypeExpr, GraphcalError> {
    match &type_ann.kind {
        TypeExprKind::Dimensionless => Ok(ResolvedTypeExpr::Dimensionless),
        TypeExprKind::Bool => Ok(ResolvedTypeExpr::Bool),
        TypeExprKind::Int => Ok(ResolvedTypeExpr::Int),

        TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_type_expr(base, registry, dim_params, index_params, src)?;
            let mut resolved_indexes = Vec::with_capacity(indexes.len());
            for idx in indexes {
                let idx_name = &idx.name;
                if let Some(gp) = index_params.iter().find(|p| p.as_str() == idx_name) {
                    resolved_indexes.push(ResolvedIndex::GenericParam(gp.clone(), idx.span));
                } else if registry.get_index(idx_name).is_some() {
                    resolved_indexes
                        .push(ResolvedIndex::Concrete(IndexName::new(idx_name), idx.span));
                } else {
                    return Err(GraphcalError::UnknownIndex {
                        name: idx.as_index_name(),
                        src: src.clone(),
                        span: idx.span.into(),
                    });
                }
            }
            Ok(ResolvedTypeExpr::Indexed {
                base: Box::new(resolved_base),
                indexes: resolved_indexes,
            })
        }

        TypeExprKind::DimExpr(dim_expr) => {
            // Single-term, no power: could be struct, generic dim param, or dimension
            if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() {
                let name = &dim_expr.terms[0].term.name.name;
                let span = dim_expr.terms[0].term.span;

                // Check type (struct sugar or tagged union) first
                if registry.get_type(name).is_some() {
                    return Ok(ResolvedTypeExpr::Struct(StructTypeName::new(name), span));
                }

                // Check generic dim param
                if let Some(gp) = dim_params.iter().find(|p| p.as_str() == name) {
                    return Ok(ResolvedTypeExpr::GenericDimParam(gp.clone(), span));
                }
            }

            // Check if any term is a generic dim param
            let has_generic = dim_expr
                .terms
                .iter()
                .any(|item| dim_params.iter().any(|p| p.as_str() == item.term.name.name));

            if has_generic {
                // Build GenericDimExpr with mixed concrete/generic terms
                let mut terms = Vec::with_capacity(dim_expr.terms.len());
                for item in &dim_expr.terms {
                    let name = &item.term.name.name;
                    let power = item.term.power.unwrap_or(1);
                    let op = item.op;

                    if let Some(gp) = dim_params.iter().find(|p| p.as_str() == name) {
                        terms.push(ResolvedDimTerm::GenericParam {
                            name: gp.clone(),
                            power,
                            op,
                            span: item.term.span,
                        });
                    } else if let Some(dim) = registry.get_dimension(name) {
                        terms.push(ResolvedDimTerm::Concrete {
                            dim: *dim,
                            power,
                            op,
                        });
                    } else {
                        return Err(GraphcalError::UnknownDimension {
                            name: DimName::new(name),
                            src: src.clone(),
                            span: item.term.span.into(),
                        });
                    }
                }
                Ok(ResolvedTypeExpr::GenericDimExpr {
                    terms,
                    span: dim_expr.span,
                })
            } else {
                // All terms are concrete dimensions — resolve to Scalar
                let mut result = Dimension::DIMENSIONLESS;
                for item in &dim_expr.terms {
                    let name = &item.term.name.name;
                    let base = registry.get_dimension(name).ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: DimName::new(name),
                            src: src.clone(),
                            span: item.term.span.into(),
                        }
                    })?;
                    let exp = item.term.power.unwrap_or(1);
                    let powered = base.pow(Rational::from_int(exp));
                    result = match item.op {
                        MulDivOp::Mul => result * powered,
                        MulDivOp::Div => result / powered,
                    };
                }
                Ok(ResolvedTypeExpr::Scalar(result))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;
    use crate::prelude::load_prelude;
    use graphcal_syntax::dimension::BaseDim;
    use graphcal_syntax::parser::Parser;

    fn make_registry() -> Registry {
        let mut r = Registry::new();
        load_prelude(&mut r);
        r
    }

    fn make_registry_with_struct() -> Registry {
        let mut r = make_registry();
        let velocity_dim = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        r.register_type(crate::registry::TypeDef {
            name: StructTypeName::new("TransferResult"),
            variants: vec![crate::registry::VariantDef {
                name: graphcal_syntax::names::VariantName::new("TransferResult"),
                fields: vec![
                    crate::registry::StructField {
                        name: graphcal_syntax::names::FieldName::new("dv1"),
                        dimension: velocity_dim,
                    },
                    crate::registry::StructField {
                        name: graphcal_syntax::names::FieldName::new("dv2"),
                        dimension: velocity_dim,
                    },
                ],
            }],
        });
        r
    }

    fn make_registry_with_index() -> Registry {
        let mut r = make_registry();
        r.register_index(crate::registry::IndexDef {
            name: IndexName::new("Maneuver"),
            variants: vec![
                graphcal_syntax::names::VariantName::new("Departure"),
                graphcal_syntax::names::VariantName::new("Insertion"),
            ],
        });
        r
    }

    fn make_src() -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(String::new()))
    }

    /// Parse a type annotation from a param declaration and return the `TypeExpr`.
    fn parse_type(source: &str) -> TypeExpr {
        // Wrap in a param declaration so the parser can handle it
        let full = format!("param x: {source} = 0.0;");
        let file = Parser::new(&full).parse_file().unwrap();
        match &file.declarations[0].kind {
            graphcal_syntax::ast::DeclKind::Param(p) => p.type_ann.clone(),
            _ => panic!("expected param"),
        }
    }

    #[test]
    fn resolve_dimensionless() {
        let r = make_registry();
        let te = parse_type("Dimensionless");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Dimensionless);
    }

    #[test]
    fn resolve_bool() {
        let r = make_registry();
        let te = parse_type("Bool");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Bool);
    }

    #[test]
    fn resolve_int() {
        let r = make_registry();
        let te = parse_type("Int");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Int);
    }

    #[test]
    fn resolve_concrete_dimension() {
        let r = make_registry();
        let te = parse_type("Length");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(
            resolved,
            ResolvedTypeExpr::Scalar(Dimension::base(BaseDim::Length))
        );
    }

    #[test]
    fn resolve_compound_dimension() {
        let r = make_registry();
        let te = parse_type("Length / Time^2");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        let expected = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time).pow_int(2);
        assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
    }

    #[test]
    fn resolve_struct_type() {
        let r = make_registry_with_struct();
        let te = parse_type("TransferResult");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert!(
            matches!(resolved, ResolvedTypeExpr::Struct(name, _) if name.as_str() == "TransferResult")
        );
    }

    #[test]
    fn resolve_generic_dim_param() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let te = parse_type("D");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &make_src()).unwrap();
        assert!(
            matches!(resolved, ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D")
        );
    }

    #[test]
    fn resolve_generic_dim_expr_with_power() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let te = parse_type("D^2");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
                assert_eq!(terms.len(), 1);
                match &terms[0] {
                    ResolvedDimTerm::GenericParam { name, power, .. } => {
                        assert_eq!(name.as_str(), "D");
                        assert_eq!(*power, 2);
                    }
                    ResolvedDimTerm::Concrete { .. } => panic!("expected GenericParam term"),
                }
            }
            _ => panic!("expected GenericDimExpr"),
        }
    }

    #[test]
    fn resolve_mixed_generic_concrete() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        // D * Length  — this is a DimExpr with a generic and a concrete term
        let te = parse_type("D * Length");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
                assert_eq!(terms.len(), 2);
                assert!(
                    matches!(&terms[0], ResolvedDimTerm::GenericParam { name, .. } if name.as_str() == "D")
                );
                assert!(matches!(&terms[1], ResolvedDimTerm::Concrete { .. }));
            }
            _ => panic!("expected GenericDimExpr, got {resolved:?}"),
        }
    }

    #[test]
    fn resolve_concrete_indexed() {
        let r = make_registry_with_index();
        let te = parse_type("Length[Maneuver]");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::Indexed { base, indexes } => {
                assert_eq!(
                    *base,
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDim::Length))
                );
                assert_eq!(indexes.len(), 1);
                assert!(
                    matches!(&indexes[0], ResolvedIndex::Concrete(name, _) if name.as_str() == "Maneuver")
                );
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn resolve_generic_indexed() {
        let r = make_registry();
        let dim_params = vec![GenericParamName::new("D")];
        let index_params = vec![GenericParamName::new("I")];
        let te = parse_type("D[I]");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &index_params, &make_src()).unwrap();
        match resolved {
            ResolvedTypeExpr::Indexed { base, indexes } => {
                assert!(
                    matches!(*base, ResolvedTypeExpr::GenericDimParam(ref name, _) if name.as_str() == "D")
                );
                assert_eq!(indexes.len(), 1);
                assert!(
                    matches!(&indexes[0], ResolvedIndex::GenericParam(name, _) if name.as_str() == "I")
                );
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn resolve_unknown_dimension_error() {
        let r = make_registry();
        let te = parse_type("UnknownDim");
        let err = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownDimension { .. }));
    }

    #[test]
    fn resolve_unknown_index_error() {
        let r = make_registry();
        let te = parse_type("Length[UnknownIdx]");
        let err = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::UnknownIndex { .. }));
    }

    #[test]
    fn resolve_struct_takes_priority_over_dim_param() {
        // When a name matches both a struct and a generic param,
        // struct should win (structs are concrete, params are only
        // in scope inside a function that has that param).
        // In practice this shouldn't happen because struct names are
        // PascalCase and generic params are single letters, but let's
        // make sure the priority is correct.
        let r = make_registry_with_struct();
        let dim_params = vec![GenericParamName::new("TransferResult")];
        let te = parse_type("TransferResult");
        let resolved = resolve_type_expr(&te, &r, &dim_params, &[], &make_src()).unwrap();
        assert!(matches!(resolved, ResolvedTypeExpr::Struct(..)));
    }

    #[test]
    fn resolve_velocity_derived_dimension() {
        let r = make_registry();
        let te = parse_type("Velocity");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        let expected = Dimension::base(BaseDim::Length) / Dimension::base(BaseDim::Time);
        assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
    }

    // --- type_resolve() integration tests ---

    fn parse_and_type_resolve(source: &str) -> Result<TIR, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = NamedSource::new("test", Arc::new(source.to_string()));
        let ir = crate::ir::lower(&file, &src)?;
        type_resolve(ir, &src)
    }

    #[test]
    fn type_resolve_rocket() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // All declarations should have resolved types
        assert!(tir.resolved_decl_types.contains_key("dry_mass"));
        assert!(tir.resolved_decl_types.contains_key("delta_v"));
        assert!(tir.resolved_decl_types.contains_key("G0"));
    }

    #[test]
    fn type_resolve_functions() {
        let source = include_str!("../../../tests/fixtures/functions.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // Functions should have resolved signatures
        assert!(tir.resolved_fn_sigs.contains_key("orbital_velocity"));
        let lerp_sig = &tir.resolved_fn_sigs[&FnName::new("lerp")];
        // lerp<D: Dim> has one generic dim param
        assert_eq!(lerp_sig.generic_dim_params.len(), 1);
        assert_eq!(lerp_sig.generic_dim_params[0].as_str(), "D");
        // lerp params: a: D, b: D, t: Dimensionless
        assert_eq!(lerp_sig.params.len(), 3);
        assert!(matches!(
            &lerp_sig.params[0].resolved_type,
            ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D"
        ));
        assert_eq!(
            lerp_sig.params[2].resolved_type,
            ResolvedTypeExpr::Dimensionless
        );
        // return type: D
        assert!(matches!(
            &lerp_sig.return_type,
            ResolvedTypeExpr::GenericDimParam(name, _) if name.as_str() == "D"
        ));
    }

    #[test]
    fn type_resolve_indexed() {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // delta_v should be Velocity[Maneuver]
        let dv_type = &tir.resolved_decl_types["delta_v"];
        assert!(matches!(dv_type, ResolvedTypeExpr::Indexed { .. }));
        // total generic fn should have resolved sig
        let total_sig = &tir.resolved_fn_sigs[&FnName::new("total")];
        assert_eq!(total_sig.generic_dim_params.len(), 1);
        assert_eq!(total_sig.generic_index_params.len(), 1);
    }

    #[test]
    fn type_resolve_hohmann() {
        let source = include_str!("../../../tests/fixtures/hohmann.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // transfer should be a struct type
        let transfer_type = &tir.resolved_decl_types["transfer"];
        assert!(
            matches!(transfer_type, ResolvedTypeExpr::Struct(name, _) if name.as_str() == "TransferResult")
        );
    }

    // --- resolved_to_declared_type() tests ---

    use crate::dim_check::DeclaredType;

    #[test]
    fn convert_dimensionless() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Dimensionless, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(Dimension::DIMENSIONLESS));
    }

    #[test]
    fn convert_bool() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Bool, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Bool);
    }

    #[test]
    fn convert_int() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Int, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Int);
    }

    #[test]
    fn convert_scalar() {
        let dim = Dimension::base(BaseDim::Length);
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Scalar(dim), &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(dim));
    }

    #[test]
    fn convert_struct() {
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Struct(StructTypeName::new("Foo"), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap();
        assert_eq!(dt, DeclaredType::Struct(StructTypeName::new("Foo")));
    }

    #[test]
    fn convert_indexed() {
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Scalar(Dimension::base(BaseDim::Length))),
                indexes: vec![ResolvedIndex::Concrete(
                    IndexName::new("M"),
                    Span::new(0, 0),
                )],
            },
            &make_src(),
        )
        .unwrap();
        assert_eq!(
            dt,
            DeclaredType::Indexed {
                element: Box::new(DeclaredType::Scalar(Dimension::base(BaseDim::Length))),
                index: IndexName::new("M"),
            }
        );
    }

    #[test]
    fn convert_generic_dim_param_fails() {
        let err = resolved_to_declared_type(
            &ResolvedTypeExpr::GenericDimParam(GenericParamName::new("D"), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }

    #[test]
    fn convert_generic_index_fails() {
        let err = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Dimensionless),
                indexes: vec![ResolvedIndex::GenericParam(
                    GenericParamName::new("I"),
                    Span::new(0, 0),
                )],
            },
            &make_src(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }
}

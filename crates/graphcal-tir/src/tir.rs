//! Typed Intermediate Representation (TIR) — type annotations resolved to semantic types.
//!
//! The TIR layer resolves ambiguous AST names (`Ident` in `DimTerm::name` and
//! `TypeExprKind::Indexed::indexes`) into concrete dimensions, struct types,
//! generic dimension parameters, or generic index parameters.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{
    AssertBody, Expr, FigureDecl, FnDecl, MulDivOp, PlotDecl, TypeExpr, TypeExprKind,
};
use graphcal_syntax::dimension::{Dimension, Rational};
use graphcal_syntax::names::{DimName, FnName, GenericParamName, IndexName, StructTypeName};
use graphcal_syntax::span::Span;

use graphcal_ir::ir::IR;
use graphcal_ir::resolve::{DeclCategory, ExpectedFail};
use graphcal_registry::error::GraphcalError;
use graphcal_registry::registry::Registry;
use graphcal_registry::time_scale::TimeScale;

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
    /// A datetime instant in a specific time scale (e.g., `Datetime` = UTC, `Datetime<TT>`).
    Datetime(TimeScale),
    /// A label of a named index (e.g., `Maneuver` in `m: Maneuver`).
    Label(IndexName, Span),
    /// A concrete scalar dimension, e.g. `Length * Time^-2`
    Scalar(Dimension),
    /// A non-generic struct type name, e.g. `TransferResult`
    Struct(StructTypeName, Span),
    /// A generic struct with concrete type arguments, e.g. `Vec3<Length, ECI>`
    GenericStruct {
        name: StructTypeName,
        type_args: Vec<Self>,
        span: Span,
    },
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

impl ResolvedTypeExpr {
    /// Format as a human-readable string, e.g. `"Length / Time^2"`, `"Bool"`, `"Vec3<Length, ECI>"`.
    #[must_use]
    pub fn format(&self, registry: &Registry) -> String {
        match self {
            Self::Dimensionless => "Dimensionless".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::Int => "Int".to_string(),
            Self::Datetime(scale) => {
                if scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{scale}>")
                }
            }
            Self::Label(index, _) => format!("Label({index})"),
            Self::Scalar(dim) => {
                let formatted = registry.dimensions.format_dimension(dim);
                if formatted.is_empty() {
                    "Dimensionless".to_string()
                } else {
                    formatted
                }
            }
            Self::Struct(name, _) => name.to_string(),
            Self::GenericStruct {
                name, type_args, ..
            } => {
                let args: Vec<String> = type_args.iter().map(|a| a.format(registry)).collect();
                format!("{}<{}>", name, args.join(", "))
            }
            Self::GenericDimParam(name, _) => name.to_string(),
            Self::GenericDimExpr { terms, .. } => {
                let parts: Vec<String> = terms.iter().map(|t| t.format(registry)).collect();
                parts.join(" ")
            }
            Self::Indexed { base, indexes } => {
                let base_str = base.format(registry);
                let idx_strs: Vec<String> = indexes
                    .iter()
                    .map(|i| match i {
                        ResolvedIndex::Concrete(name, _) => name.to_string(),
                        ResolvedIndex::GenericParam(name, _) => name.to_string(),
                    })
                    .collect();
                format!("{base_str}[{}]", idx_strs.join(", "))
            }
        }
    }
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

    /// Format this term as a human-readable string, e.g. `"Length"`, `"/ Time^2"`, `"D^2"`.
    #[must_use]
    pub fn format(&self, registry: &Registry) -> String {
        let (name, power, op) = match self {
            Self::Concrete { dim, power, op } => {
                (registry.dimensions.format_dimension(dim), *power, *op)
            }
            Self::GenericParam {
                name, power, op, ..
            } => (name.to_string(), *power, *op),
        };
        let prefix = match op {
            MulDivOp::Mul => "",
            MulDivOp::Div => "/ ",
        };
        if power == 1 {
            format!("{prefix}{name}")
        } else {
            format!("{prefix}{name}^{power}")
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
// Resolved domain constraints
// ---------------------------------------------------------------------------

/// A resolved domain constraint with evaluated SI-unit bounds.
///
/// Produced during `type_resolve()` by evaluating the bound expressions
/// in `DomainBound` to concrete f64 values (in SI units).
#[derive(Debug, Clone)]
pub struct ResolvedDomainConstraint {
    /// Minimum bound in SI units, or `None` if no `min:` was specified.
    pub min: Option<f64>,
    /// Maximum bound in SI units, or `None` if no `max:` was specified.
    pub max: Option<f64>,
    /// Original min expression text for diagnostics (e.g., `"100 kg"`).
    pub min_display: Option<String>,
    /// Original max expression text for diagnostics (e.g., `"2000 kg"`).
    pub max_display: Option<String>,
    /// Span covering the entire constraint clause for error reporting.
    pub span: Span,
}

// ---------------------------------------------------------------------------
// TIR struct
// ---------------------------------------------------------------------------

/// Typed Intermediate Representation — the result of [`type_resolve`].
///
/// Contains everything from `IR` plus resolved type annotations for
/// every declaration and function signature.
#[derive(Debug)]
pub struct TIR {
    /// The type/unit/dimension/index/struct/function registry.
    pub registry: Registry,
    /// Const declarations in source order: (name, `type_ann`, expr, span).
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    /// Param declarations in source order: (name, `type_ann`, optional default expr, span).
    pub params: Vec<(String, TypeExpr, Option<Expr>, Span)>,
    /// Node declarations in source order: (name, `type_ann`, expr, span).
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
    /// Assert declarations in source order: (name, body, span).
    pub asserts: Vec<(String, AssertBody, Span)>,
    /// Plot declarations in source order: (name, decl, span, hidden).
    pub plots: Vec<(String, PlotDecl, Span, bool)>,
    /// Figure declarations in source order: (name, decl, span).
    pub figures: Vec<(String, FigureDecl, Span)>,
    /// For each param/node, the set of `@`-references (runtime deps).
    pub runtime_deps: HashMap<String, std::collections::HashSet<String>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<String, std::collections::HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Set of all assert names.
    pub assert_names: std::collections::HashSet<String>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<String, Vec<String>>,
    /// Mapping from assert name to its expected-fail configuration.
    pub expected_fail: HashMap<String, ExpectedFail>,
    /// Resolved type for each const/param/node declaration.
    pub resolved_decl_types: HashMap<String, ResolvedTypeExpr>,
    /// Resolved function signatures (with generic placeholders).
    pub resolved_fn_sigs: HashMap<FnName, ResolvedFnSig>,
    /// Resolved domain constraints for declarations that have them.
    pub domain_constraints: HashMap<String, ResolvedDomainConstraint>,
    /// Pre-evaluated values imported from dependency files (passed through from IR).
    /// Each entry carries the runtime value and its declared type (for `dim_check`).
    pub imported_values: HashMap<
        graphcal_ir::resolve::ScopedName,
        (
            graphcal_registry::runtime_value::RuntimeValue,
            graphcal_registry::declared_type::DeclaredType,
        ),
    >,
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
    ) -> Result<HashMap<String, graphcal_registry::declared_type::DeclaredType>, GraphcalError>
    {
        let mut declared_types = HashMap::new();
        for name in graphcal_registry::builtins::builtin_constants().keys() {
            declared_types.insert(
                (*name).to_string(),
                graphcal_registry::declared_type::DeclaredType::Scalar(Dimension::dimensionless()),
            );
        }
        for (name, resolved) in &self.resolved_decl_types {
            let dt = resolved_to_declared_type(resolved, src)?;
            declared_types.insert(name.clone(), dt);
        }
        // Include imported values' declared types so dim_check can resolve references.
        // ScopedName → String: dim_check uses flat string keys.
        for (name, (_rv, dt)) in &self.imported_values {
            declared_types.insert(name.to_string(), dt.clone());
        }
        Ok(declared_types)
    }
}

/// Resolve all type annotations in an `IR`, producing a [`TIR`].
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
    for (name, type_ann) in ir
        .consts
        .iter()
        .map(|(n, t, _, _)| (n, t))
        .chain(ir.params.iter().map(|(n, t, _, _)| (n, t)))
        .chain(ir.nodes.iter().map(|(n, t, _, _)| (n, t)))
    {
        let resolved =
            resolve_type_expr(type_ann, &ir.registry, no_dim_params, no_index_params, src)?;
        resolved_decl_types.insert(name.clone(), resolved);
    }

    // Resolve function signatures from the registry (which has FnDef with TypeExpr).
    for fn_def in ir.registry.functions.all_functions() {
        let dim_params: Vec<GenericParamName> = fn_def
            .generic_params
            .iter()
            .filter(|g| g.constraint == graphcal_registry::registry::FnGenericConstraint::Dim)
            .map(|g| g.name.clone())
            .collect();
        let index_params: Vec<GenericParamName> = fn_def
            .generic_params
            .iter()
            .filter(|g| g.constraint == graphcal_registry::registry::FnGenericConstraint::Index)
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
        asserts: ir.asserts,
        plots: ir.plots,
        figures: ir.figures,
        runtime_deps: ir.runtime_deps,
        const_deps: ir.const_deps,
        source_order: ir.source_order,
        functions: ir.functions,
        assert_names: ir.assert_names,
        assumes_map: ir.assumes_map,
        expected_fail: ir.expected_fail,
        resolved_decl_types,
        resolved_fn_sigs,
        domain_constraints: HashMap::new(), // Resolved later in compile()
        imported_values: ir.imported_values,
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
) -> Result<graphcal_registry::declared_type::DeclaredType, GraphcalError> {
    use graphcal_registry::declared_type::DeclaredType;

    match resolved {
        ResolvedTypeExpr::Dimensionless => Ok(DeclaredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(DeclaredType::Bool),
        ResolvedTypeExpr::Int => Ok(DeclaredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(DeclaredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(DeclaredType::Label(index.clone())),
        ResolvedTypeExpr::Scalar(dim) => Ok(DeclaredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(DeclaredType::Struct(name.clone(), vec![])),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut declared_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                declared_args.push(resolved_to_declared_type(arg, src)?);
            }
            Ok(DeclaredType::Struct(name.clone(), declared_args))
        }
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
    registry: &Registry,
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
                        found: format_inferred(current, registry),
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
            unify_resolved_type(base, current, dim_sub, index_sub, registry, src, span)
        }

        ResolvedTypeExpr::Bool => {
            if *actual != InferredType::Bool {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Bool".to_string(),
                    found: format_inferred(actual, registry),
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
                    found: format_inferred(actual, registry),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Int argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Datetime(expected_scale) => {
            if *actual != InferredType::Datetime(*expected_scale) {
                let expected_str = if expected_scale.is_utc() {
                    "Datetime".to_string()
                } else {
                    format!("Datetime<{expected_scale}>")
                };
                return Err(GraphcalError::DimensionMismatch {
                    expected: expected_str,
                    found: format_inferred(actual, registry),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Datetime argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Label(expected_index, _) => {
            let InferredType::Label(actual_index) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: format!("Label({expected_index})"),
                    found: format_inferred(actual, registry),
                    src: src.clone(),
                    span: span.into(),
                    help: format!("expected a label of index `{expected_index}`"),
                });
            };
            if *expected_index != *actual_index {
                return Err(GraphcalError::IndexMismatch {
                    expected: expected_index.clone(),
                    found: actual_index.clone(),
                    src: src.clone(),
                    span: span.into(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Dimensionless => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            if !actual_dim.is_dimensionless() {
                return Err(GraphcalError::DimensionMismatch {
                    expected: "Dimensionless".to_string(),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    src: src.clone(),
                    span: span.into(),
                    help: "expected Dimensionless argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::Scalar(expected_dim) => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            if *expected_dim != actual_dim {
                return Err(GraphcalError::DimensionMismatch {
                    expected: registry.dimensions.format_dimension(expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
                    src: src.clone(),
                    span: span.into(),
                    help: "dimension mismatch in function argument".to_string(),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericStruct { name, .. } | ResolvedTypeExpr::Struct(name, _) => {
            // For struct unification in function args, compare name only.
            // Type args matching is not needed here since function generics
            // don't use TypeApplication in their signatures (yet).
            let InferredType::Struct(actual_name, _) = actual else {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.to_string(),
                    found: format_inferred(actual, registry),
                    src: src.clone(),
                    span: span.into(),
                    help: format!("expected struct type `{name}`"),
                });
            };
            if *name != *actual_name {
                return Err(GraphcalError::DimensionMismatch {
                    expected: name.to_string(),
                    found: format_inferred(actual, registry),
                    src: src.clone(),
                    span: span.into(),
                    help: format!("expected struct type `{name}`"),
                });
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimParam(gp, _) => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;
            if let Some(prev) = dim_sub.get(gp) {
                if *prev != actual_dim {
                    return Err(GraphcalError::DimensionMismatch {
                        expected: registry.dimensions.format_dimension(prev),
                        found: registry.dimensions.format_dimension(&actual_dim),
                        src: src.clone(),
                        span: span.into(),
                        help: format!(
                            "generic `{gp}` was bound to {} but this argument requires {}",
                            registry.dimensions.format_dimension(prev),
                            registry.dimensions.format_dimension(&actual_dim),
                        ),
                    });
                }
            } else {
                dim_sub.insert(gp.clone(), actual_dim);
            }
            Ok(())
        }

        ResolvedTypeExpr::GenericDimExpr { terms, .. } => {
            let actual_dim = expect_scalar_from_inferred(actual, registry, src, span)?;

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
                            expected: registry.dimensions.format_dimension(prev),
                            found: registry.dimensions.format_dimension(&bound_dim),
                            src: src.clone(),
                            span: span.into(),
                            help: format!(
                                "generic `{gp}` was bound to {} but this argument requires {}",
                                registry.dimensions.format_dimension(prev),
                                registry.dimensions.format_dimension(&bound_dim),
                            ),
                        });
                    }
                } else {
                    dim_sub.insert(gp.clone(), bound_dim);
                }
                return Ok(());
            }

            // General case: compute expected dimension from already-bound generics + concrete terms
            let mut expected_dim = Dimension::dimensionless();
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
                                found: registry.dimensions.format_dimension(&actual_dim),
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
                    expected: registry.dimensions.format_dimension(&expected_dim),
                    found: registry.dimensions.format_dimension(&actual_dim),
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
        ResolvedTypeExpr::Dimensionless => Ok(InferredType::Scalar(Dimension::dimensionless())),
        ResolvedTypeExpr::Bool => Ok(InferredType::Bool),
        ResolvedTypeExpr::Int => Ok(InferredType::Int),
        ResolvedTypeExpr::Datetime(scale) => Ok(InferredType::Datetime(*scale)),
        ResolvedTypeExpr::Label(index, _) => Ok(InferredType::Label(index.clone())),
        ResolvedTypeExpr::Scalar(dim) => Ok(InferredType::Scalar(dim.clone())),
        ResolvedTypeExpr::Struct(name, _) => Ok(InferredType::Struct(name.clone(), vec![])),
        ResolvedTypeExpr::GenericStruct {
            name, type_args, ..
        } => {
            let mut inferred_args = Vec::with_capacity(type_args.len());
            for arg in type_args {
                inferred_args.push(substitute_resolved_type(arg, dim_sub, index_sub, src)?);
            }
            Ok(InferredType::Struct(name.clone(), inferred_args))
        }

        ResolvedTypeExpr::GenericDimParam(gp, span) => dim_sub.get(gp).map_or_else(
            || {
                Err(GraphcalError::EvalError {
                    message: format!("generic `{gp}` not bound during substitution"),
                    src: src.clone(),
                    span: (*span).into(),
                })
            },
            |dim| Ok(InferredType::Scalar(dim.clone())),
        ),

        ResolvedTypeExpr::GenericDimExpr { terms, span: _ } => {
            let mut result = Dimension::dimensionless();
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
    registry: &Registry,
    src: &NamedSource<Arc<String>>,
    span: Span,
) -> Result<Dimension, GraphcalError> {
    match inferred {
        crate::dim_check::InferredType::Scalar(d) => Ok(d.clone()),
        other => Err(GraphcalError::DimensionMismatch {
            expected: "scalar dimension".to_string(),
            found: format_inferred(other, registry),
            src: src.clone(),
            span: span.into(),
            help: "expected a scalar value, not a struct or indexed type".to_string(),
        }),
    }
}

/// Format an inferred type for diagnostics.
fn format_inferred(it: &crate::dim_check::InferredType, registry: &Registry) -> String {
    use crate::dim_check::InferredType;
    match it {
        InferredType::Scalar(d) => registry.dimensions.format_dimension(d),
        InferredType::Bool => "Bool".to_string(),
        InferredType::Int => "Int".to_string(),
        InferredType::Label(index) => format!("Label({index})"),
        InferredType::Struct(name, args) => {
            if args.is_empty() {
                name.to_string()
            } else {
                let args_str: Vec<String> =
                    args.iter().map(|a| format_inferred(a, registry)).collect();
                format!("{name}<{}>", args_str.join(", "))
            }
        }
        InferredType::Datetime(scale) => {
            if *scale == graphcal_registry::time_scale::TimeScale::UTC {
                "Datetime".to_string()
            } else {
                format!("Datetime<{scale}>")
            }
        }
        InferredType::Indexed { element, index } => {
            format!("{}[{index}]", format_inferred(element, registry))
        }
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
#[expect(
    clippy::too_many_lines,
    reason = "single match over all TypeExprKind variants including TypeApplication"
)]
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
        TypeExprKind::Datetime => Ok(ResolvedTypeExpr::Datetime(TimeScale::UTC)),

        TypeExprKind::Indexed { base, indexes } => {
            let resolved_base = resolve_type_expr(base, registry, dim_params, index_params, src)?;
            let mut resolved_indexes = Vec::with_capacity(indexes.len());
            for idx in indexes {
                let idx_name = &idx.name;
                if let Some(gp) = index_params.iter().find(|p| p.as_str() == idx_name) {
                    resolved_indexes.push(ResolvedIndex::GenericParam(gp.clone(), idx.span));
                } else if registry.indexes.get_index(idx_name).is_some() {
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

                // Check named index → Label type
                if let Some(idx_def) = registry.indexes.get_index(name)
                    && matches!(
                        idx_def.kind,
                        graphcal_registry::registry::IndexKind::Named { .. }
                    )
                {
                    return Ok(ResolvedTypeExpr::Label(IndexName::new(name), span));
                }

                // Check type (struct sugar or tagged union) first
                if registry.types.get_type(name).is_some() {
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
                    } else if let Some(dim) = registry.dimensions.get_dimension(name) {
                        terms.push(ResolvedDimTerm::Concrete {
                            dim: dim.clone(),
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
                let mut result = Dimension::dimensionless();
                for item in &dim_expr.terms {
                    let name = &item.term.name.name;
                    let base = registry.dimensions.get_dimension(name).ok_or_else(|| {
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

        TypeExprKind::TypeApplication { name, type_args } => {
            let type_name = &name.name;

            // Special case: Datetime<TimeScale>
            if type_name == "Datetime" {
                if type_args.len() != 1 {
                    return Err(GraphcalError::EvalError {
                        message: format!(
                            "type `Datetime` expects 0 or 1 type argument(s), got {}",
                            type_args.len()
                        ),
                        src: src.clone(),
                        span: type_ann.span.into(),
                    });
                }
                // The type arg should be a bare identifier naming a time scale
                let arg = &type_args[0];
                let scale_name = match &arg.kind {
                    TypeExprKind::DimExpr(dim_expr)
                        if dim_expr.terms.len() == 1 && dim_expr.terms[0].term.power.is_none() =>
                    {
                        &dim_expr.terms[0].term.name.name
                    }
                    _ => {
                        return Err(GraphcalError::EvalError {
                            message: "expected a time scale name (e.g., UTC, TAI, TT, TDB, GPST)"
                                .to_string(),
                            src: src.clone(),
                            span: arg.span.into(),
                        });
                    }
                };
                let scale: TimeScale =
                    scale_name.parse().map_err(|_| GraphcalError::EvalError {
                        message: format!(
                            "unknown time scale `{scale_name}`; \
                                 expected one of: UTC, TAI, TT, TDB, ET, GPST, GST, BDT"
                        ),
                        src: src.clone(),
                        span: arg.span.into(),
                    })?;
                return Ok(ResolvedTypeExpr::Datetime(scale));
            }

            // Verify this is a known generic type
            let type_def = registry.types.get_type(type_name).ok_or_else(|| {
                GraphcalError::UnknownStructType {
                    name: StructTypeName::new(type_name),
                    src: src.clone(),
                    span: name.span.into(),
                }
            })?;
            let total_params = type_def.generic_params.len();
            // Count required params (those without defaults)
            let required_count = type_def
                .generic_params
                .iter()
                .take_while(|p| p.default.is_none())
                .count();
            if type_args.len() < required_count || type_args.len() > total_params {
                let hint = if required_count == total_params {
                    format!("{total_params}")
                } else {
                    format!("{required_count}..{total_params}")
                };
                return Err(GraphcalError::EvalError {
                    message: format!(
                        "type `{type_name}` expects {hint} type argument(s), got {}",
                        type_args.len()
                    ),
                    src: src.clone(),
                    span: type_ann.span.into(),
                });
            }
            // Resolve each explicit type argument, then fill in defaults
            let mut resolved_args = Vec::with_capacity(total_params);
            for arg in type_args {
                let resolved = resolve_type_expr(arg, registry, dim_params, index_params, src)?;
                resolved_args.push(resolved);
            }
            // Fill in defaults for any remaining params
            for param in type_def.generic_params.iter().skip(type_args.len()) {
                let default_expr =
                    param
                        .default
                        .as_ref()
                        .ok_or_else(|| GraphcalError::EvalError {
                            message: format!(
                                "internal: generic parameter `{}` has no default",
                                param.name
                            ),
                            src: src.clone(),
                            span: type_ann.span.into(),
                        })?;
                let resolved =
                    resolve_type_expr(default_expr, registry, dim_params, index_params, src)?;
                resolved_args.push(resolved);
            }
            Ok(ResolvedTypeExpr::GenericStruct {
                name: StructTypeName::new(type_name),
                type_args: resolved_args,
                span: type_ann.span,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]
    use super::*;
    use graphcal_registry::prelude::load_prelude;
    use graphcal_registry::registry::RegistryBuilder;
    use graphcal_syntax::dimension::BaseDimId;
    use graphcal_syntax::parser::Parser;

    fn make_registry() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        b.build()
    }

    /// Create a simple dimension `TypeExpr` from a name string like `"Velocity"`.
    fn make_dim_type_expr(name: &str) -> graphcal_syntax::ast::TypeExpr {
        graphcal_syntax::ast::TypeExpr {
            kind: graphcal_syntax::ast::TypeExprKind::DimExpr(graphcal_syntax::ast::DimExpr {
                terms: vec![graphcal_syntax::ast::DimExprItem {
                    op: graphcal_syntax::ast::MulDivOp::Mul,
                    term: graphcal_syntax::ast::DimTerm {
                        name: graphcal_syntax::ast::Ident {
                            name: name.to_string(),
                            span: Span::new(0, 0),
                        },
                        power: None,
                        span: Span::new(0, 0),
                    },
                }],
                span: Span::new(0, 0),
            }),
            constraints: vec![],
            span: Span::new(0, 0),
        }
    }

    fn make_registry_with_struct() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        b.register_type(graphcal_registry::registry::TypeDef {
            name: StructTypeName::new("TransferResult"),
            generic_params: vec![],
            derives: vec![],
            variants: vec![graphcal_registry::registry::VariantDef {
                name: graphcal_syntax::names::VariantName::new("TransferResult"),
                fields: vec![
                    graphcal_registry::registry::StructField {
                        name: graphcal_syntax::names::FieldName::new("dv1"),
                        type_ann: make_dim_type_expr("Velocity"),
                    },
                    graphcal_registry::registry::StructField {
                        name: graphcal_syntax::names::FieldName::new("dv2"),
                        type_ann: make_dim_type_expr("Velocity"),
                    },
                ],
            }],
        });
        b.build()
    }

    fn make_registry_with_index() -> Registry {
        let mut b = RegistryBuilder::new();
        load_prelude(&mut b);
        b.register_index(graphcal_registry::registry::IndexDef {
            name: IndexName::new("Maneuver"),
            kind: graphcal_registry::registry::IndexKind::Named {
                variants: vec![
                    graphcal_syntax::names::VariantName::new("Departure"),
                    graphcal_syntax::names::VariantName::new("Insertion"),
                ],
            },
        });
        b.build()
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
            ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
        );
    }

    #[test]
    fn resolve_compound_dimension() {
        let r = make_registry();
        let te = parse_type("Length / Time^2");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        let expected = Dimension::base(BaseDimId::Prelude("Length".to_string()))
            / Dimension::base(BaseDimId::Prelude("Time".to_string())).pow_int(2);
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
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
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
        let expected = Dimension::base(BaseDimId::Prelude("Length".to_string()))
            / Dimension::base(BaseDimId::Prelude("Time".to_string()));
        assert_eq!(resolved, ResolvedTypeExpr::Scalar(expected));
    }

    // --- type_resolve() integration tests ---

    fn parse_and_type_resolve(source: &str) -> Result<TIR, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        let src = NamedSource::new("test", Arc::new(source.to_string()));
        let ir = graphcal_ir::ir::lower(&file, &src)?;
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

    #[test]
    fn type_resolve_generics() {
        let source = include_str!("../../../tests/fixtures/generics.gcl");
        let tir = parse_and_type_resolve(source).unwrap();
        // pos_eci should be a GenericStruct with type args
        let pos_type = &tir.resolved_decl_types["pos_eci"];
        match pos_type {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Vec3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci")
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }
        // x_pos should be scalar Length
        assert_eq!(
            tir.resolved_decl_types["x_pos"],
            ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude("Length".to_string())))
        );
    }

    #[test]
    fn type_resolve_default_type_params() {
        let source = include_str!("../../../tests/fixtures/generics.gcl");
        let tir = parse_and_type_resolve(source).unwrap();

        // pos3_eci: Pos3<Length, Eci> — explicit, 2 type args
        let pos3_eci = &tir.resolved_decl_types["pos3_eci"];
        match pos3_eci {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Pos3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Eci")
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }

        // pos3_default: Pos3<Length> — default fills in Unframed
        let pos3_default = &tir.resolved_decl_types["pos3_default"];
        match pos3_default {
            ResolvedTypeExpr::GenericStruct {
                name, type_args, ..
            } => {
                assert_eq!(name.as_str(), "Pos3");
                assert_eq!(type_args.len(), 2);
                assert_eq!(
                    type_args[0],
                    ResolvedTypeExpr::Scalar(Dimension::base(BaseDimId::Prelude(
                        "Length".to_string()
                    )))
                );
                assert!(
                    matches!(&type_args[1], ResolvedTypeExpr::Struct(n, _) if n.as_str() == "Unframed"),
                    "expected Struct(Unframed), got {:?}",
                    type_args[1]
                );
            }
            other => panic!("expected GenericStruct, got {other:?}"),
        }
    }

    // --- resolved_to_declared_type() tests ---

    use graphcal_registry::declared_type::DeclaredType;

    #[test]
    fn convert_dimensionless() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Dimensionless, &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(Dimension::dimensionless()));
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
        let dim = Dimension::base(BaseDimId::Prelude("Length".to_string()));
        let dt =
            resolved_to_declared_type(&ResolvedTypeExpr::Scalar(dim.clone()), &make_src()).unwrap();
        assert_eq!(dt, DeclaredType::Scalar(dim));
    }

    #[test]
    fn convert_struct() {
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Struct(StructTypeName::new("Foo"), Span::new(0, 0)),
            &make_src(),
        )
        .unwrap();
        assert_eq!(dt, DeclaredType::Struct(StructTypeName::new("Foo"), vec![]));
    }

    #[test]
    fn convert_indexed() {
        let dt = resolved_to_declared_type(
            &ResolvedTypeExpr::Indexed {
                base: Box::new(ResolvedTypeExpr::Scalar(Dimension::base(
                    BaseDimId::Prelude("Length".to_string()),
                ))),
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
                element: Box::new(DeclaredType::Scalar(Dimension::base(BaseDimId::Prelude(
                    "Length".to_string()
                )))),
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

    // --- Datetime type resolution tests ---

    #[test]
    fn resolve_bare_datetime() {
        let r = make_registry();
        let te = parse_type("Datetime");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
    }

    #[test]
    fn resolve_datetime_utc() {
        let r = make_registry();
        let te = parse_type("Datetime<UTC>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::UTC));
    }

    #[test]
    fn resolve_datetime_tt() {
        let r = make_registry();
        let te = parse_type("Datetime<TT>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TT));
    }

    #[test]
    fn resolve_datetime_tai() {
        let r = make_registry();
        let te = parse_type("Datetime<TAI>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::TAI));
    }

    #[test]
    fn resolve_datetime_gpst() {
        let r = make_registry();
        let te = parse_type("Datetime<GPST>");
        let resolved = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap();
        assert_eq!(resolved, ResolvedTypeExpr::Datetime(TimeScale::GPST));
    }

    #[test]
    fn resolve_datetime_unknown_scale_error() {
        let r = make_registry();
        let te = parse_type("Datetime<XYZ>");
        let err = resolve_type_expr(&te, &r, &[], &[], &make_src()).unwrap_err();
        assert!(matches!(err, GraphcalError::EvalError { .. }));
    }

    #[test]
    fn convert_datetime_utc() {
        let dt =
            resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::UTC), &make_src())
                .unwrap();
        assert_eq!(dt, DeclaredType::Datetime(TimeScale::UTC));
    }

    #[test]
    fn convert_datetime_tt() {
        let dt = resolved_to_declared_type(&ResolvedTypeExpr::Datetime(TimeScale::TT), &make_src())
            .unwrap();
        assert_eq!(dt, DeclaredType::Datetime(TimeScale::TT));
    }
}

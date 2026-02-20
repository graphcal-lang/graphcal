//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines name resolution (`resolve`), registry construction
//! (dimensions, units, indexes, structs), and function registration into a
//! single `IR` value that downstream stages can consume without reaching
//! back to the raw AST.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{AssertBody, DeclKind, Expr, ExprKind, File, FnDecl, TypeExpr};
use graphcal_syntax::dimension::Rational;
use graphcal_syntax::names::{DimName, FnName};
use graphcal_syntax::span::Span;

use crate::error::GraphcalError;
use crate::eval::format_unit_expr;
use crate::prelude::load_prelude;
use crate::registry::{self, Registry, RegistryBuilder};
use crate::resolve::{DeclCategory, ImportedNames, ResolvedFile, resolve_with_imports};

/// Intermediate Representation produced by [`lower`].
///
/// Contains everything downstream stages need:
/// - A `Registry` with dimensions, units, indexes, structs, and functions
/// - Declarations (consts, params, nodes) with their expressions
/// - Dependency graphs for const and runtime evaluation ordering
/// - Source-order tracking for deterministic output
/// - User-defined function declarations
#[derive(Debug)]
pub struct IR {
    /// The type/unit/dimension/index/struct/function registry.
    pub registry: Registry,
    /// Const declarations in source order: (name, `type_ann`, expr, span).
    pub consts: Vec<(String, TypeExpr, Expr, Span)>,
    /// Param declarations in source order: (name, `type_ann`, expr, span).
    pub params: Vec<(String, TypeExpr, Expr, Span)>,
    /// Node declarations in source order: (name, `type_ann`, expr, span).
    pub nodes: Vec<(String, TypeExpr, Expr, Span)>,
    /// Assert declarations in source order: (name, body, span).
    pub asserts: Vec<(String, AssertBody, Span)>,
    /// For each param/node, the set of `@`-references (runtime deps).
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
    /// Set of all assert names.
    pub assert_names: HashSet<String>,
    /// Mapping from assert name to the list of declarations that assume it.
    pub assumes_map: HashMap<String, Vec<String>>,
}

/// Lower an AST into an [`IR`].
///
/// This combines:
/// 1. Name resolution (`resolve`) — checks duplicates, casing, extracts deps
/// 2. Registry construction — registers dimensions, units, indexes, structs from declarations
/// 3. Function registration — registers user-defined functions into the registry
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails
/// (e.g., unknown dimension in a type annotation, duplicate names, etc.).
#[cfg(test)]
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<IR, GraphcalError> {
    lower_with_imports(ast, src, &ImportedNames::default())
}

/// Lower an AST with imported declarations into an [`IR`].
///
/// Same as [`lower`] but accepts imported names from other files.
/// The registry is frozen (via `build()`) before returning.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
#[cfg(test)]
fn lower_with_imports(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<IR, GraphcalError> {
    let (builder, resolved_ir) = lower_to_builder(ast, src, imported)?;
    Ok(resolved_ir.freeze(builder.build()))
}

/// Lower an AST with imported declarations, returning a `RegistryBuilder`
/// that can be further mutated (e.g., to register imported type-system
/// declarations) before freezing.
///
/// Call [`UnfrozenIR::freeze`] with the final [`Registry`] to produce an [`IR`].
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
pub fn lower_to_builder(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<(RegistryBuilder, UnfrozenIR), GraphcalError> {
    // Step 1: Name resolution
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Build registry (prelude + user-declared dimensions/units/indexes/structs)
    let mut builder = RegistryBuilder::new();
    load_prelude(&mut builder);
    register_file_declarations(ast, &mut builder, src)?;

    // Step 3: Register user-defined functions
    register_functions(&resolved, &mut builder, src)?;

    // Step 4: Extract type annotations from the AST and pair with resolved declarations.
    // Build a map from declaration name to TypeExpr.
    let mut type_anns: HashMap<String, TypeExpr> = HashMap::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::Const(c) => {
                type_anns.insert(c.name.value.to_string(), c.type_ann.clone());
            }
            DeclKind::Param(p) => {
                type_anns.insert(p.name.value.to_string(), p.type_ann.clone());
            }
            DeclKind::Node(n) => {
                type_anns.insert(n.name.value.to_string(), n.type_ann.clone());
            }
            _ => {}
        }
    }
    // Also extract type annotations from imported declarations.
    for (name, type_ann, _, _) in &imported.consts {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.params {
        type_anns.insert(name.clone(), type_ann.clone());
    }
    for (name, type_ann, _, _) in &imported.nodes {
        type_anns.insert(name.clone(), type_ann.clone());
    }

    let consts = resolved
        .consts
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let params = resolved
        .params
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .ok_or_else(|| GraphcalError::EvalError {
                    message: format!("internal: missing type annotation for `{name}`"),
                    src: src.clone(),
                    span: span.into(),
                })?;
            Ok((name, type_ann, expr, span))
        })
        .collect::<Result<Vec<_>, GraphcalError>>()?;

    let unfrozen = UnfrozenIR {
        consts,
        params,
        nodes,
        asserts: resolved.asserts,
        runtime_deps: resolved.runtime_deps,
        const_deps: resolved.const_deps,
        source_order: resolved.source_order,
        functions: resolved.functions,
        assert_names: resolved.assert_names,
        assumes_map: resolved.assumes_map,
    };

    Ok((builder, unfrozen))
}

/// An IR without a frozen registry, awaiting a call to [`freeze`](Self::freeze).
pub struct UnfrozenIR {
    consts: Vec<(String, TypeExpr, Expr, Span)>,
    params: Vec<(String, TypeExpr, Expr, Span)>,
    nodes: Vec<(String, TypeExpr, Expr, Span)>,
    asserts: Vec<(String, graphcal_syntax::ast::AssertBody, Span)>,
    runtime_deps: HashMap<String, HashSet<String>>,
    const_deps: HashMap<String, HashSet<String>>,
    source_order: Vec<(String, DeclCategory)>,
    functions: Vec<(String, graphcal_syntax::ast::FnDecl, Span)>,
    assert_names: HashSet<String>,
    assumes_map: HashMap<String, Vec<String>>,
}

impl UnfrozenIR {
    /// Freeze into a complete [`IR`] by providing a built [`Registry`].
    #[must_use]
    pub fn freeze(self, registry: Registry) -> IR {
        IR {
            registry,
            consts: self.consts,
            params: self.params,
            nodes: self.nodes,
            asserts: self.asserts,
            runtime_deps: self.runtime_deps,
            const_deps: self.const_deps,
            source_order: self.source_order,
            functions: self.functions,
            assert_names: self.assert_names,
            assumes_map: self.assumes_map,
        }
    }
}

/// Register dimensions, units, indexes, and struct types from a file's declarations
/// into the registry.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_file_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, None)
}

/// Register only the named type-system declarations (dimensions, units, indexes, types)
/// from a file into the registry.
///
/// This is the selective counterpart to [`register_file_declarations`]: instead of
/// registering everything, it only registers declarations whose names are in `names`.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_selected_declarations(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    names: &std::collections::HashSet<String>,
) -> Result<(), GraphcalError> {
    register_declarations_impl(file, registry, src, Some(names))
}

/// Shared implementation for registering type-system declarations.
///
/// When `filter` is `None`, all declarations are registered.
/// When `filter` is `Some(names)`, only declarations whose names are in `names` are registered.
#[expect(clippy::too_many_lines, reason = "sequential declaration registration")]
fn register_declarations_impl(
    file: &File,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    filter: Option<&std::collections::HashSet<String>>,
) -> Result<(), GraphcalError> {
    let should_register = |name: &str| filter.is_none_or(|names| names.contains(name));

    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(d) if should_register(d.name.value.as_str()) => {
                if let Some(def) = &d.definition {
                    // Derived dimension — resolve the expression
                    let dim = registry.resolve_dim_expr(def).ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: d.name.value.clone(),
                            src: src.clone(),
                            span: d.name.span.into(),
                        }
                    })?;
                    registry.register_dimension(d.name.value.clone(), dim);
                } else {
                    // Base dimension — register a new orthogonal axis
                    registry.register_base_dimension(d.name.value.clone());
                }
            }
            DeclKind::Unit(u) if should_register(u.name.value.as_str()) => {
                let dim = registry.resolve_dim_expr(&u.dim_type).ok_or_else(|| {
                    GraphcalError::UnknownDimension {
                        name: DimName::new(u.name.value.as_str()),
                        src: src.clone(),
                        span: u.name.span.into(),
                    }
                })?;
                let scale = if let Some(def) = &u.definition {
                    let (_unit_dim, base_scale) = registry
                        .resolve_unit_expr(&def.unit_expr)
                        .ok_or_else(|| GraphcalError::UnknownUnit {
                            name: u.name.value.clone(),
                            src: src.clone(),
                            span: def.span.into(),
                        })?;
                    def.scale * base_scale
                } else {
                    1.0
                };
                // If this is a base unit (scale=1, no definition) for a single
                // base dimension, record the unit name as the SI symbol for
                // that dimension. This handles user-defined dimensions like
                // `unit bit: Information;` → symbol "bit" for Information.
                if u.definition.is_none() {
                    // Check if this dimension is a single base dimension
                    let mut iter = dim.iter();
                    if let Some((&id, &exp)) = iter.next()
                        && iter.next().is_none()
                        && exp == Rational::ONE
                    {
                        registry.set_base_dim_symbol(id, u.name.value.to_string());
                    }
                }
                registry.register_unit(u.name.value.clone(), dim, scale);
            }
            DeclKind::Index(idx) if should_register(idx.name.value.as_str()) => {
                let kind = match &idx.kind {
                    graphcal_syntax::ast::IndexDeclKind::Named { variants } => {
                        registry.register_type(registry::TypeDef {
                            name: graphcal_syntax::names::StructTypeName::new(
                                idx.name.value.as_str(),
                            ),
                            generic_params: vec![],
                            derives: vec![],
                            variants: variants
                                .iter()
                                .map(|v| registry::VariantDef {
                                    name: v.value.clone(),
                                    fields: vec![],
                                })
                                .collect(),
                        });
                        registry::IndexKind::Named {
                            variants: variants.iter().map(|v| v.value.clone()).collect(),
                        }
                    }
                    graphcal_syntax::ast::IndexDeclKind::Range {
                        start: start_expr,
                        end: end_expr,
                        step: step_expr,
                    } => lower_range_index(
                        &idx.name.value,
                        start_expr,
                        end_expr,
                        step_expr,
                        registry,
                        src,
                        decl.span,
                    )?,
                };
                registry.register_index(registry::IndexDef {
                    name: idx.name.value.clone(),
                    kind,
                });
            }
            DeclKind::Type(t) if should_register(t.name.value.as_str()) => {
                let generic_params: Vec<registry::TypeGenericParam> = t
                    .generic_params
                    .iter()
                    .map(|g| registry::TypeGenericParam {
                        name: g.name.value.clone(),
                        constraint: g.constraint.into(),
                        default: g.default.clone(),
                    })
                    .collect();
                let mut variants = Vec::new();
                for variant in &t.variants {
                    let mut fields = Vec::new();
                    for field in &variant.fields {
                        fields.push(registry::StructField {
                            name: field.name.value.clone(),
                            type_ann: field.type_ann.clone(),
                        });
                    }
                    variants.push(registry::VariantDef {
                        name: variant.name.value.clone(),
                        fields,
                    });
                }
                registry.register_type(registry::TypeDef {
                    name: t.name.value.clone(),
                    generic_params,
                    derives: t.derives.iter().map(|d| d.value).collect(),
                    variants,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Evaluate a range expression (e.g. `0.0 s`) to get its SI value and dimension.
///
/// Range expressions are syntactically restricted to numeric literals and
/// unit-annotated literals, so we evaluate them directly against the
/// `RegistryBuilder` instead of going through the full `eval_expr` pipeline.
///
/// Returns `(si_value, dimension)`.
fn eval_range_expr(
    expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(f64, graphcal_syntax::dimension::Dimension), GraphcalError> {
    use graphcal_syntax::dimension::Dimension;

    match &expr.kind {
        ExprKind::Number(n) => Ok((*n, Dimension::dimensionless())),
        ExprKind::UnitLiteral { value, unit } => {
            let (dim, scale) =
                registry
                    .resolve_unit_expr(unit)
                    .ok_or_else(|| GraphcalError::EvalError {
                        message: "unknown unit in range expression".to_string(),
                        src: src.clone(),
                        span: unit.span.into(),
                    })?;
            Ok((*value * scale, dim))
        }
        ExprKind::UnaryOp {
            op: graphcal_syntax::ast::UnaryOp::Neg,
            operand,
        } => {
            let (val, dim) = eval_range_expr(operand, registry, src)?;
            Ok((-val, dim))
        }
        _ => Err(GraphcalError::EvalError {
            message: "range expression must be a numeric or unit literal".to_string(),
            src: src.clone(),
            span: expr.span.into(),
        }),
    }
}

/// Lower a range index declaration, evaluating start/end/step and validating dimensions.
fn lower_range_index(
    name: &graphcal_syntax::names::IndexName,
    start_expr: &Expr,
    end_expr: &Expr,
    step_expr: &Expr,
    registry: &RegistryBuilder,
    src: &NamedSource<Arc<String>>,
    decl_span: graphcal_syntax::span::Span,
) -> Result<registry::IndexKind, GraphcalError> {
    let (start_val, start_dim) = eval_range_expr(start_expr, registry, src)?;
    let (end_val, end_dim) = eval_range_expr(end_expr, registry, src)?;
    let (step_val, step_dim) = eval_range_expr(step_expr, registry, src)?;

    // All three must have the same dimension
    if start_dim != end_dim || start_dim != step_dim {
        return Err(GraphcalError::RangeIndexDimensionMismatch {
            name: name.clone(),
            start_dim: format!("Dimension({})", registry.format_dimension(&start_dim)),
            end_dim: format!("Dimension({})", registry.format_dimension(&end_dim)),
            step_dim: format!("Dimension({})", registry.format_dimension(&step_dim)),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    // Validate: start <= end
    if start_val > end_val {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: format!("start ({start_val}) must be <= end ({end_val})"),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    // Validate: step > 0
    if step_val <= 0.0 {
        return Err(GraphcalError::RangeIndexInvalid {
            name: name.clone(),
            message: format!("step ({step_val}) must be > 0"),
            src: src.clone(),
            span: decl_span.into(),
        });
    }

    // Extract display unit from the start expression's unit annotation.
    let (display_label, display_scale) = match &start_expr.kind {
        ExprKind::UnitLiteral { unit, .. } => {
            if let Some((_dim, scale)) = registry.resolve_unit_expr(unit) {
                (Some(format_unit_expr(unit)), scale)
            } else {
                (None, 1.0)
            }
        }
        _ => (None, 1.0),
    };

    Ok(registry::IndexKind::Range {
        start: start_val,
        end: end_val,
        step: step_val,
        dimension: start_dim,
        display_label,
        display_scale,
    })
}

/// Register user-defined functions from a [`ResolvedFile`] into the registry builder.
fn register_functions(
    resolved: &ResolvedFile,
    registry: &mut RegistryBuilder,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (name, fn_decl, span) in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: FnName::new(name),
            generic_params: fn_decl
                .generic_params
                .iter()
                .map(|g| {
                    let constraint = match g.constraint {
                        graphcal_syntax::ast::GenericConstraint::Dim => {
                            registry::FnGenericConstraint::Dim
                        }
                        graphcal_syntax::ast::GenericConstraint::Index => {
                            registry::FnGenericConstraint::Index
                        }
                        graphcal_syntax::ast::GenericConstraint::Type => {
                            return Err(GraphcalError::EvalError {
                                message: format!(
                                    "internal: `Type` constraint is not valid on function generic parameter `{}`",
                                    g.name.value
                                ),
                                src: src.clone(),
                                span: g.name.span.into(),
                            });
                        }
                    };
                    Ok(registry::FnGenericParam {
                        name: g.name.value.clone(),
                        constraint,
                    })
                })
                .collect::<Result<Vec<_>, GraphcalError>>()?,
            params: fn_decl
                .params
                .iter()
                .map(|p| registry::FnParamDef {
                    name: p.name.name.clone(),
                    type_expr: p.type_ann.clone(),
                })
                .collect(),
            return_type_expr: fn_decl.return_type.clone(),
            body: fn_decl.body.clone(),
            span: *span,
        });
    }
    Ok(())
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
    use graphcal_syntax::parser::Parser;

    fn make_src(source: &str) -> NamedSource<Arc<String>> {
        NamedSource::new("test", Arc::new(source.to_string()))
    }

    fn parse_and_lower(source: &str) -> Result<IR, GraphcalError> {
        let file = Parser::new(source).parse_file().unwrap();
        lower(&file, &make_src(source))
    }

    #[test]
    fn lower_rocket() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 1); // G0
        assert_eq!(ir.params.len(), 3); // dry_mass, fuel_mass, isp
        assert_eq!(ir.nodes.len(), 3); // v_exhaust, mass_ratio, delta_v
        assert!(ir.registry.dimensions.get_dimension("Length").is_some());
        assert!(ir.registry.units.get_unit("km").is_some());
    }

    #[test]
    fn lower_constants() {
        let source = include_str!("../../../tests/fixtures/constants.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert_eq!(ir.consts.len(), 4);
        assert_eq!(ir.params.len(), 1);
        assert_eq!(ir.nodes.len(), 2);
    }

    #[test]
    fn lower_functions() {
        let source = include_str!("../../../tests/fixtures/functions.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(!ir.functions.is_empty());
        // Functions should be registered in the registry
        assert!(
            ir.registry
                .functions
                .get_function("orbital_velocity")
                .is_some()
        );
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.indexes.get_index("Maneuver").is_some());
    }

    #[test]
    fn lower_hohmann() {
        let source = include_str!("../../../tests/fixtures/hohmann.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.types.get_type("TransferResult").is_some());
    }

    #[test]
    fn lower_duplicate_name_error() {
        let err = parse_and_lower("param x: Dimensionless = 1.0;\nnode x: Dimensionless = 2.0;")
            .unwrap_err();
        assert!(matches!(err, GraphcalError::DuplicateName { .. }));
    }

    #[test]
    fn lower_source_order_preserved() {
        let ir = parse_and_lower(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;",
        )
        .unwrap();
        let names: Vec<&str> = ir.source_order.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["b", "a", "z"]);
    }

    #[test]
    fn lower_deps_extracted() {
        let ir = parse_and_lower(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 2.0;\nnode c: Dimensionless = @a + @b;",
        )
        .unwrap();
        let c_deps = &ir.runtime_deps["c"];
        assert!(c_deps.contains("a"));
        assert!(c_deps.contains("b"));
        assert_eq!(c_deps.len(), 2);
    }
}

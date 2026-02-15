//! Intermediate Representation (IR) — the result of lowering an AST.
//!
//! `lower()` combines name resolution (`resolve`), registry construction
//! (dimensions, units, indexes, structs), and function registration into a
//! single `IR` value that downstream stages can consume without reaching
//! back to the raw AST.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{DeclKind, Expr, File, FnDecl, TypeExpr};
use graphcal_syntax::names::{DimName, FnName};
use graphcal_syntax::span::Span;

use crate::error::GraphcalError;
use crate::prelude::load_prelude;
use crate::registry::{self, Registry};
use crate::resolve::{DeclCategory, ImportedNames, ResolvedFile, resolve_with_imports};

/// The kind of a declaration (mirrors [`DeclCategory`] for external use).
pub use crate::resolve::DeclCategory as IrDeclCategory;

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
    /// For each param/node, the set of `@`-references (runtime deps).
    pub runtime_deps: HashMap<String, HashSet<String>>,
    /// For each const, the set of const-references (const deps).
    pub const_deps: HashMap<String, HashSet<String>>,
    /// All declaration names in source order with their category.
    pub source_order: Vec<(String, DeclCategory)>,
    /// User-defined function declarations: (name, decl, span).
    pub functions: Vec<(String, FnDecl, Span)>,
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
pub fn lower(ast: &File, src: &NamedSource<Arc<String>>) -> Result<IR, GraphcalError> {
    lower_with_imports(ast, src, &ImportedNames::default())
}

/// Lower an AST with imported declarations into an [`IR`].
///
/// Same as [`lower`] but accepts imported names from other files.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if name resolution or registry construction fails.
pub fn lower_with_imports(
    ast: &File,
    src: &NamedSource<Arc<String>>,
    imported: &ImportedNames,
) -> Result<IR, GraphcalError> {
    // Step 1: Name resolution
    let resolved = resolve_with_imports(ast, src, imported)?;

    // Step 2: Build registry (prelude + user-declared dimensions/units/indexes/structs)
    let mut registry = Registry::new();
    load_prelude(&mut registry);
    register_file_declarations(ast, &mut registry, src)?;

    // Step 3: Register user-defined functions
    register_functions(&resolved, &mut registry);

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
                .expect("type annotation must exist for every declaration");
            (name, type_ann, expr, span)
        })
        .collect();
    let params = resolved
        .params
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .expect("type annotation must exist for every declaration");
            (name, type_ann, expr, span)
        })
        .collect();
    let nodes = resolved
        .nodes
        .into_iter()
        .map(|(name, expr, span)| {
            let type_ann = type_anns
                .remove(&name)
                .expect("type annotation must exist for every declaration");
            (name, type_ann, expr, span)
        })
        .collect();

    Ok(IR {
        registry,
        consts,
        params,
        nodes,
        runtime_deps: resolved.runtime_deps,
        const_deps: resolved.const_deps,
        source_order: resolved.source_order,
        functions: resolved.functions,
    })
}

/// Register dimensions, units, indexes, and struct types from a file's declarations
/// into the registry.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a referenced dimension or unit is unknown.
pub fn register_file_declarations(
    file: &File,
    registry: &mut Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(d) => {
                let dim = if let Some(def) = &d.definition {
                    registry.resolve_dim_expr(def).ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: d.name.value.clone(),
                            src: src.clone(),
                            span: d.name.span.into(),
                        }
                    })?
                } else {
                    continue;
                };
                registry.register_dimension(d.name.value.clone(), dim);
            }
            DeclKind::Unit(u) => {
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
                registry.register_unit(u.name.value.clone(), dim, scale);
            }
            DeclKind::Index(idx) => {
                registry.register_index(registry::IndexDef {
                    name: idx.name.value.clone(),
                    variants: idx.variants.iter().map(|v| v.value.clone()).collect(),
                });
            }
            DeclKind::Type(t) => {
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

/// Register user-defined functions from a [`ResolvedFile`] into the registry.
fn register_functions(resolved: &ResolvedFile, registry: &mut Registry) {
    for (name, fn_decl, span) in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: FnName::new(name),
            generic_params: fn_decl
                .generic_params
                .iter()
                .map(|g| registry::FnGenericParam {
                    name: g.name.value.clone(),
                    constraint: match g.constraint {
                        graphcal_syntax::ast::GenericConstraint::Dim => {
                            registry::FnGenericConstraint::Dim
                        }
                        graphcal_syntax::ast::GenericConstraint::Index => {
                            registry::FnGenericConstraint::Index
                        }
                        graphcal_syntax::ast::GenericConstraint::Type => {
                            // Type constraint is for type declarations, not functions.
                            // This should be caught by validation before reaching here.
                            unreachable!(
                                "`Type` constraint is not valid on function generic parameters"
                            )
                        }
                    },
                })
                .collect(),
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
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
        assert!(ir.registry.get_dimension("Length").is_some());
        assert!(ir.registry.get_unit("km").is_some());
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
        assert!(ir.registry.get_function("orbital_velocity").is_some());
    }

    #[test]
    fn lower_indexed() {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.get_index("Maneuver").is_some());
    }

    #[test]
    fn lower_hohmann() {
        let source = include_str!("../../../tests/fixtures/hohmann.gcl");
        let ir = parse_and_lower(source).unwrap();
        assert!(ir.registry.get_type("TransferResult").is_some());
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

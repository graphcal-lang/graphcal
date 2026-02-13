use std::collections::HashMap;
use std::sync::Arc;

use miette::{Diagnostic, NamedSource};
use thiserror::Error;

use indexmap::IndexMap;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::const_eval::eval_consts;
use crate::dag::{RuntimeGraph, build_dag};
use crate::dim_check::{DeclaredType, check_dimensions};
use crate::error::GraphcalError;
use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::prelude::load_prelude;
use crate::registry::{self, Registry};
use crate::resolve::{DeclCategory, ImportedNames, ResolvedFile, resolve, resolve_with_imports};
use graphcal_syntax::ast::{DeclKind, ExprKind};
use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::parser::ParseError;

use std::path::Path;

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclType {
    Const,
    Param,
    Node,
}

/// Display unit metadata: the unit name(s) and scale factor for pretty-printing.
#[derive(Debug, Clone)]
pub struct DisplayUnit {
    /// Human-readable unit string (e.g., "km", "m/s^2", "km/hour")
    pub label: String,
    /// Scale factor from SI to this display unit: `display_value = si_value / scale`
    pub scale: f64,
}

/// A runtime value: either a scalar with dimension and display info, or a struct.
#[derive(Debug, Clone)]
pub enum Value {
    Scalar {
        /// The value in base SI units.
        si_value: f64,
        /// The dimension of this value.
        dimension: Dimension,
        /// Optional display unit for pretty-printing.
        display_unit: Option<DisplayUnit>,
    },
    Struct {
        /// The struct type name.
        type_name: String,
        /// Fields in definition order.
        fields: IndexMap<String, Self>,
    },
    /// An indexed collection: maps variant names to values.
    Indexed {
        /// The index type name.
        index_name: String,
        /// Entries in declaration order.
        entries: IndexMap<String, Self>,
    },
}

impl Value {
    /// Get the SI value. Panics on non-scalar values.
    #[must_use]
    pub fn si_value(&self) -> f64 {
        match self {
            Self::Scalar { si_value, .. } => *si_value,
            Self::Struct { type_name, .. } => {
                panic!("called si_value() on struct `{type_name}`")
            }
            Self::Indexed { index_name, .. } => {
                panic!("called si_value() on indexed value `{index_name}[...]`")
            }
        }
    }

    /// Get the dimension. Panics on non-scalar values.
    #[must_use]
    pub fn dimension(&self) -> Dimension {
        match self {
            Self::Scalar { dimension, .. } => *dimension,
            Self::Struct { type_name, .. } => {
                panic!("called dimension() on struct `{type_name}`")
            }
            Self::Indexed { index_name, .. } => {
                panic!("called dimension() on indexed value `{index_name}[...]`")
            }
        }
    }

    /// Get the value formatted for display: in display units if available, otherwise SI.
    #[must_use]
    pub fn display_value(&self) -> f64 {
        match self {
            Self::Scalar {
                si_value,
                display_unit,
                ..
            } => display_unit
                .as_ref()
                .map_or(*si_value, |du| *si_value / du.scale),
            Self::Struct { type_name, .. } => {
                panic!("called display_value() on struct `{type_name}`")
            }
            Self::Indexed { index_name, .. } => {
                panic!("called display_value() on indexed value `{index_name}[...]`")
            }
        }
    }

    /// Get the unit label for display, or `None` for dimensionless values.
    ///
    /// Returns the explicit display unit label if set (e.g., "km", "km/hour"),
    /// otherwise falls back to the SI unit string (e.g., "m/s", "kg").
    #[must_use]
    pub fn display_label(&self) -> Option<String> {
        match self {
            Self::Scalar {
                display_unit,
                dimension,
                ..
            } => display_unit
                .as_ref()
                .map_or_else(|| dimension.si_unit_string(), |du| Some(du.label.clone())),
            Self::Struct { .. } | Self::Indexed { .. } => None,
        }
    }
}

/// The result of evaluating a `.gcl` file.
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order.
    pub consts: Vec<(String, Value)>,
    /// Param values in source order.
    pub params: Vec<(String, Value)>,
    /// Node values in source order.
    pub nodes: Vec<(String, Value)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(String, Value, DeclType)>,
}

/// Full pipeline: parse -> resolve -> const eval -> DAG build -> runtime eval.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
pub fn compile_and_eval(source: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_named(source, "input")
}

/// Full pipeline with a custom source name (used for file paths in diagnostics).
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing or evaluation fails.
pub fn compile_and_eval_named(source: &str, name: &str) -> Result<EvalResult, CompileError> {
    compile_and_eval_with_overrides(source, name, &HashMap::new())
}

/// Full pipeline with parameter overrides.
///
/// Each entry in `overrides` maps a param name to a replacement expression.
/// The overrides are validated (must refer to existing params, not consts/nodes)
/// and then substituted before dimension checking and evaluation.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing, validation, or evaluation fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_with_overrides(
    source: &str,
    name: &str,
    overrides: &HashMap<String, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let src = NamedSource::new(name, Arc::new(source.to_string()));
    let file = graphcal_syntax::parser::Parser::with_name(source, name).parse_file()?;
    let mut resolved = resolve(&file, &src)?;

    // Validate and apply overrides
    for (override_name, override_expr) in overrides {
        // Check if the name exists at all
        if let Some((_, cat)) = resolved
            .source_order
            .iter()
            .find(|(n, _)| n == override_name)
        {
            match cat {
                DeclCategory::Param => {}
                DeclCategory::Const => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: "const".to_string(),
                    }));
                }
                DeclCategory::Node => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: "node".to_string(),
                    }));
                }
            }
        } else {
            return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                name: override_name.clone(),
            }));
        }

        // Replace the expression in resolved.params
        if let Some(entry) = resolved
            .params
            .iter_mut()
            .find(|(n, _, _)| n == override_name)
        {
            entry.1 = override_expr.clone();
        }

        // Re-extract runtime deps for the overridden param
        let all_runtime: std::collections::HashSet<&str> = resolved
            .params
            .iter()
            .chain(resolved.nodes.iter())
            .map(|(n, _, _)| n.as_str())
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        resolved
            .runtime_deps
            .insert(override_name.clone(), graph_refs);
    }

    // Build registry: prelude + user-declared dimensions/units
    let mut registry = Registry::new();
    load_prelude(&mut registry);
    register_file_declarations(&file, &mut registry, &src)?;

    // Register user-defined functions
    register_functions(&resolved, &mut registry);

    // Check for recursive function calls
    crate::fn_check::check_no_recursion(&registry, &src)?;

    // Dimension check
    let declared_types = check_dimensions(&file, &registry, &src)?;

    // Dimension-check override expressions against their param's declared type
    for (override_name, override_expr) in overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name,
            &declared_types,
            &registry,
            &src,
        )?;
    }

    let const_values = eval_consts(&resolved, &registry, &src)?;
    let dag = build_dag(&resolved, &src)?;
    let result = evaluate(
        &resolved,
        &dag,
        &const_values,
        &registry,
        &declared_types,
        &src,
    )?;
    Ok(result)
}

/// Register dimensions, units, indexes, and types from a file's declarations into the registry.
fn register_file_declarations(
    file: &graphcal_syntax::ast::File,
    registry: &mut Registry,
    src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Dimension(d) => {
                let dim = if let Some(def) = &d.definition {
                    registry.resolve_dim_expr(def).ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: d.name.name.clone(),
                            src: src.clone(),
                            span: d.name.span.into(),
                        }
                    })?
                } else {
                    continue;
                };
                registry.register_dimension(&d.name.name, dim);
            }
            DeclKind::Unit(u) => {
                let dim = registry.resolve_dim_expr(&u.dim_type).ok_or_else(|| {
                    GraphcalError::UnknownDimension {
                        name: u.name.name.clone(),
                        src: src.clone(),
                        span: u.name.span.into(),
                    }
                })?;
                let scale = if let Some(def) = &u.definition {
                    let (_unit_dim, base_scale) = registry
                        .resolve_unit_expr(&def.unit_expr)
                        .ok_or_else(|| GraphcalError::UnknownUnit {
                            name: u.name.name.clone(),
                            src: src.clone(),
                            span: def.span.into(),
                        })?;
                    def.scale * base_scale
                } else {
                    1.0
                };
                registry.register_unit(&u.name.name, dim, scale);
            }
            DeclKind::Index(idx) => {
                registry.register_index(registry::IndexDef {
                    name: idx.name.name.clone(),
                    variants: idx.variants.iter().map(|v| v.name.clone()).collect(),
                });
            }
            DeclKind::Type(t) => {
                let mut fields = Vec::new();
                for field in &t.fields {
                    let dim = registry.resolve_type_expr(&field.type_ann).ok_or_else(|| {
                        GraphcalError::UnknownDimension {
                            name: field.name.name.clone(),
                            src: src.clone(),
                            span: field.name.span.into(),
                        }
                    })?;
                    fields.push(registry::StructField {
                        name: field.name.name.clone(),
                        dimension: dim,
                    });
                }
                registry.register_struct(registry::StructDef {
                    name: t.name.name.clone(),
                    fields,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Register user-defined functions from a `ResolvedFile` into the registry.
fn register_functions(resolved: &ResolvedFile, registry: &mut Registry) {
    for (name, fn_decl, span) in &resolved.functions {
        registry.register_function(registry::FnDef {
            name: name.clone(),
            generic_params: fn_decl
                .generic_params
                .iter()
                .map(|g| registry::FnGenericParam {
                    name: g.name.name.clone(),
                    constraint: match g.constraint {
                        graphcal_syntax::ast::GenericConstraint::Dim => {
                            registry::FnGenericConstraint::Dim
                        }
                        graphcal_syntax::ast::GenericConstraint::Index => {
                            registry::FnGenericConstraint::Index
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

/// Full pipeline for multi-file projects with parameter overrides.
///
/// Loads all files referenced by `use` declarations starting from `root_path`,
/// collects imported declarations, and evaluates the root file with imports merged.
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or evaluation fails.
#[expect(
    clippy::too_many_lines,
    reason = "multi-file project compilation has many sequential steps"
)]
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_project(
    root_path: &Path,
    overrides: &HashMap<String, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path)?;

    // Build registry starting with prelude
    let mut registry = Registry::new();
    load_prelude(&mut registry);

    // Collect imported names for the root file
    let mut imported = ImportedNames::default();

    let root_file = &project.files[&project.root];
    let root_src = &root_file.named_source;

    // Process imported files (all except root, in load order = deps first)
    for file_path in &project.load_order {
        if *file_path == project.root {
            continue;
        }
        let loaded = &project.files[file_path];

        // Register dimensions, units, indexes, types from imported file
        register_file_declarations(&loaded.ast, &mut registry, &loaded.named_source)?;
    }

    // Register root file declarations too
    register_file_declarations(&root_file.ast, &mut registry, root_src)?;

    // Now collect exported declarations from imported files based on `use` statements.
    // Walk the root file's use declarations and validate names.
    for decl in &root_file.ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = root_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&use_decl.path);
            let import_canonical = import_path.canonicalize().map_err(|_| {
                CompileError::Eval(GraphcalError::ImportFileNotFound {
                    path: use_decl.path.clone(),
                    src: root_src.clone(),
                    span: use_decl.path_span.into(),
                })
            })?;

            let imported_file = &project.files[&import_canonical];

            // For each requested name, find the matching declaration
            for requested_name in &use_decl.names {
                let found = find_declaration_in_file(&imported_file.ast, &requested_name.name);

                match found {
                    Some(ImportedDecl::Const(name, expr, span)) => {
                        imported.consts.push((name, expr, span));
                    }
                    Some(ImportedDecl::Param(name, expr, span)) => {
                        imported.params.push((name, expr, span));
                    }
                    Some(ImportedDecl::Node(name, expr, span)) => {
                        imported.nodes.push((name, expr, span));
                    }
                    Some(ImportedDecl::Fn(name, fn_decl, span)) => {
                        imported.functions.push((name, fn_decl, span));
                    }
                    None => {
                        return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                            name: requested_name.name.clone(),
                            file_path: use_decl.path.clone(),
                            src: root_src.clone(),
                            span: requested_name.span.into(),
                        }));
                    }
                }
            }
        }
    }

    // Resolve root file with imported names
    let mut resolved = resolve_with_imports(&root_file.ast, root_src, &imported)?;

    // Validate and apply overrides (same logic as compile_and_eval_with_overrides)
    for (override_name, override_expr) in overrides {
        if let Some((_, cat)) = resolved
            .source_order
            .iter()
            .find(|(n, _)| n == override_name)
        {
            match cat {
                DeclCategory::Param => {}
                DeclCategory::Const => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: "const".to_string(),
                    }));
                }
                DeclCategory::Node => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: "node".to_string(),
                    }));
                }
            }
        } else {
            return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                name: override_name.clone(),
            }));
        }

        if let Some(entry) = resolved
            .params
            .iter_mut()
            .find(|(n, _, _)| n == override_name)
        {
            entry.1 = override_expr.clone();
        }

        let all_runtime: std::collections::HashSet<&str> = resolved
            .params
            .iter()
            .chain(resolved.nodes.iter())
            .map(|(n, _, _)| n.as_str())
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        resolved
            .runtime_deps
            .insert(override_name.clone(), graph_refs);
    }

    // Register user-defined functions
    register_functions(&resolved, &mut registry);

    // Check for recursive function calls
    crate::fn_check::check_no_recursion(&registry, root_src)?;

    // Build imported type declarations for dimension checking.
    // The imported declarations' type annotations must be resolved so dim_check
    // knows the dimensions of imported `@` references used in the root file.
    let mut imported_types: HashMap<String, crate::dim_check::DeclaredType> = HashMap::new();
    for file_path in &project.load_order {
        if *file_path == project.root {
            continue;
        }
        let loaded = &project.files[file_path];
        for decl in &loaded.ast.declarations {
            match &decl.kind {
                DeclKind::Const(c) => {
                    let dt = crate::dim_check::resolve_type_annotation(
                        &c.type_ann,
                        &registry,
                        &loaded.named_source,
                    )?;
                    imported_types.insert(c.name.name.clone(), dt);
                }
                DeclKind::Param(p) => {
                    let dt = crate::dim_check::resolve_type_annotation(
                        &p.type_ann,
                        &registry,
                        &loaded.named_source,
                    )?;
                    imported_types.insert(p.name.name.clone(), dt);
                }
                DeclKind::Node(n) => {
                    let dt = crate::dim_check::resolve_type_annotation(
                        &n.type_ann,
                        &registry,
                        &loaded.named_source,
                    )?;
                    imported_types.insert(n.name.name.clone(), dt);
                }
                _ => {}
            }
        }
    }

    // Dimension check
    let declared_types = crate::dim_check::check_dimensions_with_imports(
        &root_file.ast,
        &registry,
        root_src,
        &imported_types,
    )?;

    // Dimension-check override expressions
    for (override_name, override_expr) in overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name,
            &declared_types,
            &registry,
            root_src,
        )?;
    }

    let const_values = eval_consts(&resolved, &registry, root_src)?;
    let dag = build_dag(&resolved, root_src)?;
    let result = evaluate(
        &resolved,
        &dag,
        &const_values,
        &registry,
        &declared_types,
        root_src,
    )?;
    Ok(result)
}

/// A declaration found in a file, classified by kind.
enum ImportedDecl {
    Const(
        String,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Param(
        String,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Node(
        String,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Fn(
        String,
        graphcal_syntax::ast::FnDecl,
        graphcal_syntax::span::Span,
    ),
}

/// Find a declaration by name in a file's AST.
fn find_declaration_in_file(file: &graphcal_syntax::ast::File, name: &str) -> Option<ImportedDecl> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Const(c) if c.name.name == name => {
                return Some(ImportedDecl::Const(
                    c.name.name.clone(),
                    c.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Param(p) if p.name.name == name => {
                return Some(ImportedDecl::Param(
                    p.name.name.clone(),
                    p.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Node(n) if n.name.name == name => {
                return Some(ImportedDecl::Node(
                    n.name.name.clone(),
                    n.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Fn(f) if f.name.name == name => {
                return Some(ImportedDecl::Fn(f.name.name.clone(), f.clone(), decl.span));
            }
            _ => {}
        }
    }
    None
}

/// Convert a `RuntimeValue` to a `Value` using declared type info and display unit extraction.
fn runtime_to_value(
    rv: &RuntimeValue,
    declared_type: Option<&DeclaredType>,
    display_unit: Option<DisplayUnit>,
    registry: &Registry,
) -> Value {
    match rv {
        RuntimeValue::Scalar(si_value) => {
            let dimension = match declared_type {
                Some(DeclaredType::Scalar(d)) => *d,
                _ => Dimension::DIMENSIONLESS,
            };
            Value::Scalar {
                si_value: *si_value,
                dimension,
                display_unit,
            }
        }
        RuntimeValue::Struct { type_name, fields } => {
            let struct_def = registry.get_struct(type_name);
            let converted_fields = fields
                .iter()
                .map(|(field_name, field_rv)| {
                    let field_declared = struct_def.and_then(|sd| {
                        sd.fields
                            .iter()
                            .find(|f| f.name == *field_name)
                            .map(|f| DeclaredType::Scalar(f.dimension))
                    });
                    let val = runtime_to_value(field_rv, field_declared.as_ref(), None, registry);
                    (field_name.clone(), val)
                })
                .collect();
            Value::Struct {
                type_name: type_name.clone(),
                fields: converted_fields,
            }
        }
        RuntimeValue::Indexed {
            index_name,
            entries,
        } => {
            let element_declared = match declared_type {
                Some(DeclaredType::Indexed { element, .. }) => Some(element.as_ref()),
                _ => None,
            };
            let converted_entries = entries
                .iter()
                .map(|(variant, entry_rv)| {
                    let val = runtime_to_value(
                        entry_rv,
                        element_declared,
                        display_unit.clone(),
                        registry,
                    );
                    (variant.clone(), val)
                })
                .collect();
            Value::Indexed {
                index_name: index_name.clone(),
                entries: converted_entries,
            }
        }
        RuntimeValue::VariantLabel { .. } => {
            panic!("VariantLabel should not appear in final values")
        }
    }
}

/// Evaluate the runtime DAG given resolved const values.
fn evaluate(
    resolved: &ResolvedFile,
    dag: &RuntimeGraph,
    const_values: &HashMap<String, RuntimeValue>,
    registry: &Registry,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> Result<EvalResult, GraphcalError> {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut values: HashMap<String, RuntimeValue> = HashMap::new();

    // Insert const values into the lookup table
    for (name, val) in const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them)
    for idx in &dag.topo_order {
        let name = &dag.graph[*idx];
        if values.contains_key(name) {
            continue;
        }
        let expr = &dag.expressions[name];
        let val = eval_expr(
            expr,
            &values,
            &empty_locals,
            &builtin_consts,
            &builtin_fns,
            registry,
            src,
        )?;
        values.insert(name.clone(), val);
    }

    // Build a map from name -> expression for display unit extraction
    let expr_map: HashMap<&str, &graphcal_syntax::ast::Expr> = resolved
        .consts
        .iter()
        .chain(resolved.params.iter())
        .chain(resolved.nodes.iter())
        .map(|(name, expr, _)| (name.as_str(), expr))
        .collect();

    // Helper to build a Value for a given declaration name
    let make_value = |name: &str, rv: &RuntimeValue| -> Value {
        let display_unit = expr_map
            .get(name)
            .and_then(|expr| extract_display_unit(expr, registry));
        runtime_to_value(rv, declared_types.get(name), display_unit, registry)
    };

    // Collect results in source order
    let consts = resolved
        .consts
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &const_values[name]);
            (name.clone(), val)
        })
        .collect();
    let params = resolved
        .params
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &values[name]);
            (name.clone(), val)
        })
        .collect();
    let nodes = resolved
        .nodes
        .iter()
        .map(|(name, _, _)| {
            let val = make_value(name, &values[name]);
            (name.clone(), val)
        })
        .collect();

    // Build the `all` list in source order
    let all = resolved
        .source_order
        .iter()
        .map(|(name, cat)| {
            let rv = match cat {
                DeclCategory::Const => &const_values[name],
                DeclCategory::Param | DeclCategory::Node => &values[name],
            };
            let val = make_value(name, rv);
            let decl_type = match cat {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
            };
            (name.clone(), val, decl_type)
        })
        .collect();

    Ok(EvalResult {
        consts,
        params,
        nodes,
        all,
    })
}

/// Extract display unit from an expression.
///
/// - `ExprKind::Convert { target, .. }` -> use the target unit
/// - `ExprKind::UnitLiteral { unit, .. }` -> use the literal's unit
/// - Anything else -> `None` (display in SI)
fn extract_display_unit(
    expr: &graphcal_syntax::ast::Expr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    match &expr.kind {
        ExprKind::Convert { target, .. } => {
            let (_dim, scale) = registry.resolve_unit_expr(target)?;
            Some(DisplayUnit {
                label: format_unit_expr(target),
                scale,
            })
        }
        ExprKind::UnitLiteral { unit, .. } => {
            let (_dim, scale) = registry.resolve_unit_expr(unit)?;
            Some(DisplayUnit {
                label: format_unit_expr(unit),
                scale,
            })
        }
        // For map literals, extract display unit from the first entry
        ExprKind::MapLiteral { entries } => entries
            .first()
            .and_then(|e| extract_display_unit(&e.value, registry)),
        // For `for` comprehensions, extract display unit from the body
        ExprKind::ForComp { body, .. } => extract_display_unit(body, registry),
        // For scan, extract display unit from the init expression
        ExprKind::Scan { init, .. } => extract_display_unit(init, registry),
        _ => None,
    }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/hour`, `kg * m / s^2`
fn format_unit_expr(expr: &graphcal_syntax::ast::UnitExpr) -> String {
    use graphcal_syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.name.clone();
        if let Some(pow) = item.power
            && pow != 1
        {
            part = format!("{part}^{pow}");
        }
        match item.op {
            MulDivOp::Mul => numerator.push(part),
            MulDivOp::Div => denominator.push(part),
        }
    }

    if denominator.is_empty() {
        numerator.join(" * ")
    } else if numerator.len() == 1 && denominator.len() == 1 {
        format!("{}/{}", numerator[0], denominator[0])
    } else {
        let num = numerator.join(" * ");
        let den = denominator.join(" * ");
        format!("{num}/{den}")
    }
}

/// Top-level compile error that wraps both parse and eval errors.
#[derive(Debug, Error, Diagnostic)]
pub enum CompileError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Parse(#[from] ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Eval(#[from] GraphcalError),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::*;

    /// Find the SI value of a named scalar declaration.
    fn find_value(result: &EvalResult, name: &str) -> f64 {
        result
            .consts
            .iter()
            .chain(result.params.iter())
            .chain(result.nodes.iter())
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .si_value()
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "clearer to express expected math directly"
    )]
    fn eval_rocket_milestone() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let result = compile_and_eval(source).unwrap();

        assert!((find_value(&result, "dry_mass") - 1200.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "fuel_mass") - 2800.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "isp") - 320.0).abs() < f64::EPSILON);
        assert!((find_value(&result, "G0") - 9.80665).abs() < 1e-10);

        let v_exhaust = find_value(&result, "v_exhaust");
        assert!(
            (v_exhaust - 320.0 * 9.80665).abs() < 0.001,
            "v_exhaust = {v_exhaust}"
        );

        let mass_ratio = find_value(&result, "mass_ratio");
        assert!(
            (mass_ratio - (4000.0 / 1200.0)).abs() < 1e-6,
            "mass_ratio = {mass_ratio}"
        );

        let delta_v = find_value(&result, "delta_v");
        let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
        assert!(
            (delta_v - expected_delta_v).abs() < 0.001,
            "delta_v = {delta_v}, expected = {expected_delta_v}"
        );
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "clearer to express expected math directly"
    )]
    fn eval_constants_ksr() {
        let source = include_str!("../../../tests/fixtures/constants.gcl");
        let result = compile_and_eval(source).unwrap();

        assert!((find_value(&result, "G0") - 9.80665).abs() < f64::EPSILON);
        assert!((find_value(&result, "TWO_G0") - 19.6133).abs() < 1e-10);
        assert!(
            (find_value(&result, "HALF_PI") - std::f64::consts::FRAC_PI_2).abs() < f64::EPSILON
        );
        assert!((find_value(&result, "SQRT2") - std::f64::consts::SQRT_2).abs() < f64::EPSILON);

        let circumference = find_value(&result, "circumference");
        let expected = 2.0 * std::f64::consts::PI * 100.0;
        assert!(
            (circumference - expected).abs() < 1e-10,
            "circumference = {circumference}"
        );

        let area = find_value(&result, "area");
        let expected_area = std::f64::consts::PI * 100.0_f64.powf(2.0);
        assert!((area - expected_area).abs() < 1e-10, "area = {area}");
    }

    #[test]
    fn eval_if_else_true_branch() {
        let result =
            compile_and_eval("param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_if_else_false_branch() {
        let result =
            compile_and_eval("param x: Dimensionless = -3.0;\nnode y: Dimensionless = if @x > 0.0 { @x } else { 0.0 };").unwrap();
        assert!((find_value(&result, "y") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_and() {
        let result = compile_and_eval(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 0.0;\nnode c: Dimensionless = if @a > 0.0 && @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_or() {
        let result = compile_and_eval(
            "param a: Dimensionless = 1.0;\nparam b: Dimensionless = 0.0;\nnode c: Dimensionless = if @a > 0.0 || @b > 0.0 { 1.0 } else { 0.0 };",
        )
        .unwrap();
        assert!((find_value(&result, "c") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_unary_neg() {
        let result =
            compile_and_eval("param x: Dimensionless = 5.0;\nnode y: Dimensionless = -@x;")
                .unwrap();
        assert!((find_value(&result, "y") - (-5.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_power() {
        let result =
            compile_and_eval("param x: Dimensionless = 3.0;\nnode y: Dimensionless = @x ^ 2.0;")
                .unwrap();
        assert!((find_value(&result, "y") - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_result_source_order() {
        let result = compile_and_eval(
            "param b: Dimensionless = 2.0;\nparam a: Dimensionless = 1.0;\nnode z: Dimensionless = @a + @b;\nnode y: Dimensionless = @z * 2.0;",
        )
        .unwrap();
        assert_eq!(result.params[0].0, "b");
        assert_eq!(result.params[1].0, "a");
        assert_eq!(result.nodes[0].0, "z");
        assert_eq!(result.nodes[1].0, "y");
    }

    #[test]
    fn eval_result_all_field_source_order() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let result = compile_and_eval(source).unwrap();
        let names: Vec<&str> = result.all.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "dry_mass",
                "fuel_mass",
                "isp",
                "G0",
                "v_exhaust",
                "mass_ratio",
                "delta_v"
            ]
        );
        assert_eq!(result.all[0].2, DeclType::Param);
        assert_eq!(result.all[3].2, DeclType::Const);
        assert_eq!(result.all[4].2, DeclType::Node);
    }

    #[test]
    fn eval_orbital_milestone() {
        let source = include_str!("../../../tests/fixtures/orbital.gcl");
        let result = compile_and_eval(source).unwrap();

        // alt = 400 km -> SI: 400_000.0 m
        assert!(
            (find_value(&result, "alt") - 400_000.0).abs() < f64::EPSILON,
            "alt = {}",
            find_value(&result, "alt")
        );
        // period = 90 min -> SI: 5400.0 s
        assert!(
            (find_value(&result, "period") - 5400.0).abs() < f64::EPSILON,
            "period = {}",
            find_value(&result, "period")
        );
        // R_EARTH = 6371 km -> SI: 6_371_000.0 m
        assert!(
            (find_value(&result, "R_EARTH") - 6_371_000.0).abs() < f64::EPSILON,
            "R_EARTH = {}",
            find_value(&result, "R_EARTH")
        );

        // circumference = 2 * PI * (6_371_000 + 400_000)
        let expected_circumference = 2.0 * std::f64::consts::PI * 6_771_000.0;
        assert!(
            (find_value(&result, "circumference") - expected_circumference).abs() < 0.01,
            "circumference = {}",
            find_value(&result, "circumference")
        );

        // speed = circumference / period
        let expected_speed = expected_circumference / 5400.0;
        assert!(
            (find_value(&result, "speed") - expected_speed).abs() < 0.01,
            "speed = {}",
            find_value(&result, "speed")
        );

        // speed_kmh = speed (same SI value, only display unit changes)
        assert!(
            (find_value(&result, "speed_kmh") - expected_speed).abs() < 0.01,
            "speed_kmh SI = {}",
            find_value(&result, "speed_kmh")
        );

        // Check display units
        let speed_kmh = result.nodes.iter().find(|(n, _)| n == "speed_kmh").unwrap();
        assert_eq!(speed_kmh.1.display_label(), Some("km/hour".to_string()));
        let display_kmh = speed_kmh.1.display_value();
        let expected_kmh = expected_speed / (1000.0 / 3600.0);
        assert!(
            (display_kmh - expected_kmh).abs() < 0.01,
            "speed_kmh display = {display_kmh}"
        );
    }

    #[test]
    fn eval_hohmann_milestone() {
        let source = include_str!("../../../tests/fixtures/hohmann.gcl");
        let result = compile_and_eval(source).unwrap();

        // transfer is a struct — check its fields via total_dv and tof_hours nodes
        let total_dv = find_value(&result, "total_dv");
        // LEO-to-GEO Hohmann total delta-v should be ~3935 m/s
        assert!(
            total_dv > 3900.0 && total_dv < 4000.0,
            "total_dv = {total_dv}"
        );

        let tof_hours = find_value(&result, "tof_hours");
        // Transfer time ~5.26 hours -> SI ~18924 seconds
        assert!(
            tof_hours > 18000.0 && tof_hours < 20000.0,
            "tof_hours SI = {tof_hours}"
        );

        // Check that tof_hours has display unit "hour"
        let tof_entry = result.nodes.iter().find(|(n, _)| n == "tof_hours").unwrap();
        assert_eq!(tof_entry.1.display_label(), Some("hour".to_string()));
        let tof_display = tof_entry.1.display_value();
        assert!(
            tof_display > 5.0 && tof_display < 6.0,
            "tof display = {tof_display} hours"
        );

        // Check that transfer node is a struct
        let transfer_entry = result.nodes.iter().find(|(n, _)| n == "transfer").unwrap();
        match &transfer_entry.1 {
            Value::Struct { type_name, fields } => {
                assert_eq!(type_name, "TransferResult");
                assert_eq!(fields.len(), 4);
                assert!(fields.contains_key("dv1"));
                assert!(fields.contains_key("dv2"));
                assert!(fields.contains_key("total_dv"));
                assert!(fields.contains_key("tof"));
            }
            _ => panic!("expected struct for transfer"),
        }
    }

    #[test]
    fn eval_functions_milestone() {
        let source = include_str!("../../../tests/fixtures/functions.gcl");
        let result = compile_and_eval(source).unwrap();

        // v_parking: orbital velocity at LEO (R_EARTH + 200 km)
        // sqrt(GM_EARTH / (R_EARTH + 200 km)) = sqrt(3.986004418e14 / 6571000)
        let v_parking = find_value(&result, "v_parking");
        assert!(
            v_parking > 7700.0 && v_parking < 7800.0,
            "v_parking = {v_parking}"
        );

        // v_check should equal v_parking (same computation via fn-calling-fn)
        let v_check = find_value(&result, "v_check");
        assert!(
            (v_check - v_parking).abs() < 1e-6,
            "v_check = {v_check}, v_parking = {v_parking}"
        );

        // midpoint_alt: lerp(200 km, 35786 km, 0.5) = 17993 km -> 17993000 m SI
        let midpoint = find_value(&result, "midpoint_alt");
        assert!(
            (midpoint - 17_993_000.0).abs() < 1.0,
            "midpoint_alt = {midpoint}"
        );

        // transfer: Hohmann LEO-to-GEO, total_dv ~3935 m/s
        let transfer_entry = result.nodes.iter().find(|(n, _)| n == "transfer").unwrap();
        match &transfer_entry.1 {
            Value::Struct { type_name, fields } => {
                assert_eq!(type_name, "TransferResult");
                assert_eq!(fields.len(), 3);
                let total_dv = fields["total_dv"].si_value();
                assert!(
                    total_dv > 3900.0 && total_dv < 4000.0,
                    "total_dv = {total_dv}"
                );
            }
            _ => panic!("expected struct for transfer"),
        }
    }

    /// Helper: find a named value and return it (for indexed value tests).
    fn find_entry(result: &EvalResult, name: &str) -> Value {
        result
            .all
            .iter()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .clone()
    }

    /// Helper: extract indexed entries as `Vec<(variant, si_value)>`.
    fn indexed_si_values(value: &Value) -> Vec<(&str, f64)> {
        match value {
            Value::Indexed { entries, .. } => entries
                .iter()
                .map(|(k, v)| (k.as_str(), v.si_value()))
                .collect(),
            _ => panic!("expected indexed value, got {value:?}"),
        }
    }

    #[test]
    fn eval_indexed_milestone() {
        let source = include_str!("../../../tests/fixtures/indexed.gcl");
        let result = compile_and_eval(source).unwrap();

        // delta_v param: 2460, 120, 1830 m/s (SI)
        let dv = find_entry(&result, "delta_v");
        let dv_vals = indexed_si_values(&dv);
        assert_eq!(dv_vals.len(), 3);
        assert!(
            (dv_vals[0].1 - 2460.0).abs() < 0.01,
            "Departure = {}",
            dv_vals[0].1
        );
        assert!(
            (dv_vals[1].1 - 120.0).abs() < 0.01,
            "Correction = {}",
            dv_vals[1].1
        );
        assert!(
            (dv_vals[2].1 - 1830.0).abs() < 0.01,
            "Insertion = {}",
            dv_vals[2].1
        );

        // double_dv: doubled values
        let ddv = find_entry(&result, "double_dv");
        let double_dv_vals = indexed_si_values(&ddv);
        assert!((double_dv_vals[0].1 - 4920.0).abs() < 0.01);
        assert!((double_dv_vals[1].1 - 240.0).abs() < 0.01);
        assert!((double_dv_vals[2].1 - 3660.0).abs() < 0.01);

        // total_dv: 2460 + 120 + 1830 = 4410 m/s
        assert!((find_value(&result, "total_dv") - 4410.0).abs() < 0.01);

        // max_dv: 2460
        assert!((find_value(&result, "max_dv") - 2460.0).abs() < 0.01);

        // min_dv: 120
        assert!((find_value(&result, "min_dv") - 120.0).abs() < 0.01);

        // mean_dv: 4410 / 3 = 1470
        assert!((find_value(&result, "mean_dv") - 1470.0).abs() < 0.01);

        // n_maneuvers: 3
        assert!((find_value(&result, "n_maneuvers") - 3.0).abs() < f64::EPSILON);

        // departure_dv: 2460
        assert!((find_value(&result, "departure_dv") - 2460.0).abs() < 0.01);

        // cumulative_dv: scan cumulative [2460, 2460+120=2580, 2580+1830=4410]
        let cumulative = find_entry(&result, "cumulative_dv");
        let cumulative_vals = indexed_si_values(&cumulative);
        assert!((cumulative_vals[0].1 - 2460.0).abs() < 0.01);
        assert!((cumulative_vals[1].1 - 2580.0).abs() < 0.01);
        assert!((cumulative_vals[2].1 - 4410.0).abs() < 0.01);

        // total_check (generic function): same as total_dv
        assert!((find_value(&result, "total_check") - 4410.0).abs() < 0.01);
    }

    // --- Comparison and boolean operator tests ---

    #[test]
    fn eval_comparison_eq() {
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x == 5.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_comparison_neq() {
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x != 3.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_comparison_lt() {
        let result = compile_and_eval(
            "param x: Dimensionless = 3.0;\nnode y: Dimensionless = if @x < 5.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_comparison_lte() {
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x <= 5.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_comparison_gt() {
        let result = compile_and_eval(
            "param x: Dimensionless = 10.0;\nnode y: Dimensionless = if @x > 5.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_comparison_gte() {
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x >= 5.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_not() {
        let result = compile_and_eval(
            "param x: Dimensionless = 0.0;\nnode y: Dimensionless = if !(@x > 0.0) { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_and_short_circuit() {
        // When first operand is false, second should not matter
        let result = compile_and_eval(
            "param x: Dimensionless = 0.0;\nnode y: Dimensionless = if @x > 0.0 && @x < 10.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_boolean_or_short_circuit() {
        // When first operand is true, second should not matter
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 0.0 || @x < -10.0 { 1.0 } else { 0.0 };",
        ).unwrap();
        assert!((find_value(&result, "y") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_nested_if_else() {
        let result = compile_and_eval(
            "param x: Dimensionless = 5.0;\nnode y: Dimensionless = if @x > 10.0 { 3.0 } else { if @x > 0.0 { 2.0 } else { 1.0 } };",
        ).unwrap();
        assert!((find_value(&result, "y") - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_unary_neg_dimensioned() {
        let result = compile_and_eval("param x: Length = 100 m;\nnode y: Length = -@x;").unwrap();
        assert!((find_value(&result, "y") - (-100.0)).abs() < f64::EPSILON);
    }

    // --- Override tests ---

    fn parse_expr(s: &str) -> graphcal_syntax::ast::Expr {
        graphcal_syntax::parser::Parser::new(s)
            .parse_single_expr()
            .unwrap()
    }

    #[test]
    fn override_param_changes_result() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        // Default isp=320 s, override to 450 s => higher delta_v
        let default = compile_and_eval_named(source, "test").unwrap();
        let default_dv = find_value(&default, "delta_v");

        let mut overrides = HashMap::new();
        overrides.insert("isp".to_string(), parse_expr("450 s"));
        let overridden = compile_and_eval_with_overrides(source, "test", &overrides).unwrap();
        let new_dv = find_value(&overridden, "delta_v");

        assert!(new_dv > default_dv, "higher isp should give higher delta_v");
    }

    #[test]
    fn override_with_wrong_dimension_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        // isp expects Time, not Mass
        let mut overrides = HashMap::new();
        overrides.insert("isp".to_string(), parse_expr("450 kg"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        assert!(result.is_err());
    }

    #[test]
    fn override_node_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert("delta_v".to_string(), parse_expr("100 m/s"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
                assert_eq!(name, "delta_v");
                assert_eq!(actual_kind, "node");
            }
            other => panic!("expected OverrideNotAParam, got {other:?}"),
        }
    }

    #[test]
    fn override_const_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert("G0".to_string(), parse_expr("10.0 m/s^2"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
                assert_eq!(name, "G0");
                assert_eq!(actual_kind, "const");
            }
            other => panic!("expected OverrideNotAParam, got {other:?}"),
        }
    }

    #[test]
    fn override_unknown_param_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert("nonexistent".to_string(), parse_expr("100"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideUnknownParam { name })) => {
                assert_eq!(name, "nonexistent");
            }
            other => panic!("expected OverrideUnknownParam, got {other:?}"),
        }
    }

    #[test]
    fn project_multi_file_rocket() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/rocket_split/main.gcl");
        let result = compile_and_eval_project(&root, &HashMap::new()).unwrap();
        let delta_v = find_value(&result, "delta_v");
        let expected_delta_v = 320.0 * 9.80665 * (4000.0_f64 / 1200.0).ln();
        assert!(
            (delta_v - expected_delta_v).abs() < 0.001,
            "delta_v = {delta_v}, expected = {expected_delta_v}"
        );
    }

    // --- Runtime arithmetic error tests ---

    /// Helper: assert that `compile_and_eval` fails with an `EvalError` whose message contains `needle`.
    fn assert_eval_error(source: &str, needle: &str) {
        let err = compile_and_eval(source).unwrap_err();
        match &err {
            CompileError::Eval(GraphcalError::EvalError { message, .. }) => {
                assert!(
                    message.contains(needle),
                    "expected error containing {needle:?}, got {message:?}"
                );
            }
            other => panic!("expected EvalError containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn eval_division_by_zero() {
        assert_eval_error(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;",
            "division by zero",
        );
    }

    #[test]
    fn eval_zero_divided_by_zero() {
        assert_eval_error(
            "param x: Dimensionless = 0.0;\nnode y: Dimensionless = @x / 0.0;",
            "division by zero",
        );
    }

    #[test]
    fn eval_sqrt_negative() {
        assert_eval_error("node y: Dimensionless = sqrt(-1.0);", "NaN");
    }

    #[test]
    fn eval_ln_zero() {
        assert_eval_error("node y: Dimensionless = ln(0.0);", "infinite");
    }

    #[test]
    fn eval_ln_negative() {
        assert_eval_error("node y: Dimensionless = ln(-1.0);", "NaN");
    }

    #[test]
    fn eval_exp_overflow() {
        assert_eval_error("node y: Dimensionless = exp(1000.0);", "infinite");
    }

    #[test]
    fn eval_power_negative_base_frac_exp() {
        assert_eval_error("node y: Dimensionless = (-1.0) ^ 0.5;", "NaN");
    }

    #[test]
    fn eval_valid_division_ok() {
        let result =
            compile_and_eval("param x: Dimensionless = 10.0;\nnode y: Dimensionless = @x / 2.0;")
                .unwrap();
        assert!((find_value(&result, "y") - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_valid_sqrt_ok() {
        let result = compile_and_eval("node y: Dimensionless = sqrt(4.0);").unwrap();
        assert!((find_value(&result, "y") - 2.0).abs() < f64::EPSILON);
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn division_of_finite_nonzero_is_finite(
                a in proptest::num::f64::NORMAL,
                b in proptest::num::f64::NORMAL,
            ) {
                prop_assume!(b != 0.0 && a.is_finite() && b.is_finite());
                let source = format!(
                    "param x: Dimensionless = {a:e};\nparam y: Dimensionless = {b:e};\nnode z: Dimensionless = @x / @y;"
                );
                let result = compile_and_eval(&source);
                match result {
                    Ok(r) => {
                        let z = find_value(&r, "z");
                        prop_assert!(z.is_finite(), "division produced non-finite: {z}");
                    }
                    Err(CompileError::Eval(GraphcalError::EvalError { message, .. })) => {
                        // Overflow to infinity is correctly caught
                        prop_assert!(
                            message.contains("overflow") || message.contains("infinite"),
                            "unexpected error: {message}"
                        );
                    }
                    Err(e) => prop_assert!(false, "unexpected error type: {e:?}"),
                }
            }

            #[test]
            fn sqrt_of_positive_is_finite(a in 0.0f64..1e150) {
                let source = format!(
                    "param x: Dimensionless = {a:e};\nnode y: Dimensionless = sqrt(@x);"
                );
                let result = compile_and_eval(&source).unwrap();
                let y = find_value(&result, "y");
                prop_assert!(y.is_finite(), "sqrt produced non-finite: {y}");
            }

            #[test]
            fn exp_of_small_is_finite(a in -700.0f64..700.0) {
                let source = format!(
                    "param x: Dimensionless = {a:e};\nnode y: Dimensionless = exp(@x);"
                );
                let result = compile_and_eval(&source).unwrap();
                let y = find_value(&result, "y");
                prop_assert!(y.is_finite(), "exp produced non-finite: {y}");
            }
        }
    }
}

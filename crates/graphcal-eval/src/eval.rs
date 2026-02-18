use std::collections::HashMap;
use std::sync::Arc;

use miette::{Diagnostic, NamedSource};
use thiserror::Error;

use indexmap::IndexMap;

use crate::builtins::{builtin_constants, builtin_functions};
use crate::dim_check::DeclaredType;
use crate::error::GraphcalError;
use crate::eval_expr::{RuntimeValue, eval_expr};
use crate::registry::Registry;
use crate::resolve::{DeclCategory, ImportedNames};
use graphcal_syntax::ast::{DeclKind, ExprKind};
use graphcal_syntax::dimension::Dimension;
use graphcal_syntax::names::{DeclName, FieldName, IndexName, StructTypeName, VariantName};
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

/// A runtime value: either a scalar with dimension and display info, a bool, an integer, or a struct.
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
    Bool(bool),
    Int(i64),
    Struct {
        /// The struct type name.
        type_name: StructTypeName,
        /// The variant name (= type name for single-variant struct sugar).
        variant: VariantName,
        /// Fields in definition order.
        fields: IndexMap<FieldName, Self>,
    },
    /// An indexed collection: maps variant names to values.
    Indexed {
        /// The index type name.
        index_name: IndexName,
        /// Entries in declaration order.
        entries: IndexMap<VariantName, Self>,
    },
}

impl Value {
    /// Get the SI value. Panics on non-scalar values.
    #[must_use]
    pub fn si_value(&self) -> f64 {
        match self {
            Self::Scalar { si_value, .. } => *si_value,
            Self::Bool(_) => panic!("called si_value() on Bool"),
            Self::Int(_) => panic!("called si_value() on Int"),
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
            Self::Scalar { dimension, .. } => dimension.clone(),
            Self::Bool(_) => panic!("called dimension() on Bool"),
            Self::Int(_) => panic!("called dimension() on Int"),
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
            Self::Bool(_) => panic!("called display_value() on Bool"),
            Self::Int(_) => panic!("called display_value() on Int"),
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
    pub fn display_label(
        &self,
        symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    ) -> Option<String> {
        match self {
            Self::Scalar {
                display_unit,
                dimension,
                ..
            } => display_unit.as_ref().map_or_else(
                || dimension.si_unit_string(symbols),
                |du| Some(du.label.clone()),
            ),
            Self::Bool(_) | Self::Int(_) | Self::Struct { .. } | Self::Indexed { .. } => None,
        }
    }
}

/// A runtime error associated with a specific node or param evaluation.
#[derive(Debug, Clone)]
pub enum NodeError {
    /// The expression evaluation failed directly (e.g., division by zero).
    EvalFailed {
        /// Human-readable error message.
        message: String,
    },
    /// Could not evaluate because one or more dependencies failed.
    DependencyFailed {
        /// Names of the dependencies that failed.
        failed_deps: Vec<DeclName>,
    },
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EvalFailed { message } => write!(f, "{message}"),
            Self::DependencyFailed { failed_deps } => {
                let names: Vec<&str> = failed_deps.iter().map(DeclName::as_str).collect();
                write!(f, "dependency failed: {}", names.join(", "))
            }
        }
    }
}

/// The result of evaluating a `.gcl` file.
#[derive(Debug)]
pub struct EvalResult {
    /// Const values in source order (consts are compile-time and never fail at runtime).
    pub consts: Vec<(DeclName, Value)>,
    /// Param values in source order (may contain per-node errors).
    pub params: Vec<(DeclName, Result<Value, NodeError>)>,
    /// Node values in source order (may contain per-node errors).
    pub nodes: Vec<(DeclName, Result<Value, NodeError>)>,
    /// All values in source order with their declaration type.
    pub all: Vec<(DeclName, Result<Value, NodeError>, DeclType)>,
    /// Base dimension symbols for display (e.g., `BaseDimId(0) → "m"`).
    pub base_dim_symbols: std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
}

impl EvalResult {
    /// Returns `true` if all params and nodes evaluated successfully.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.params.iter().any(|(_, r)| r.is_err()) || self.nodes.iter().any(|(_, r)| r.is_err())
    }
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
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let src = NamedSource::new(name, Arc::new(source.to_string()));
    let file = graphcal_syntax::parser::Parser::with_name(source, name).parse_file()?;

    // Lower AST → IR
    let mut ir = crate::ir::lower(&file, &src)?;

    // Validate and apply overrides to IR
    for (override_name, override_expr) in overrides {
        let name_str = override_name.as_str();
        if let Some((_, cat)) = ir.source_order.iter().find(|(n, _)| n == name_str) {
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

        // Replace the expression in ir.params
        if let Some(entry) = ir.params.iter_mut().find(|(n, _, _, _)| n == name_str) {
            entry.2 = override_expr.clone();
        }

        // Re-extract runtime deps for the overridden param
        let all_runtime: std::collections::HashSet<&str> = ir
            .params
            .iter()
            .chain(ir.nodes.iter())
            .map(|(n, _, _, _)| n.as_str())
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        ir.runtime_deps.insert(name_str.to_string(), graph_refs);
    }

    // Type resolve IR → TIR
    let tir = crate::tir::type_resolve(ir, &src)?;

    // Check
    crate::fn_check::check_no_recursion_tir(&tir, &src)?;
    crate::dim_check::check_dimensions_tir(&tir, &src)?;

    // Dimension-check override expressions against their param's declared type
    let declared_types = tir.build_declared_types(&src)?;
    for (override_name, override_expr) in overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
            &tir.registry,
            &tir.resolved_fn_sigs,
            &src,
        )?;
    }

    // Compile TIR → ExecPlan
    let plan = crate::exec_plan::compile(&tir, &src)?;

    // Evaluate
    let result = evaluate_plan(&tir, &plan, &declared_types, &src);
    Ok(result)
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
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path)?;

    let root_file = &project.files[&project.root];
    let root_src = &root_file.named_source;

    // Collect imported names from imported files based on `use` statements.
    let mut imported = ImportedNames::default();
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

            for use_item in &use_decl.names {
                let found = find_declaration_in_file(&imported_file.ast, &use_item.name.name);
                let local_name = use_item.local_name().to_string();

                match found {
                    Some(ImportedDecl::Const(type_ann, expr, span)) => {
                        imported.consts.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Param(type_ann, expr, span)) => {
                        imported.params.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Node(type_ann, expr, span)) => {
                        imported.nodes.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Fn(fn_decl, span)) => {
                        imported.functions.push((local_name, fn_decl, span));
                    }
                    None => {
                        return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                            name: use_item.name.name.clone(),
                            file_path: use_decl.path.clone(),
                            src: root_src.clone(),
                            span: use_item.name.span.into(),
                        }));
                    }
                }
            }
        }
    }

    // Lower root AST → IR (includes root file declarations + functions in registry)
    let mut ir = crate::ir::lower_with_imports(&root_file.ast, root_src, &imported)?;

    // Register imported file declarations (dims/units/indexes/structs) into IR's registry
    for file_path in &project.load_order {
        if *file_path == project.root {
            continue;
        }
        let loaded = &project.files[file_path];
        crate::ir::register_file_declarations(&loaded.ast, &mut ir.registry, &loaded.named_source)?;
    }

    // Validate and apply overrides to IR
    for (override_name, override_expr) in overrides {
        let name_str = override_name.as_str();
        if let Some((_, cat)) = ir.source_order.iter().find(|(n, _)| n == name_str) {
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

        if let Some(entry) = ir.params.iter_mut().find(|(n, _, _, _)| n == name_str) {
            entry.2 = override_expr.clone();
        }

        let all_runtime: std::collections::HashSet<&str> = ir
            .params
            .iter()
            .chain(ir.nodes.iter())
            .map(|(n, _, _, _)| n.as_str())
            .collect();
        let mut graph_refs = std::collections::HashSet::new();
        crate::resolve::collect_graph_refs(override_expr, &all_runtime, &mut graph_refs);
        ir.runtime_deps.insert(name_str.to_string(), graph_refs);
    }

    // Type resolve IR → TIR
    let tir = crate::tir::type_resolve(ir, root_src)?;

    // Check
    crate::fn_check::check_no_recursion_tir(&tir, root_src)?;

    // Dimension check — TIR already includes imported declarations with their real types,
    // so no separate `imported_types` map is needed.
    crate::dim_check::check_dimensions_tir(&tir, root_src)?;

    // Dimension-check override expressions
    let declared_types = tir.build_declared_types(root_src)?;
    for (override_name, override_expr) in overrides {
        crate::dim_check::check_override_dimension(
            override_expr,
            override_name.as_str(),
            &declared_types,
            &tir.registry,
            &tir.resolved_fn_sigs,
            root_src,
        )?;
    }

    // Compile TIR → ExecPlan
    let plan = crate::exec_plan::compile(&tir, root_src)?;

    // Evaluate
    let result = evaluate_plan(&tir, &plan, &declared_types, root_src);
    Ok(result)
}

/// Compile source to TIR without evaluating.
///
/// Runs the pipeline up through type resolution, function recursion check, and
/// dimension check, but does not build an execution plan or evaluate. This is
/// useful for tooling (e.g., LSP) that needs type information without execution.
///
/// # Errors
///
/// Returns a [`CompileError`] if parsing, lowering, or checking fails.
pub fn compile_to_tir(source: &str, name: &str) -> Result<crate::tir::TIR, CompileError> {
    let src = NamedSource::new(name, Arc::new(source.to_string()));
    let file = graphcal_syntax::parser::Parser::with_name(source, name).parse_file()?;
    let ir = crate::ir::lower(&file, &src)?;
    let tir = crate::tir::type_resolve(ir, &src)?;
    crate::fn_check::check_no_recursion_tir(&tir, &src)?;
    crate::dim_check::check_dimensions_tir(&tir, &src)?;
    Ok(tir)
}

/// Compile a multi-file project to TIR without evaluating.
///
/// Loads all files referenced by `use` declarations starting from `root_path`,
/// resolves imports, and runs the pipeline up through dimension checking.
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or checking fails.
pub fn compile_to_tir_project(
    root_path: &Path,
) -> Result<(crate::tir::TIR, crate::loader::LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path)?;

    let root_file = &project.files[&project.root];
    let root_src = &root_file.named_source;

    let mut imported = ImportedNames::default();
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

            for use_item in &use_decl.names {
                let found = find_declaration_in_file(&imported_file.ast, &use_item.name.name);
                let local_name = use_item.local_name().to_string();

                match found {
                    Some(ImportedDecl::Const(type_ann, expr, span)) => {
                        imported.consts.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Param(type_ann, expr, span)) => {
                        imported.params.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Node(type_ann, expr, span)) => {
                        imported.nodes.push((local_name, type_ann, expr, span));
                    }
                    Some(ImportedDecl::Fn(fn_decl, span)) => {
                        imported.functions.push((local_name, fn_decl, span));
                    }
                    None => {
                        return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                            name: use_item.name.name.clone(),
                            file_path: use_decl.path.clone(),
                            src: root_src.clone(),
                            span: use_item.name.span.into(),
                        }));
                    }
                }
            }
        }
    }

    let mut ir = crate::ir::lower_with_imports(&root_file.ast, root_src, &imported)?;

    for file_path in &project.load_order {
        if *file_path == project.root {
            continue;
        }
        let loaded = &project.files[file_path];
        crate::ir::register_file_declarations(&loaded.ast, &mut ir.registry, &loaded.named_source)?;
    }

    let tir = crate::tir::type_resolve(ir, root_src)?;
    crate::fn_check::check_no_recursion_tir(&tir, root_src)?;
    crate::dim_check::check_dimensions_tir(&tir, root_src)?;
    Ok((tir, project))
}

/// A declaration found in a file, classified by kind.
enum ImportedDecl {
    Const(
        graphcal_syntax::ast::TypeExpr,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Param(
        graphcal_syntax::ast::TypeExpr,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Node(
        graphcal_syntax::ast::TypeExpr,
        graphcal_syntax::ast::Expr,
        graphcal_syntax::span::Span,
    ),
    Fn(graphcal_syntax::ast::FnDecl, graphcal_syntax::span::Span),
}

/// Find a declaration by name in a file's AST.
fn find_declaration_in_file(file: &graphcal_syntax::ast::File, name: &str) -> Option<ImportedDecl> {
    for decl in &file.declarations {
        match &decl.kind {
            DeclKind::Const(c) if c.name.value.as_str() == name => {
                return Some(ImportedDecl::Const(
                    c.type_ann.clone(),
                    c.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Param(p) if p.name.value.as_str() == name => {
                return Some(ImportedDecl::Param(
                    p.type_ann.clone(),
                    p.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Node(n) if n.name.value.as_str() == name => {
                return Some(ImportedDecl::Node(
                    n.type_ann.clone(),
                    n.value.clone(),
                    decl.span,
                ));
            }
            DeclKind::Fn(f) if f.name.value.as_str() == name => {
                return Some(ImportedDecl::Fn(f.clone(), decl.span));
            }
            _ => {}
        }
    }
    None
}

/// Resolve a struct field's declared type, handling generic type parameter substitution.
///
/// If the field's type annotation references a generic type parameter (e.g., `D` in
/// `Vec3<D: Dim, F: Type>`), the substitution map provides the concrete type.
/// Otherwise, falls back to direct registry resolution.
fn resolve_field_declared_type(
    field: &crate::registry::StructField,
    generic_sub: &HashMap<&str, &DeclaredType>,
    registry: &Registry,
) -> Option<DeclaredType> {
    // Check if the field type is a bare generic param reference (e.g., `D`)
    if let graphcal_syntax::ast::TypeExprKind::DimExpr(dim_expr) = &field.type_ann.kind
        && dim_expr.terms.len() == 1
        && dim_expr.terms[0].term.power.is_none()
    {
        let name = &dim_expr.terms[0].term.name.name;
        if let Some(concrete) = generic_sub.get(name.as_str()) {
            return Some((*concrete).clone());
        }
    }
    // Non-generic: resolve directly from the registry
    registry
        .resolve_type_expr(&field.type_ann)
        .map(DeclaredType::Scalar)
}

/// Convert a `RuntimeValue` to a `Value` using declared type info.
///
/// All scalar values start with `display_unit: None`. Call `attach_display_units()`
/// afterwards to populate display units from the source expression.
fn runtime_to_value(
    rv: &RuntimeValue,
    declared_type: Option<&DeclaredType>,
    registry: &Registry,
) -> Value {
    match rv {
        RuntimeValue::Scalar(si_value) => {
            let dimension = match declared_type {
                Some(DeclaredType::Scalar(d)) => d.clone(),
                _ => Dimension::dimensionless(),
            };
            Value::Scalar {
                si_value: *si_value,
                dimension,
                display_unit: None,
            }
        }
        RuntimeValue::Bool(b) => Value::Bool(*b),
        RuntimeValue::Int(i) => Value::Int(*i),
        RuntimeValue::Struct {
            type_name,
            variant,
            fields,
        } => {
            let type_def = registry.get_type(type_name.as_str());
            let variant_def = type_def.and_then(|td| td.get_variant(variant.as_str()));

            // Build a substitution map from generic param names to concrete DeclaredTypes
            // when we have concrete type args from the declared type.
            let generic_sub: HashMap<&str, &DeclaredType> =
                if let (Some(td), Some(DeclaredType::Struct(_, type_args))) =
                    (type_def, declared_type)
                {
                    td.generic_params
                        .iter()
                        .zip(type_args.iter())
                        .map(|(param, arg)| (param.name.as_str(), arg))
                        .collect()
                } else {
                    HashMap::new()
                };

            let converted_fields = fields
                .iter()
                .map(|(field_name, field_rv)| {
                    let field_declared = variant_def.and_then(|vd| {
                        vd.fields
                            .iter()
                            .find(|f| f.name == *field_name)
                            .and_then(|f| resolve_field_declared_type(f, &generic_sub, registry))
                    });
                    let val = runtime_to_value(field_rv, field_declared.as_ref(), registry);
                    (field_name.clone(), val)
                })
                .collect();
            Value::Struct {
                type_name: type_name.clone(),
                variant: variant.clone(),
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
            // For range indexes, replace synthetic #N keys with formatted display values.
            let idx_def = registry.get_index(index_name.as_str());
            let converted_entries = entries
                .iter()
                .enumerate()
                .map(|(i, (variant, entry_rv))| {
                    let display_key = match idx_def {
                        Some(def) if def.is_range() => VariantName::new(format_range_step(def, i)),
                        _ => variant.clone(),
                    };
                    let val = runtime_to_value(entry_rv, element_declared, registry);
                    (display_key, val)
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
        RuntimeValue::RangeLabel { .. } => {
            panic!("RangeLabel should not appear in final values")
        }
    }
}

/// Evaluate an `Unfold` expression: `unfold(init, |prev_i, i| body)`.
///
/// This builds up results incrementally over a range index, inserting partial
/// results into `values` so that `@self_name[prev_i]` resolves correctly.
#[expect(
    clippy::too_many_arguments,
    reason = "evaluation context requires many parameters"
)]
#[expect(
    clippy::needless_range_loop,
    reason = "loop index i is used for step_value(i), step_index fields, and variant indexing"
)]
fn eval_unfold(
    self_name: &str,
    init: &graphcal_syntax::ast::Expr,
    prev_name: &graphcal_syntax::ast::Ident,
    curr_name: &graphcal_syntax::ast::Ident,
    body: &graphcal_syntax::ast::Expr,
    values: &mut HashMap<String, RuntimeValue>,
    builtin_consts: &HashMap<&str, f64>,
    builtin_fns: &HashMap<&str, crate::builtins::BuiltinFunction>,
    registry: &Registry,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> Result<RuntimeValue, GraphcalError> {
    // Find the range index from the node's declared type
    let declared = declared_types
        .get(self_name)
        .ok_or_else(|| GraphcalError::EvalError {
            message: format!("no declared type for node `{self_name}`"),
            src: src.clone(),
            span: (0, 0).into(),
        })?;
    let index_name = match declared {
        DeclaredType::Indexed { index, .. } => index.clone(),
        _ => {
            return Err(GraphcalError::EvalError {
                message: format!("node `{self_name}` must have an indexed type for time scan"),
                src: src.clone(),
                span: (0, 0).into(),
            });
        }
    };
    let idx_def =
        registry
            .get_index(index_name.as_str())
            .ok_or_else(|| GraphcalError::EvalError {
                message: format!("unknown index `{index_name}`"),
                src: src.clone(),
                span: (0, 0).into(),
            })?;

    let step_count = idx_def.step_count();
    let variants = idx_def.variants();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    // Evaluate init expression
    let init_val = eval_expr(
        init,
        values,
        &empty_locals,
        builtin_consts,
        builtin_fns,
        registry,
        src,
    )?;

    // Build results incrementally
    let mut result_entries: IndexMap<VariantName, RuntimeValue> = IndexMap::new();

    // Step 0: init value
    result_entries.insert(variants[0].clone(), init_val);

    // Steps 1..N: evaluate body with prev_t and t bindings
    for i in 1..step_count {
        // Insert partial result so @self[prev_t] can resolve
        values.insert(
            self_name.to_string(),
            RuntimeValue::Indexed {
                index_name: index_name.clone(),
                entries: result_entries.clone(),
            },
        );

        let prev_value = idx_def.step_value(i - 1);
        let curr_value = idx_def.step_value(i);

        let mut scan_locals = HashMap::new();
        scan_locals.insert(
            prev_name.name.clone(),
            RuntimeValue::RangeLabel {
                index_name: index_name.clone(),
                step_index: i - 1,
                value: prev_value,
            },
        );
        scan_locals.insert(
            curr_name.name.clone(),
            RuntimeValue::RangeLabel {
                index_name: index_name.clone(),
                step_index: i,
                value: curr_value,
            },
        );

        let body_val = eval_expr(
            body,
            values,
            &scan_locals,
            builtin_consts,
            builtin_fns,
            registry,
            src,
        )?;
        result_entries.insert(variants[i].clone(), body_val);
    }

    // Remove the partial value we inserted
    values.remove(self_name);

    Ok(RuntimeValue::Indexed {
        index_name,
        entries: result_entries,
    })
}

/// Evaluate using TIR + `ExecPlan` (new linear pipeline).
///
/// Runtime errors are contained per-node: if a node fails, independent nodes
/// still evaluate, and dependent nodes receive a `DependencyFailed` error.
#[expect(
    clippy::too_many_lines,
    reason = "linear evaluation pipeline is clearest as a single function"
)]
fn evaluate_plan(
    tir: &crate::tir::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<String, crate::dim_check::DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> EvalResult {
    let builtin_consts = builtin_constants();
    let builtin_fns = builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut values: HashMap<String, RuntimeValue> = HashMap::new();
    let mut errors: HashMap<String, NodeError> = HashMap::new();

    // Insert const values into the lookup table
    for (name, val) in &plan.const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order (params first, then nodes that depend on them)
    for name in &plan.topo_order {
        if values.contains_key(name) {
            continue;
        }

        // Check if any dependency has failed
        let failed_deps: Vec<DeclName> = tir
            .runtime_deps
            .get(name)
            .map(|deps| {
                deps.iter()
                    .filter(|dep| errors.contains_key(*dep))
                    .map(DeclName::new)
                    .collect()
            })
            .unwrap_or_default();

        if !failed_deps.is_empty() {
            errors.insert(name.clone(), NodeError::DependencyFailed { failed_deps });
            continue;
        }

        let expr = &plan.expressions[name];

        // Unfold requires special handling: it needs to build up results
        // incrementally and insert partial results into `values` so that
        // @self[prev_i] can resolve during body evaluation.
        let result = if let ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } = &expr.kind
        {
            eval_unfold(
                name,
                init,
                prev_name,
                curr_name,
                body,
                &mut values,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                declared_types,
                src,
            )
        } else {
            eval_expr(
                expr,
                &values,
                &empty_locals,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                src,
            )
        };

        match result {
            Ok(val) => {
                values.insert(name.clone(), val);
            }
            Err(e) => {
                let message = match &e {
                    GraphcalError::EvalError { message, .. } => message.clone(),
                    other => format!("{other}"),
                };
                errors.insert(name.clone(), NodeError::EvalFailed { message });
            }
        }
    }

    // Build a map from name -> expression for display unit extraction
    let expr_map: HashMap<&str, &graphcal_syntax::ast::Expr> = tir
        .consts
        .iter()
        .chain(tir.params.iter())
        .chain(tir.nodes.iter())
        .map(|(name, _, expr, _)| (name.as_str(), expr))
        .collect();

    let make_value = |name: &str, rv: &RuntimeValue| -> Value {
        let mut value = runtime_to_value(rv, declared_types.get(name), &tir.registry);
        if let Some(expr) = expr_map.get(name) {
            attach_display_units(&mut value, expr, &tir.registry);
        }
        value
    };

    let make_result = |name: &str| -> Result<Value, NodeError> {
        errors.get(name).map_or_else(
            || Ok(make_value(name, &values[name])),
            |err| Err(err.clone()),
        )
    };

    let consts = tir
        .consts
        .iter()
        .map(|(name, _, _, _)| {
            let val = make_value(name, &plan.const_values[name]);
            (DeclName::new(name), val)
        })
        .collect();
    let params = tir
        .params
        .iter()
        .map(|(name, _, _, _)| (DeclName::new(name), make_result(name)))
        .collect();
    let nodes = tir
        .nodes
        .iter()
        .map(|(name, _, _, _)| (DeclName::new(name), make_result(name)))
        .collect();

    let all = tir
        .source_order
        .iter()
        .map(|(name, cat)| {
            let decl_type = match cat {
                DeclCategory::Const => DeclType::Const,
                DeclCategory::Param => DeclType::Param,
                DeclCategory::Node => DeclType::Node,
            };
            let result = match cat {
                DeclCategory::Const => Ok(make_value(name, &plan.const_values[name])),
                DeclCategory::Param | DeclCategory::Node => make_result(name),
            };
            (DeclName::new(name), result, decl_type)
        })
        .collect();

    EvalResult {
        consts,
        params,
        nodes,
        all,
        base_dim_symbols: tir.registry.base_dim_symbols().clone(),
    }
}

/// Walk a `Value` tree alongside its source `Expr`, attaching display units
/// from unit literals and explicit `->` conversions to leaf `Scalar` nodes.
///
/// Display units are preserved from value literals (e.g., `6878.0 km`) and
/// explicit conversions (e.g., `@x -> km`). All other expressions (references,
/// arithmetic, field access, etc.) leave the display unit as `None`, so values
/// display in SI base units.
fn attach_display_units(value: &mut Value, expr: &graphcal_syntax::ast::Expr, registry: &Registry) {
    match (&mut *value, &expr.kind) {
        (Value::Scalar { display_unit, .. }, ExprKind::UnitLiteral { unit, .. }) => {
            *display_unit = resolve_unit_to_display(unit, registry);
        }
        (Value::Scalar { display_unit, .. }, ExprKind::Convert { target, .. }) => {
            *display_unit = resolve_unit_to_display(target, registry);
        }
        // Struct construction: recurse into each field initializer
        (Value::Struct { fields, .. }, ExprKind::StructConstruction { fields: inits, .. }) => {
            for init in inits {
                if let Some(field_val) = fields.get_mut(&init.name.value)
                    && let Some(init_expr) = &init.value
                {
                    attach_display_units(field_val, init_expr, registry);
                }
            }
        }
        // Map literal: recurse into each entry
        (
            Value::Indexed { entries, .. },
            ExprKind::MapLiteral {
                entries: map_entries,
            },
        ) => {
            for map_entry in map_entries {
                if let Some(entry_val) = entries.get_mut(&map_entry.variant.value) {
                    attach_display_units(entry_val, &map_entry.value, registry);
                }
            }
        }
        // For comprehension: extract a single display unit from body, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::ForComp { body, .. }) => {
            if let Some(du) = extract_flat_display_unit(body, registry) {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // Scan: extract a single display unit from init, apply uniformly
        (Value::Indexed { entries, .. }, ExprKind::Scan { init, .. })
        | (Value::Indexed { entries, .. }, ExprKind::Unfold { init, .. }) => {
            if let Some(du) = extract_flat_display_unit(init, registry) {
                for entry_val in entries.values_mut() {
                    set_scalar_display_unit(entry_val, &du);
                }
            }
        }
        // All other combinations: no display unit to attach
        _ => {}
    }
}

/// Resolve a `UnitExpr` to a `DisplayUnit`.
fn resolve_unit_to_display(
    unit: &graphcal_syntax::ast::UnitExpr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    let (_dim, scale) = registry.resolve_unit_expr(unit)?;
    Some(DisplayUnit {
        label: format_unit_expr(unit),
        scale,
    })
}

/// Extract a single display unit from a scalar-producing expression.
///
/// Used for indexed collections (for comprehensions, scan) where all entries
/// share the same display unit.
fn extract_flat_display_unit(
    expr: &graphcal_syntax::ast::Expr,
    registry: &Registry,
) -> Option<DisplayUnit> {
    match &expr.kind {
        ExprKind::UnitLiteral { unit, .. } => resolve_unit_to_display(unit, registry),
        ExprKind::Convert { target, .. } => resolve_unit_to_display(target, registry),
        ExprKind::MapLiteral { entries } => entries
            .first()
            .and_then(|e| extract_flat_display_unit(&e.value, registry)),
        ExprKind::ForComp { body, .. } => extract_flat_display_unit(body, registry),
        ExprKind::Scan { init, .. } | ExprKind::Unfold { init, .. } => {
            extract_flat_display_unit(init, registry)
        }
        _ => None,
    }
}

/// Format a range index step value for display, e.g. `"0 s"`, `"0.25 s"`.
fn format_range_step(idx_def: &crate::registry::IndexDef, step_index: usize) -> String {
    let si_value = idx_def.step_value(step_index);
    if let crate::registry::IndexKind::Range {
        display_label,
        display_scale,
        ..
    } = &idx_def.kind
    {
        let display_value = si_value / display_scale;
        let formatted = format_step_number(display_value);
        match display_label {
            Some(label) => format!("{formatted} {label}"),
            None => formatted,
        }
    } else {
        format!("#{step_index}")
    }
}

/// Format a numeric value for display in range index labels.
fn format_step_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "value.abs() < 1e15 guarantees it fits in i64"
        )]
        let int_val = value as i64;
        format!("{int_val}")
    } else {
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

/// Set display unit on a scalar value. No-op for non-scalar values.
fn set_scalar_display_unit(value: &mut Value, du: &DisplayUnit) {
    if let Value::Scalar { display_unit, .. } = value {
        *display_unit = Some(du.clone());
    }
}

/// Format a `UnitExpr` as a human-readable label.
/// E.g., `m`, `km/hour`, `kg * m / s^2`
pub(crate) fn format_unit_expr(expr: &graphcal_syntax::ast::UnitExpr) -> String {
    use graphcal_syntax::ast::MulDivOp;

    let mut numerator = Vec::new();
    let mut denominator = Vec::new();

    for item in &expr.terms {
        let mut part = item.name.value.to_string();
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
        // Check consts first (they are not wrapped in Result)
        if let Some((_, val)) = result.consts.iter().find(|(n, _)| n.as_str() == name) {
            return val.si_value();
        }
        // Check params and nodes (wrapped in Result)
        result
            .params
            .iter()
            .chain(result.nodes.iter())
            .find(|(n, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
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
        assert_eq!(result.params[0].0.as_str(), "b");
        assert_eq!(result.params[1].0.as_str(), "a");
        assert_eq!(result.nodes[0].0.as_str(), "z");
        assert_eq!(result.nodes[1].0.as_str(), "y");
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
        let speed_kmh = result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "speed_kmh")
            .unwrap();
        let speed_kmh_val = speed_kmh.1.as_ref().unwrap();
        assert_eq!(
            speed_kmh_val.display_label(&result.base_dim_symbols),
            Some("km/hour".to_string())
        );
        let display_kmh = speed_kmh_val.display_value();
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
        let tof_entry = result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "tof_hours")
            .unwrap();
        let tof_val = tof_entry.1.as_ref().unwrap();
        assert_eq!(
            tof_val.display_label(&result.base_dim_symbols),
            Some("hour".to_string())
        );
        let tof_display = tof_val.display_value();
        assert!(
            tof_display > 5.0 && tof_display < 6.0,
            "tof display = {tof_display} hours"
        );

        // Check that transfer node is a struct
        let transfer_entry = result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "transfer")
            .unwrap();
        match transfer_entry.1.as_ref().unwrap() {
            Value::Struct {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.as_str(), "TransferResult");
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
    fn eval_generics_milestone() {
        let source = include_str!("../../../tests/fixtures/generics.gcl");
        let result = compile_and_eval(source).unwrap();

        // x_pos: field access on Vec3<Length, Eci>, should be 6878 km = 6878000 m
        let x_pos = find_value(&result, "x_pos");
        assert!((x_pos - 6_878_000.0).abs() < 1.0, "x_pos = {x_pos}");

        // y_vel: field access on Vec3<Velocity, Eci>, should be 7.67 km/s = 7670 m/s
        let y_vel = find_value(&result, "y_vel");
        assert!((y_vel - 7670.0).abs() < 1.0, "y_vel = {y_vel}");

        // pos3_eci_x: explicit type args, 100 km = 100000 m
        let pos3_eci_x = find_value(&result, "pos3_eci_x");
        assert!(
            (pos3_eci_x - 100_000.0).abs() < 1.0,
            "pos3_eci_x = {pos3_eci_x}"
        );

        // pos3_default_y: default type param (F = Unframed), 20 km = 20000 m
        let pos3_default_y = find_value(&result, "pos3_default_y");
        assert!(
            (pos3_default_y - 20_000.0).abs() < 1.0,
            "pos3_default_y = {pos3_default_y}"
        );

        // dv_sum_x: derive(Add), 100 + 10 = 110 m/s
        let dv_sum_x = find_value(&result, "dv_sum_x");
        assert!((dv_sum_x - 110.0).abs() < 0.01, "dv_sum_x = {dv_sum_x}");

        // dv_diff_y: derive(Sub), 200 - 20 = 180 m/s
        let dv_diff_y = find_value(&result, "dv_diff_y");
        assert!((dv_diff_y - 180.0).abs() < 0.01, "dv_diff_y = {dv_diff_y}");

        // dv_neg_z: derive(Neg), -(300 m/s) = -300 m/s
        let dv_neg_z = find_value(&result, "dv_neg_z");
        assert!((dv_neg_z - (-300.0)).abs() < 0.01, "dv_neg_z = {dv_neg_z}");

        // pos_body_x: as cast (phantom only), same value as pos_eci.x = 6878 km = 6878000 m
        let pos_body_x = find_value(&result, "pos_body_x");
        assert!(
            (pos_body_x - 6_878_000.0).abs() < 1.0,
            "pos_body_x = {pos_body_x}"
        );

        // total_dv: non-generic struct still works, 100 + 200 = 300 m/s
        let total_dv = find_value(&result, "total_dv");
        assert!((total_dv - 300.0).abs() < 0.01, "total_dv = {total_dv}");
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
        let transfer_entry = result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "transfer")
            .unwrap();
        match transfer_entry.1.as_ref().unwrap() {
            Value::Struct {
                type_name, fields, ..
            } => {
                assert_eq!(type_name.as_str(), "TransferResult");
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
            .find(|(n, _, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"))
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
        let result = compile_and_eval("param x: Length = 100.0 m;\nnode y: Length = -@x;").unwrap();
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
        overrides.insert(DeclName::new("isp"), parse_expr("450.0 s"));
        let overridden = compile_and_eval_with_overrides(source, "test", &overrides).unwrap();
        let new_dv = find_value(&overridden, "delta_v");

        assert!(new_dv > default_dv, "higher isp should give higher delta_v");
    }

    #[test]
    fn override_with_wrong_dimension_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        // isp expects Time, not Mass
        let mut overrides = HashMap::new();
        overrides.insert(DeclName::new("isp"), parse_expr("450.0 kg"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        assert!(result.is_err());
    }

    #[test]
    fn override_node_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert(DeclName::new("delta_v"), parse_expr("100.0 m/s"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
                assert_eq!(name.as_str(), "delta_v");
                assert_eq!(actual_kind, "node");
            }
            other => panic!("expected OverrideNotAParam, got {other:?}"),
        }
    }

    #[test]
    fn override_const_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert(DeclName::new("G0"), parse_expr("10.0 m/s^2"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideNotAParam { name, actual_kind })) => {
                assert_eq!(name.as_str(), "G0");
                assert_eq!(actual_kind, "const");
            }
            other => panic!("expected OverrideNotAParam, got {other:?}"),
        }
    }

    #[test]
    fn override_unknown_param_errors() {
        let source = include_str!("../../../tests/fixtures/rocket.gcl");
        let mut overrides = HashMap::new();
        overrides.insert(DeclName::new("nonexistent"), parse_expr("100"));
        let result = compile_and_eval_with_overrides(source, "test", &overrides);
        match result {
            Err(CompileError::Eval(GraphcalError::OverrideUnknownParam { name })) => {
                assert_eq!(name.as_str(), "nonexistent");
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

    #[test]
    fn project_import_alias() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/alias/main.gcl");
        let result = compile_and_eval_project(&root, &HashMap::new()).unwrap();
        let y = find_value(&result, "y");
        assert!((y - 43.0).abs() < f64::EPSILON, "y = {y}, expected 43.0");
    }

    #[test]
    fn project_import_alias_conflict_resolution() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/multi/alias_conflict/main.gcl");
        let result = compile_and_eval_project(&root, &HashMap::new()).unwrap();
        let sum = find_value(&result, "sum");
        assert!(
            (sum - 3.0).abs() < f64::EPSILON,
            "sum = {sum}, expected 3.0"
        );
    }

    // --- Runtime arithmetic error tests ---

    /// Helper: assert that a specific node in the result has a `NodeError::EvalFailed`
    /// whose message contains `needle`.
    fn assert_node_error(source: &str, node_name: &str, needle: &str) {
        let result = compile_and_eval(source).unwrap();
        let (_, node_result, _) = result
            .all
            .iter()
            .find(|(n, _, _)| n.as_str() == node_name)
            .unwrap_or_else(|| panic!("node `{node_name}` not found"));
        match node_result {
            Err(NodeError::EvalFailed { message }) => {
                assert!(
                    message.contains(needle),
                    "expected error containing {needle:?}, got {message:?}"
                );
            }
            Err(other) => panic!("expected EvalFailed containing {needle:?}, got {other:?}"),
            Ok(val) => panic!("expected error for `{node_name}`, got value {val:?}"),
        }
    }

    #[test]
    fn eval_division_by_zero() {
        assert_node_error(
            "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;",
            "y",
            "division by zero",
        );
    }

    #[test]
    fn eval_zero_divided_by_zero() {
        assert_node_error(
            "param x: Dimensionless = 0.0;\nnode y: Dimensionless = @x / 0.0;",
            "y",
            "division by zero",
        );
    }

    #[test]
    fn eval_sqrt_negative() {
        assert_node_error("node y: Dimensionless = sqrt(-1.0);", "y", "NaN");
    }

    #[test]
    fn eval_ln_zero() {
        assert_node_error("node y: Dimensionless = ln(0.0);", "y", "infinite");
    }

    #[test]
    fn eval_ln_negative() {
        assert_node_error("node y: Dimensionless = ln(-1.0);", "y", "NaN");
    }

    #[test]
    fn eval_exp_overflow() {
        assert_node_error("node y: Dimensionless = exp(1000.0);", "y", "infinite");
    }

    #[test]
    fn eval_power_negative_base_frac_exp() {
        assert_node_error("node y: Dimensionless = (-1.0) ^ 0.5;", "y", "NaN");
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

    // --- Error containment tests ---

    #[test]
    fn eval_error_does_not_block_independent_nodes() {
        let result = compile_and_eval(
            "param x: Dimensionless = 1.0;\n\
             node bad: Dimensionless = @x / 0.0;\n\
             node good: Dimensionless = @x + 1.0;",
        )
        .unwrap();
        // bad should have an error
        assert!(
            result
                .nodes
                .iter()
                .find(|(n, _)| n.as_str() == "bad")
                .unwrap()
                .1
                .is_err()
        );
        // good should succeed because it does not depend on bad
        assert!((find_value(&result, "good") - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn eval_error_propagates_to_dependents() {
        let result = compile_and_eval(
            "param x: Dimensionless = 1.0;\n\
             node bad: Dimensionless = @x / 0.0;\n\
             node downstream: Dimensionless = @bad + 1.0;",
        )
        .unwrap();
        // bad fails with EvalFailed
        let bad_result = &result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "bad")
            .unwrap()
            .1;
        assert!(matches!(bad_result, Err(NodeError::EvalFailed { .. })));
        // downstream fails with DependencyFailed
        let ds_result = &result
            .nodes
            .iter()
            .find(|(n, _)| n.as_str() == "downstream")
            .unwrap()
            .1;
        assert!(matches!(ds_result, Err(NodeError::DependencyFailed { .. })));
    }

    #[test]
    fn eval_has_errors_true_when_node_fails() {
        let result =
            compile_and_eval("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x / 0.0;")
                .unwrap();
        assert!(result.has_errors());
    }

    #[test]
    fn eval_has_errors_false_when_all_ok() {
        let result =
            compile_and_eval("param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;")
                .unwrap();
        assert!(!result.has_errors());
    }

    // --- Integer type tests ---

    /// Helper: find a named Int value.
    fn find_int_value(result: &EvalResult, name: &str) -> i64 {
        let val = result
            .all
            .iter()
            .find(|(n, _, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
        match val {
            Value::Int(i) => *i,
            other => panic!("expected Int for `{name}`, got {other:?}"),
        }
    }

    /// Helper: find a named Bool value.
    fn find_bool_value(result: &EvalResult, name: &str) -> bool {
        let val = result
            .all
            .iter()
            .find(|(n, _, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("value `{name}` not found"))
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("value `{name}` has error: {e}"));
        match val {
            Value::Bool(b) => *b,
            other => panic!("expected Bool for `{name}`, got {other:?}"),
        }
    }

    #[test]
    fn eval_integers_milestone() {
        let source = include_str!("../../../tests/fixtures/integers.gcl");
        let result = compile_and_eval(source).unwrap();

        assert_eq!(find_int_value(&result, "a"), 10);
        assert_eq!(find_int_value(&result, "b"), 3);
        assert_eq!(find_int_value(&result, "sum"), 13);
        assert_eq!(find_int_value(&result, "diff"), 7);
        assert_eq!(find_int_value(&result, "prod"), 30);
        assert_eq!(find_int_value(&result, "quot"), 3); // truncating division
        assert_eq!(find_int_value(&result, "rem"), 1);
        assert_eq!(find_int_value(&result, "power"), 9);
        assert_eq!(find_int_value(&result, "neg_a"), -10);

        assert!(find_bool_value(&result, "a_gt_b"));
        assert!(!find_bool_value(&result, "a_eq_b"));
        assert!(!find_bool_value(&result, "a_le_b"));

        assert_eq!(find_int_value(&result, "SEVEN"), 7);
        assert_eq!(find_int_value(&result, "clamped"), 7); // 10 > 7, so clamp to 7

        // to_float(10) = 10.0
        assert!((find_value(&result, "a_float") - 10.0).abs() < f64::EPSILON);
        // to_int(3.7) = 3 (truncating)
        assert_eq!(find_int_value(&result, "back_to_int"), 3);
    }

    #[test]
    fn eval_int_division_by_zero() {
        assert_node_error(
            "param x: Int = 10;\nnode y: Int = @x / 0;",
            "y",
            "integer division by zero",
        );
    }

    #[test]
    fn eval_int_modulo_by_zero() {
        assert_node_error(
            "param x: Int = 10;\nnode y: Int = @x % 0;",
            "y",
            "integer modulo by zero",
        );
    }

    #[test]
    fn eval_int_negative_exponent() {
        // `-1` is parsed as UnaryOp::Neg(Integer(1)), not a literal, so dim_check
        // rejects it as a non-literal exponent before the evaluator sees it.
        let err = compile_and_eval("param x: Int = 2;\nnode y: Int = @x ^ -1;");
        assert!(err.is_err());
    }

    #[test]
    fn eval_int_mixed_type_error() {
        // Int + Scalar should be a type error
        let err = compile_and_eval("param x: Int = 10;\nnode y: Dimensionless = @x + 1.0;");
        assert!(err.is_err());
    }

    #[test]
    fn eval_int_with_unit_parse_error() {
        // `10 km` should be a parse error
        let err = compile_and_eval("param x: Length = 10 km;");
        assert!(err.is_err());
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
                let r = compile_and_eval(&source).unwrap();
                let z_result = &r.all.iter()
                    .find(|(n, _, _)| n.as_str() == "z")
                    .unwrap().1;
                match z_result {
                    Ok(val) => {
                        let z = val.si_value();
                        prop_assert!(z.is_finite(), "division produced non-finite: {z}");
                    }
                    Err(NodeError::EvalFailed { message }) => {
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

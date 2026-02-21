//! Project-based compilation: loading multi-file projects, resolving qualified
//! references, lowering to IR, and applying parameter overrides.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_syntax::ast::{DeclKind, Expr, ExprKind};
use graphcal_syntax::names::{DeclName, FnName, Spanned};
use graphcal_syntax::span::Span;

use crate::dim_check::DeclaredType;
use crate::error::GraphcalError;
use crate::eval_expr::RuntimeValue;
use crate::registry::{Registry, RegistryBuilder};
use crate::resolve::{DeclCategory, ImportedNames, ImportedValueNames};

use super::runtime::evaluate_plan;
use super::types::{AssertResult, CompileError, EvalResult};

// ---------------------------------------------------------------------------
// Project-based compilation: `LoadedProject` → TIR / EvalResult
// ---------------------------------------------------------------------------

/// A qualified reference found during expression walking.
pub(super) struct QualifiedRef {
    module: String,
    module_span: Span,
    name: String,
    name_span: Span,
}

/// Walk an expression tree and collect all qualified references.
pub(super) fn collect_qualified_refs(expr: &Expr, refs: &mut Vec<QualifiedRef>) {
    match &expr.kind {
        ExprKind::QualifiedGraphRef { module, name }
        | ExprKind::QualifiedConstRef { module, name } => {
            refs.push(QualifiedRef {
                module: module.name.clone(),
                module_span: module.span,
                name: name.value.to_string(),
                name_span: name.span,
            });
        }
        ExprKind::QualifiedFnCall { module, name, args } => {
            refs.push(QualifiedRef {
                module: module.name.clone(),
                module_span: module.span,
                name: name.value.to_string(),
                name_span: name.span,
            });
            for arg in args {
                collect_qualified_refs(arg, refs);
            }
        }
        // Recurse into sub-expressions
        ExprKind::BinOp { lhs, rhs, .. } => {
            collect_qualified_refs(lhs, refs);
            collect_qualified_refs(rhs, refs);
        }
        ExprKind::UnaryOp { operand, .. } => collect_qualified_refs(operand, refs),
        ExprKind::FnCall { args, .. } => {
            for arg in args {
                collect_qualified_refs(arg, refs);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_qualified_refs(condition, refs);
            collect_qualified_refs(then_branch, refs);
            collect_qualified_refs(else_branch, refs);
        }
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            collect_qualified_refs(inner, refs);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                collect_qualified_refs(&stmt.value, refs);
            }
            collect_qualified_refs(expr, refs);
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            collect_qualified_refs(expr, refs);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &field.value {
                    collect_qualified_refs(val, refs);
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                collect_qualified_refs(&entry.value, refs);
            }
        }
        ExprKind::ForComp { body, .. } => collect_qualified_refs(body, refs),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            collect_qualified_refs(source, refs);
            collect_qualified_refs(init, refs);
            collect_qualified_refs(body, refs);
        }
        ExprKind::Unfold { init, body, .. } => {
            collect_qualified_refs(init, refs);
            collect_qualified_refs(body, refs);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_qualified_refs(scrutinee, refs);
            for arm in arms {
                collect_qualified_refs(&arm.body, refs);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
    }
}

/// Rewrite qualified references to flat names in-place.
///
/// Replaces `QualifiedGraphRef { module: "m", name: "x" }` with `GraphRef("m::x")`,
/// `QualifiedConstRef` with `ConstRef`, and `QualifiedFnCall` with `FnCall`.
#[expect(
    clippy::too_many_lines,
    reason = "single match over all ExprKind variants plus rewrite logic"
)]
pub(super) fn rewrite_qualified_refs(expr: &mut Expr) {
    // First, rewrite children recursively
    match &mut expr.kind {
        ExprKind::BinOp { lhs, rhs, .. } => {
            rewrite_qualified_refs(lhs);
            rewrite_qualified_refs(rhs);
        }
        ExprKind::UnaryOp { operand, .. } => rewrite_qualified_refs(operand),
        ExprKind::FnCall { args, .. } | ExprKind::QualifiedFnCall { args, .. } => {
            for arg in args {
                rewrite_qualified_refs(arg);
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            rewrite_qualified_refs(condition);
            rewrite_qualified_refs(then_branch);
            rewrite_qualified_refs(else_branch);
        }
        ExprKind::Convert { expr: inner, .. } | ExprKind::AsCast { expr: inner, .. } => {
            rewrite_qualified_refs(inner);
        }
        ExprKind::Block { stmts, expr } => {
            for stmt in stmts {
                rewrite_qualified_refs(&mut stmt.value);
            }
            rewrite_qualified_refs(expr);
        }
        ExprKind::FieldAccess { expr, .. } | ExprKind::IndexAccess { expr, .. } => {
            rewrite_qualified_refs(expr);
        }
        ExprKind::StructConstruction { fields, .. } => {
            for field in fields {
                if let Some(val) = &mut field.value {
                    rewrite_qualified_refs(val);
                }
            }
        }
        ExprKind::MapLiteral { entries } | ExprKind::TableLiteral { entries, .. } => {
            for entry in entries {
                rewrite_qualified_refs(&mut entry.value);
            }
        }
        ExprKind::ForComp { body, .. } => rewrite_qualified_refs(body),
        ExprKind::Scan {
            source, init, body, ..
        } => {
            rewrite_qualified_refs(source);
            rewrite_qualified_refs(init);
            rewrite_qualified_refs(body);
        }
        ExprKind::Unfold { init, body, .. } => {
            rewrite_qualified_refs(init);
            rewrite_qualified_refs(body);
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_qualified_refs(scrutinee);
            for arm in arms {
                rewrite_qualified_refs(&mut arm.body);
            }
        }
        ExprKind::Number(_)
        | ExprKind::Integer(_)
        | ExprKind::Bool(_)
        | ExprKind::UnitLiteral { .. }
        | ExprKind::GraphRef(_)
        | ExprKind::ConstRef(_)
        | ExprKind::QualifiedGraphRef { .. }
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::LocalRef(_)
        | ExprKind::VariantLiteral { .. } => {}
    }

    // Now rewrite the current node if it's a qualified ref.
    // For QualifiedFnCall we need to move args out, so we use mem::replace.
    match &expr.kind {
        ExprKind::QualifiedGraphRef { .. }
        | ExprKind::QualifiedConstRef { .. }
        | ExprKind::QualifiedFnCall { .. } => {}
        _ => return,
    }
    let old_kind = std::mem::replace(&mut expr.kind, ExprKind::Number(0.0));
    expr.kind = match old_kind {
        ExprKind::QualifiedGraphRef { module, name } => {
            let flat = DeclName::new(format!("{}::{}", module.name, name.value));
            ExprKind::GraphRef(Spanned {
                value: flat,
                span: name.span,
            })
        }
        ExprKind::QualifiedConstRef { module, name } => {
            let flat = DeclName::new(format!("{}::{}", module.name, name.value));
            ExprKind::ConstRef(Spanned {
                value: flat,
                span: name.span,
            })
        }
        ExprKind::QualifiedFnCall { module, name, args } => {
            let flat = FnName::new(format!("{}::{}", module.name, name.value));
            ExprKind::FnCall {
                name: Spanned {
                    value: flat,
                    span: name.span,
                },
                args,
            }
        }
        other => other,
    };
}

// ---------------------------------------------------------------------------
// Per-file evaluation types and pipeline
// ---------------------------------------------------------------------------

/// The result of evaluating a single file in the per-file pipeline.
struct EvaluatedFile {
    /// Evaluated runtime values (params + nodes): name → `RuntimeValue`.
    values: HashMap<String, RuntimeValue>,
    /// Evaluated const values: name → `RuntimeValue`.
    const_values: HashMap<String, RuntimeValue>,
    /// Declared types for all consts/params/nodes in this file.
    declared_types: HashMap<String, DeclaredType>,
    /// Assertion results from this file.
    assertions: Vec<(DeclName, AssertResult, Span)>,
    /// The file's frozen registry (for type-system import by downstream files).
    registry: Registry,
    /// Functions declared in this file: (name, decl, span).
    functions: Vec<(String, graphcal_syntax::ast::FnDecl, Span)>,
    /// Assert names declared in this file.
    assert_names: HashSet<String>,
}

/// Evaluate a project using per-file evaluation.
///
/// Each file is compiled and evaluated as an independent unit, in topological
/// order (dependencies first). Import declarations bind pre-evaluated values
/// from dependency files into the importing file's scope.
///
/// All assertions in all files are evaluated and aggregated.
#[expect(
    clippy::too_many_lines,
    reason = "per-file evaluation orchestration is a single logical pipeline"
)]
fn evaluate_project_perfile(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    // Pre-compute override routing: map each override name to the file that owns
    // the param. Walk root file's imports to find the owning file for each override.
    let override_targets = route_overrides_to_files(project, overrides)?;

    let mut evaluated_files: HashMap<PathBuf, EvaluatedFile> = HashMap::new();

    for file_path in &project.load_order {
        let loaded_file = &project.files[file_path];
        let file_src = &loaded_file.named_source;
        let file_dir = file_path.parent().unwrap_or_else(|| Path::new("."));

        // Build ImportedValueNames and imported_values from this file's import declarations.
        let mut imported_names = ImportedValueNames::default();
        let mut imported_values: HashMap<String, (RuntimeValue, DeclaredType)> = HashMap::new();
        // Track imported value categories for output (source order).
        let mut imported_source_order: Vec<(String, DeclCategory)> = Vec::new();
        // Track type-system declarations to import from dependency registries.
        let mut imported_type_system_names: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        // Module imports: module_name → (canonical_path, span).
        let mut module_map: HashMap<String, (PathBuf, Span)> = HashMap::new();
        // Track extra RegistryBuilder entries to merge from dependencies.
        let mut extra_registry_builders: Vec<&Registry> = Vec::new();

        for decl in &loaded_file.ast.declarations {
            if let DeclKind::Import(import_decl) = &decl.kind {
                let import_path = file_dir.join(&import_decl.path);
                let import_canonical = import_path.canonicalize().map_err(|_| {
                    CompileError::Eval(GraphcalError::ImportFileNotFound {
                        path: import_decl.path.clone(),
                        src: file_src.clone(),
                        span: import_decl.path_span.into(),
                    })
                })?;

                let dep = evaluated_files.get(&import_canonical).ok_or_else(|| {
                    CompileError::Eval(GraphcalError::EvalError {
                        message: format!(
                            "internal: dependency {} not yet evaluated",
                            import_canonical.display()
                        ),
                        src: file_src.clone(),
                        span: import_decl.path_span.into(),
                    })
                })?;

                match &import_decl.kind {
                    graphcal_syntax::ast::ImportKind::Selective(names) => {
                        for import_item in names {
                            let orig_name = &import_item.name.name;
                            let local_name = import_item.local_name().to_string();

                            // Check if it's a value (const/param/node) or type-system decl.
                            if let Some(rv) = dep.const_values.get(orig_name) {
                                imported_names
                                    .const_names
                                    .push((local_name.clone(), import_item.name.span));
                                let dt = dep.declared_types.get(orig_name).cloned().unwrap_or(
                                    DeclaredType::Scalar(
                                        graphcal_syntax::dimension::Dimension::dimensionless(),
                                    ),
                                );
                                imported_source_order
                                    .push((local_name.clone(), DeclCategory::Const));
                                imported_values.insert(local_name, (rv.clone(), dt));
                            } else if let Some(rv) = dep.values.get(orig_name) {
                                let span = import_item.name.span;
                                imported_names.param_names.push((local_name.clone(), span));
                                let dt = dep.declared_types.get(orig_name).cloned().unwrap_or(
                                    DeclaredType::Scalar(
                                        graphcal_syntax::dimension::Dimension::dimensionless(),
                                    ),
                                );
                                imported_source_order
                                    .push((local_name.clone(), DeclCategory::Param));
                                imported_values.insert(local_name, (rv.clone(), dt));
                            } else if let Some((_, fn_decl, fn_span)) =
                                dep.functions.iter().find(|(n, _, _)| n == orig_name)
                            {
                                imported_names.functions.push((
                                    local_name,
                                    fn_decl.clone(),
                                    *fn_span,
                                ));
                            } else if dep.assert_names.contains(orig_name) {
                                // Assert is already evaluated in the dep file.
                                // We just need to make the name visible for #[assumes].
                                imported_names
                                    .assert_names
                                    .push((local_name, import_item.name.span));
                            } else {
                                // Check if it's a type-system declaration in the dep's file.
                                let dep_loaded = &project.files[&import_canonical];
                                let found = find_declaration_in_file(&dep_loaded.ast, orig_name);
                                if found.is_some() {
                                    // Type-system declaration (dim/unit/index/type).
                                    imported_type_system_names
                                        .entry(import_canonical.clone())
                                        .or_default()
                                        .insert(orig_name.clone());
                                } else {
                                    return Err(CompileError::Eval(
                                        GraphcalError::ImportNameNotFound {
                                            name: orig_name.clone(),
                                            file_path: import_decl.path.clone(),
                                            src: file_src.clone(),
                                            span: import_item.name.span.into(),
                                        },
                                    ));
                                }
                            }
                        }
                    }
                    graphcal_syntax::ast::ImportKind::Module { alias } => {
                        let module_name = if let Some(alias_ident) = alias {
                            alias_ident.name.clone()
                        } else {
                            crate::loader::derive_module_name(&import_decl.path).map_err(
                                |stem| {
                                    CompileError::Eval(GraphcalError::InvalidModuleName {
                                        stem,
                                        src: file_src.clone(),
                                        span: import_decl.path_span.into(),
                                    })
                                },
                            )?
                        };
                        if let Some((_, first_span)) = module_map.get(&module_name) {
                            return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                                name: module_name,
                                src: file_src.clone(),
                                span: import_decl.path_span.into(),
                                first: (*first_span).into(),
                            }));
                        }
                        module_map.insert(
                            module_name.clone(),
                            (import_canonical.clone(), import_decl.path_span),
                        );

                        // Import all values under module::name prefix.
                        for (name, rv) in &dep.const_values {
                            let flat = format!("{module_name}::{name}");
                            imported_names
                                .const_names
                                .push((flat.clone(), import_decl.path_span));
                            let dt = dep.declared_types.get(name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_source_order.push((flat.clone(), DeclCategory::Const));
                            imported_values.insert(flat, (rv.clone(), dt));
                        }
                        for (name, rv) in &dep.values {
                            let flat = format!("{module_name}::{name}");
                            imported_names
                                .param_names
                                .push((flat.clone(), import_decl.path_span));
                            let dt = dep.declared_types.get(name).cloned().unwrap_or(
                                DeclaredType::Scalar(
                                    graphcal_syntax::dimension::Dimension::dimensionless(),
                                ),
                            );
                            imported_source_order.push((flat.clone(), DeclCategory::Param));
                            imported_values.insert(flat, (rv.clone(), dt));
                        }
                        for (name, fn_decl, fn_span) in &dep.functions {
                            let flat = format!("{module_name}::{name}");
                            imported_names
                                .functions
                                .push((flat, fn_decl.clone(), *fn_span));
                        }
                        // Import all type-system declarations from dep's registry.
                        extra_registry_builders.push(&dep.registry);
                    }
                }
            }
        }

        // For module imports, resolve qualified references in expressions.
        let file_ast = if module_map.is_empty() {
            std::borrow::Cow::Borrowed(&loaded_file.ast)
        } else {
            let mut ast = loaded_file.ast.clone();
            for decl in &mut ast.declarations {
                match &mut decl.kind {
                    DeclKind::Const(c) => rewrite_qualified_refs(&mut c.value),
                    DeclKind::Param(p) => rewrite_qualified_refs(&mut p.value),
                    DeclKind::Node(n) => rewrite_qualified_refs(&mut n.value),
                    DeclKind::Assert(a) => match &mut a.body {
                        graphcal_syntax::ast::AssertBody::Expr(e) => rewrite_qualified_refs(e),
                        graphcal_syntax::ast::AssertBody::Tolerance {
                            actual,
                            expected,
                            tolerance,
                            ..
                        } => {
                            rewrite_qualified_refs(actual);
                            rewrite_qualified_refs(expected);
                            rewrite_qualified_refs(tolerance);
                        }
                    },
                    DeclKind::Fn(f) => match &mut f.body {
                        graphcal_syntax::ast::FnBody::Short(e) => rewrite_qualified_refs(e),
                        graphcal_syntax::ast::FnBody::Block { stmts, expr } => {
                            for stmt in stmts {
                                rewrite_qualified_refs(&mut stmt.value);
                            }
                            rewrite_qualified_refs(expr);
                        }
                    },
                    _ => {}
                }
            }
            std::borrow::Cow::Owned(ast)
        };

        // Lower to IR using per-file evaluation path.
        // For root files, we need the imported_values later for output enrichment.
        let is_root = *file_path == project.root;
        let imported_values_for_output = if is_root {
            Some(imported_values.clone())
        } else {
            None
        };

        let (mut builder, unfrozen) = crate::ir::lower_to_builder_with_imported_values(
            &file_ast,
            file_src,
            &imported_names,
            imported_values,
        )?;

        // Register type-system declarations from selectively imported files.
        for (dep_path, names) in &imported_type_system_names {
            let dep_loaded = &project.files[dep_path];
            crate::ir::register_selected_declarations(
                &dep_loaded.ast,
                &mut builder,
                &dep_loaded.named_source,
                names,
            )?;
        }

        // Merge type-system declarations from module-imported registries.
        for dep_registry in &extra_registry_builders {
            merge_registry_into_builder(&mut builder, dep_registry);
        }

        let ir = unfrozen.freeze(builder.build());

        // Apply overrides routed to this file (using original param names).
        let mut ir = ir;
        let file_overrides: HashMap<DeclName, graphcal_syntax::ast::Expr> = override_targets
            .iter()
            .filter(|(_, (target_path, _))| target_path == file_path)
            .map(|(name, (_, orig_name))| (orig_name.clone(), overrides[name].clone()))
            .collect();
        if !file_overrides.is_empty() {
            apply_overrides(&mut ir, &file_overrides)?;
        }

        // Type-resolve, check, build exec plan, evaluate.
        let tir = crate::tir::type_resolve(ir, file_src)?;
        crate::fn_check::check_no_recursion_tir(&tir, file_src)?;
        crate::dim_check::check_dimensions_tir(&tir, file_src)?;

        let declared_types = tir.build_declared_types(file_src)?;

        for (override_name, override_expr) in &file_overrides {
            crate::dim_check::check_override_dimension(
                override_expr,
                override_name.as_str(),
                &declared_types,
                &tir.registry,
                &tir.resolved_fn_sigs,
                file_src,
            )?;
        }

        let plan = crate::exec_plan::compile(&tir, file_src)?;
        let eval_result = evaluate_plan(&tir, &plan, &declared_types, file_src);

        if is_root {
            // Aggregate assertions from all dependency files.
            let mut all_assertions: Vec<(DeclName, AssertResult, Span)> = Vec::new();
            for dep_path in &project.load_order {
                if *dep_path == project.root {
                    continue;
                }
                if let Some(dep_eval) = evaluated_files.get(dep_path) {
                    all_assertions.extend(dep_eval.assertions.iter().cloned());
                }
            }
            all_assertions.extend(eval_result.assertions);

            // Prepend imported values to the output so they appear in the
            // result just like in the old single-IR approach.
            let mut all_consts = Vec::new();
            let mut all_params = Vec::new();
            let mut all_all = Vec::new();

            let root_imported_values = imported_values_for_output.unwrap_or_default();
            for (name, cat) in &imported_source_order {
                if let Some((rv, dt)) = root_imported_values.get(name) {
                    let value = super::runtime::runtime_to_value(rv, Some(dt), &tir.registry);
                    let decl_name = DeclName::new(name);
                    match cat {
                        DeclCategory::Const => {
                            all_consts.push((decl_name.clone(), value.clone()));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Const));
                        }
                        DeclCategory::Param => {
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Param));
                        }
                        DeclCategory::Node => {
                            // Imported nodes appear as params in the output.
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((decl_name, Ok(value), super::types::DeclType::Node));
                        }
                        DeclCategory::Assert => {}
                    }
                }
            }

            all_consts.extend(eval_result.consts);
            all_params.extend(eval_result.params);
            let all_nodes = eval_result.nodes;
            all_all.extend(eval_result.all);

            return Ok(EvalResult {
                consts: all_consts,
                params: all_params,
                nodes: all_nodes,
                all: all_all,
                assertions: all_assertions,
                assumes_map: eval_result.assumes_map,
                base_dim_symbols: eval_result.base_dim_symbols,
            });
        }

        // For non-root files, we need RuntimeValues to pass to downstream files.
        // Re-run the const evaluation and runtime evaluation to get the values map.
        let file_runtime_values = extract_runtime_values(&tir, &plan, &declared_types, file_src);

        // Store the evaluated file.
        evaluated_files.insert(
            file_path.clone(),
            EvaluatedFile {
                values: file_runtime_values,
                const_values: plan.const_values,
                declared_types,
                assertions: eval_result.assertions,
                registry: tir.registry,
                functions: tir.functions,
                assert_names: tir.assert_names,
            },
        );
    }

    // Should not reach here — root file should have returned above.
    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: NamedSource::new("internal", Arc::new(String::new())),
        span: (0, 0).into(),
    }))
}

/// Route `--set` / `--input` overrides to the files that own the targeted params.
///
/// Returns a map: `override_name` → (`owning_file_path`, `original_param_name`).
/// The `original_param_name` may differ from `override_name` when an alias is used.
fn route_overrides_to_files(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<HashMap<DeclName, (PathBuf, DeclName)>, CompileError> {
    if overrides.is_empty() {
        return Ok(HashMap::new());
    }

    let root_file = &project.files[&project.root];
    let root_dir = project.root.parent().unwrap_or_else(|| Path::new("."));
    let root_src = &root_file.named_source;

    let mut result: HashMap<DeclName, (PathBuf, DeclName)> = HashMap::new();

    for override_name in overrides.keys() {
        let name_str = override_name.as_str();

        // Check if the root file itself declares this param.
        let found_in_root =
            root_file.ast.declarations.iter().any(
                |d| matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == name_str),
            );
        if found_in_root {
            result.insert(
                override_name.clone(),
                (project.root.clone(), override_name.clone()),
            );
            continue;
        }

        // Check if the root file imports this param from a dependency.
        let mut found = false;
        for decl in &root_file.ast.declarations {
            if let DeclKind::Import(import_decl) = &decl.kind {
                if let graphcal_syntax::ast::ImportKind::Selective(names) = &import_decl.kind {
                    for item in names {
                        let local_name = item.local_name().to_string();
                        if local_name == name_str {
                            let orig_name = &item.name.name;
                            let import_path = root_dir.join(&import_decl.path);
                            let import_canonical = import_path.canonicalize().map_err(|_| {
                                CompileError::Eval(GraphcalError::ImportFileNotFound {
                                    path: import_decl.path.clone(),
                                    src: root_src.clone(),
                                    span: import_decl.path_span.into(),
                                })
                            })?;

                            // Verify it's actually a param in the source file.
                            let dep_file = &project.files[&import_canonical];
                            let is_param = dep_file.ast.declarations.iter().any(|d| {
                                matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                            });
                            if is_param {
                                result.insert(
                                    override_name.clone(),
                                    (import_canonical, DeclName::new(orig_name.clone())),
                                );
                                found = true;
                                break;
                            }
                        }
                    }
                }
                if found {
                    break;
                }
            }
        }

        if !found {
            // Check if the name matches a non-param declaration (node, const, assert)
            // in the root file to provide a better error message.
            for decl in &root_file.ast.declarations {
                let kind = match &decl.kind {
                    DeclKind::Node(n) if n.name.value.as_str() == name_str => Some("node"),
                    DeclKind::Const(c) if c.name.value.as_str() == name_str => Some("const"),
                    DeclKind::Assert(a) if a.name.value.as_str() == name_str => Some("assert"),
                    _ => None,
                };
                if let Some(actual_kind) = kind {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: actual_kind.to_string(),
                    }));
                }
            }
            return Err(CompileError::Eval(GraphcalError::OverrideUnknownParam {
                name: override_name.clone(),
            }));
        }
    }

    Ok(result)
}

/// Extract `RuntimeValue`s from a plan evaluation for passing to downstream files.
fn extract_runtime_values(
    tir: &crate::tir::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> HashMap<String, RuntimeValue> {
    let builtin_consts = crate::builtins::builtin_constants();
    let builtin_fns = crate::builtins::builtin_functions();
    let empty_locals: HashMap<String, RuntimeValue> = HashMap::new();

    let mut values: HashMap<String, RuntimeValue> = HashMap::new();

    // Insert imported values.
    for (name, val) in &plan.imported_values {
        values.insert(name.clone(), val.clone());
    }

    // Insert const values.
    for (name, val) in &plan.const_values {
        values.insert(name.clone(), val.clone());
    }

    // Evaluate in topological order.
    for name in &plan.topo_order {
        if values.contains_key(name) {
            continue;
        }

        // Check if any dependency has failed — skip if so.
        let has_failed_dep = tir
            .runtime_deps
            .get(name)
            .is_some_and(|deps| deps.iter().any(|dep| !values.contains_key(dep)));

        if has_failed_dep {
            continue;
        }

        let expr = &plan.expressions[name];

        let result = if let graphcal_syntax::ast::ExprKind::Unfold {
            init,
            prev_name,
            curr_name,
            body,
        } = &expr.kind
        {
            super::runtime::eval_unfold(
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
            crate::eval_expr::eval_expr(
                expr,
                &values,
                &empty_locals,
                &builtin_consts,
                &builtin_fns,
                &tir.registry,
                src,
            )
        };

        if let Ok(val) = result {
            values.insert(name.clone(), val);
        }
    }

    // Return only locally-defined param/node values (not imported, not consts).
    let local_runtime_names: HashSet<&str> = tir
        .params
        .iter()
        .chain(tir.nodes.iter())
        .map(|(n, _, _, _)| n.as_str())
        .collect();

    values
        .into_iter()
        .filter(|(name, _)| local_runtime_names.contains(name.as_str()))
        .collect()
}

/// Merge type-system declarations from a dependency's frozen Registry into a builder.
///
/// This imports dimensions, units, indexes, and struct types so that the
/// importing file can reference them.
fn merge_registry_into_builder(builder: &mut RegistryBuilder, dep_registry: &Registry) {
    // Import base dimension names (for display formatting).
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        builder.register_base_dimension(graphcal_syntax::names::DimName::new(name), id.clone());
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        builder.register_dimension(name.clone(), dim.clone());
    }

    // Import base dimension symbols (for SI unit string display).
    for (id, symbol) in dep_registry.dimensions.base_dim_symbols() {
        builder.set_base_dim_symbol(id.clone(), symbol.clone());
    }

    // Import units.
    for (name, dim, scale) in dep_registry.units.all_units() {
        builder.register_unit((*name).clone(), dim.clone(), *scale);
    }

    // Import indexes.
    for idx_def in dep_registry.indexes.all_indexes() {
        builder.register_index(idx_def.clone());
    }

    // Import struct types.
    for type_def in dep_registry.types.all_types() {
        builder.register_type(type_def.clone());
    }

    // Import functions.
    for fn_def in dep_registry.functions.all_functions() {
        builder.register_function(fn_def.clone());
    }
}

// ---------------------------------------------------------------------------
// Legacy project-based compilation (AST inlining) — still used for single-file
// and for compile_to_tir paths that don't need evaluation.
// ---------------------------------------------------------------------------

/// Resolve imports from `use` declarations and lower a project's root file to IR.
///
/// This is the shared first half of the compilation pipeline for both
/// `compile_to_tir_from_project` and `compile_and_eval_from_project`.
#[expect(
    clippy::too_many_lines,
    reason = "handles both selective and module import resolution in a single pass"
)]
pub(super) fn lower_project_to_ir(
    project: &crate::loader::LoadedProject,
) -> Result<(crate::ir::IR, NamedSource<Arc<String>>), CompileError> {
    let root_file = &project.files[&project.root];
    let root_src = &root_file.named_source;
    let root_dir = project.root.parent().unwrap_or_else(|| Path::new("."));

    // Collect imported names from imported files based on `use` statements.
    let mut imported = ImportedNames::default();
    // Track which type-system declarations (dims/units/indexes/types) are explicitly
    // imported from each file, so we only register those (not everything in the file).
    let mut imported_type_system_names: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    // Module imports: map module_name → (canonical_path, span_of_import_decl).
    let mut module_map: HashMap<String, (PathBuf, Span)> = HashMap::new();
    for decl in &root_file.ast.declarations {
        if let DeclKind::Import(import_decl) = &decl.kind {
            let import_path = root_dir.join(&import_decl.path);
            let import_canonical = import_path.canonicalize().map_err(|_| {
                CompileError::Eval(GraphcalError::ImportFileNotFound {
                    path: import_decl.path.clone(),
                    src: root_src.clone(),
                    span: import_decl.path_span.into(),
                })
            })?;

            let imported_file = &project.files[&import_canonical];

            let names = match &import_decl.kind {
                graphcal_syntax::ast::ImportKind::Selective(names) => names,
                graphcal_syntax::ast::ImportKind::Module { alias } => {
                    // Module imports: derive module name, store mapping for later resolution.
                    let module_name = if let Some(alias_ident) = alias {
                        alias_ident.name.clone()
                    } else {
                        crate::loader::derive_module_name(&import_decl.path).map_err(|stem| {
                            CompileError::Eval(GraphcalError::InvalidModuleName {
                                stem,
                                src: root_src.clone(),
                                span: import_decl.path_span.into(),
                            })
                        })?
                    };
                    if let Some((_, first_span)) = module_map.get(&module_name) {
                        return Err(CompileError::Eval(GraphcalError::DuplicateModuleName {
                            name: module_name,
                            src: root_src.clone(),
                            span: import_decl.path_span.into(),
                            first: (*first_span).into(),
                        }));
                    }
                    module_map.insert(
                        module_name,
                        (import_canonical.clone(), import_decl.path_span),
                    );
                    continue;
                }
            };
            for import_item in names {
                let found = find_declaration_in_file(&imported_file.ast, &import_item.name.name);
                let local_name = import_item.local_name().to_string();

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
                    Some(ImportedDecl::Assert(expr, span)) => {
                        imported.asserts.push((local_name, expr, span));
                    }
                    Some(ImportedDecl::TypeSystem) => {
                        imported_type_system_names
                            .entry(import_canonical.clone())
                            .or_default()
                            .insert(import_item.name.name.clone());
                    }
                    None => {
                        return Err(CompileError::Eval(GraphcalError::ImportNameNotFound {
                            name: import_item.name.name.clone(),
                            file_path: import_decl.path.clone(),
                            src: root_src.clone(),
                            span: import_item.name.span.into(),
                        }));
                    }
                }
            }
        }
    }

    // Resolve module-qualified references: walk root file expressions, look up
    // each `module::name` in the module's file, and import under flat names.
    if !module_map.is_empty() {
        let mut qualified_refs: Vec<QualifiedRef> = Vec::new();
        for decl in &root_file.ast.declarations {
            match &decl.kind {
                DeclKind::Const(c) => collect_qualified_refs(&c.value, &mut qualified_refs),
                DeclKind::Param(p) => collect_qualified_refs(&p.value, &mut qualified_refs),
                DeclKind::Node(n) => collect_qualified_refs(&n.value, &mut qualified_refs),
                DeclKind::Assert(a) => match &a.body {
                    graphcal_syntax::ast::AssertBody::Expr(e) => {
                        collect_qualified_refs(e, &mut qualified_refs);
                    }
                    graphcal_syntax::ast::AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => {
                        collect_qualified_refs(actual, &mut qualified_refs);
                        collect_qualified_refs(expected, &mut qualified_refs);
                        collect_qualified_refs(tolerance, &mut qualified_refs);
                    }
                },
                DeclKind::Fn(f) => match &f.body {
                    graphcal_syntax::ast::FnBody::Short(e) => {
                        collect_qualified_refs(e, &mut qualified_refs);
                    }
                    graphcal_syntax::ast::FnBody::Block { stmts, expr } => {
                        for stmt in stmts {
                            collect_qualified_refs(&stmt.value, &mut qualified_refs);
                        }
                        collect_qualified_refs(expr, &mut qualified_refs);
                    }
                },
                _ => {}
            }
        }

        // Deduplicate: track which (module, name) pairs we've already imported.
        let mut already_imported: HashSet<(String, String)> = HashSet::new();

        for qref in &qualified_refs {
            let (module_path, _) = module_map.get(&qref.module).ok_or_else(|| {
                CompileError::Eval(GraphcalError::UnknownModule {
                    name: qref.module.clone(),
                    src: root_src.clone(),
                    span: qref.module_span.into(),
                })
            })?;

            let key = (qref.module.clone(), qref.name.clone());
            if !already_imported.insert(key) {
                continue; // Already imported this (module, name) pair.
            }

            let module_file = &project.files[module_path];
            let flat_name = format!("{}::{}", qref.module, qref.name);

            let found = find_declaration_in_file(&module_file.ast, &qref.name);
            match found {
                Some(ImportedDecl::Const(type_ann, expr, span)) => {
                    imported.consts.push((flat_name, type_ann, expr, span));
                }
                Some(ImportedDecl::Param(type_ann, expr, span)) => {
                    imported.params.push((flat_name, type_ann, expr, span));
                }
                Some(ImportedDecl::Node(type_ann, expr, span)) => {
                    imported.nodes.push((flat_name, type_ann, expr, span));
                }
                Some(ImportedDecl::Fn(fn_decl, span)) => {
                    imported.functions.push((flat_name, fn_decl, span));
                }
                Some(ImportedDecl::Assert(body, span)) => {
                    imported.asserts.push((flat_name, body, span));
                }
                Some(ImportedDecl::TypeSystem) => {
                    imported_type_system_names
                        .entry(module_path.clone())
                        .or_default()
                        .insert(qref.name.clone());
                }
                None => {
                    return Err(CompileError::Eval(GraphcalError::QualifiedNameNotFound {
                        module: qref.module.clone(),
                        name: qref.name.clone(),
                        src: root_src.clone(),
                        span: qref.name_span.into(),
                    }));
                }
            }
        }
    }

    // Rewrite qualified references to flat names in the root AST before lowering.
    // This must happen before `lower_to_builder` because name resolution inside it
    // expects all references to use flat names (e.g. "constants::G0" not QualifiedConstRef).
    let root_ast = if module_map.is_empty() {
        std::borrow::Cow::Borrowed(&root_file.ast)
    } else {
        let mut ast = root_file.ast.clone();
        for decl in &mut ast.declarations {
            match &mut decl.kind {
                DeclKind::Const(c) => rewrite_qualified_refs(&mut c.value),
                DeclKind::Param(p) => rewrite_qualified_refs(&mut p.value),
                DeclKind::Node(n) => rewrite_qualified_refs(&mut n.value),
                DeclKind::Assert(a) => match &mut a.body {
                    graphcal_syntax::ast::AssertBody::Expr(e) => rewrite_qualified_refs(e),
                    graphcal_syntax::ast::AssertBody::Tolerance {
                        actual,
                        expected,
                        tolerance,
                        ..
                    } => {
                        rewrite_qualified_refs(actual);
                        rewrite_qualified_refs(expected);
                        rewrite_qualified_refs(tolerance);
                    }
                },
                DeclKind::Fn(f) => match &mut f.body {
                    graphcal_syntax::ast::FnBody::Short(e) => rewrite_qualified_refs(e),
                    graphcal_syntax::ast::FnBody::Block { stmts, expr } => {
                        for stmt in stmts {
                            rewrite_qualified_refs(&mut stmt.value);
                        }
                        rewrite_qualified_refs(expr);
                    }
                },
                _ => {}
            }
        }
        std::borrow::Cow::Owned(ast)
    };

    // Lower root AST → builder + unfrozen IR (includes root file declarations + functions)
    let (mut builder, unfrozen) = crate::ir::lower_to_builder(&root_ast, root_src, &imported)?;

    // Register only explicitly imported type-system declarations from imported files.
    for file_path in &project.load_order {
        if *file_path == project.root {
            continue;
        }
        if let Some(names) = imported_type_system_names.get(file_path) {
            let loaded = &project.files[file_path];
            crate::ir::register_selected_declarations(
                &loaded.ast,
                &mut builder,
                &loaded.named_source,
                names,
            )?;
        }
    }

    // Freeze the builder into an immutable registry and assemble the IR.
    let ir = unfrozen.freeze(builder.build());

    Ok((ir, root_src.clone()))
}

/// Validate and apply parameter overrides to an IR.
pub(super) fn apply_overrides(
    ir: &mut crate::ir::IR,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<(), CompileError> {
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
                DeclCategory::Assert => {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind: "assert".to_string(),
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
    Ok(())
}

/// Compile a [`LoadedProject`](crate::loader::LoadedProject) to TIR without evaluating.
///
/// Resolves imports from `use` declarations in the root file, lowers to IR,
/// type-resolves, and runs all checks (recursion, dimensions). The project may
/// have been loaded from disk, constructed from in-memory source, or a mix of
/// both (via [`LoadedProject::load_with_overlay`](crate::loader::LoadedProject::load_with_overlay)).
///
/// # Errors
///
/// Returns a [`CompileError`] if lowering, resolution, or checking fails.
pub fn compile_to_tir_from_project(
    project: &crate::loader::LoadedProject,
) -> Result<crate::tir::TIR, CompileError> {
    let (ir, root_src) = lower_project_to_ir(project)?;
    let tir = crate::tir::type_resolve(ir, &root_src)?;
    crate::fn_check::check_no_recursion_tir(&tir, &root_src)?;
    crate::dim_check::check_dimensions_tir(&tir, &root_src)?;
    Ok(tir)
}

/// Compile and evaluate a [`LoadedProject`](crate::loader::LoadedProject).
///
/// Uses per-file evaluation: each file is compiled and evaluated independently
/// in topological order. Import declarations bind pre-evaluated values from
/// dependency files. All assertions in all files are evaluated and aggregated.
///
/// # Errors
///
/// Returns a [`CompileError`] if any pipeline stage fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_from_project(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
) -> Result<EvalResult, CompileError> {
    evaluate_project_perfile(project, overrides)
}

// ---------------------------------------------------------------------------
// Convenience wrappers: existing public API, now delegating to project-based core
// ---------------------------------------------------------------------------

/// Full pipeline for multi-file projects with parameter overrides.
///
/// Loads all files referenced by `use` declarations starting from `root_path`,
/// collects imported declarations, and evaluates the root file with imports merged.
///
/// # Errors
///
/// Returns a [`CompileError`] if loading, parsing, resolution, or evaluation fails.
#[expect(
    clippy::implicit_hasher,
    reason = "public API accepts HashMap without requiring specific hasher"
)]
pub fn compile_and_eval_project(
    root_path: &Path,
    overrides: &HashMap<DeclName, graphcal_syntax::ast::Expr>,
    project_root: Option<&Path>,
) -> Result<EvalResult, CompileError> {
    let project = crate::loader::load_project(root_path, project_root)?;
    compile_and_eval_from_project(&project, overrides)
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
    let project = crate::loader::LoadedProject::from_source(source, name)?;
    compile_to_tir_from_project(&project)
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
    project_root: Option<&Path>,
) -> Result<(crate::tir::TIR, crate::loader::LoadedProject), CompileError> {
    let project = crate::loader::load_project(root_path, project_root)?;
    let tir = compile_to_tir_from_project(&project)?;
    Ok((tir, project))
}

/// A declaration found in a file, classified by kind.
pub(super) enum ImportedDecl {
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
    Assert(
        graphcal_syntax::ast::AssertBody,
        graphcal_syntax::span::Span,
    ),
    /// A type-system declaration (dimension, unit, index, or struct type).
    /// These are registered into the `Registry`, not into `ImportedNames`.
    TypeSystem,
}

pub(super) fn find_declaration_in_file(
    file: &graphcal_syntax::ast::File,
    name: &str,
) -> Option<ImportedDecl> {
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
            DeclKind::Assert(a) if a.name.value.as_str() == name => {
                return Some(ImportedDecl::Assert(a.body.clone(), decl.span));
            }
            DeclKind::Dimension(d) if d.name.value.as_str() == name => {
                return Some(ImportedDecl::TypeSystem);
            }
            DeclKind::Unit(u) if u.name.value.as_str() == name => {
                return Some(ImportedDecl::TypeSystem);
            }
            DeclKind::Index(idx) if idx.name.value.as_str() == name => {
                return Some(ImportedDecl::TypeSystem);
            }
            DeclKind::Type(t) if t.name.value.as_str() == name => {
                return Some(ImportedDecl::TypeSystem);
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
pub(super) fn resolve_field_declared_type(
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
        .dimensions
        .resolve_type_expr(&field.type_ann)
        .map(DeclaredType::Scalar)
}

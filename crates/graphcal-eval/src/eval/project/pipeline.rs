//! Orchestration: top-level compilation and evaluation pipelines for multi-file projects.

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "submodule of project/ uses parent types extensively"
)]
use super::*;

/// Compile a single file within a project, using pre-evaluated values from dependencies.
///
/// Builds import bindings, lowers to IR, applies overrides, and type-resolves to TIR.
/// Both [`evaluate_project_perfile`] and [`compile_to_tir_project_perfile`] call this
/// for each file in the project.
#[expect(
    clippy::too_many_lines,
    reason = "import processing, inline DAG handling, and cross-file DAG handling form a cohesive pipeline"
)]
pub(super) fn compile_single_file_in_project(
    project: &crate::loader::LoadedProject,
    file_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    evaluated_files: &HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    override_targets: &HashMap<DeclName, (graphcal_compiler::syntax::dag_id::DagId, DeclName)>,
) -> Result<CompiledFile, CompileError> {
    let loaded_file = &project.files[file_dag_id];
    let file_src = &loaded_file.named_source;

    let mut ctx = ImportContext {
        imported_names: ImportedValueNames::default(),
        imported_values: HashMap::new(),
        imported_source_order: Vec::new(),
        imported_type_system_names: HashMap::new(),
        module_map: HashMap::new(),
        extra_registry_builders: Vec::new(),
        deferred_instantiated: Vec::new(),
        deferred_inline_dags: Vec::new(),
    };

    // Collect inline DAG definitions from the file's AST.
    let dag_definitions: HashMap<String, &graphcal_compiler::syntax::ast::DagDecl> = loaded_file
        .ast
        .declarations
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Dag(dag) => Some((dag.name.value.to_string(), dag)),
            _ => None,
        })
        .collect();

    // Check for recursive DAG instantiation.
    imports::check_dag_recursion(&dag_definitions, file_src)?;

    // Process all import declarations (non-instantiated, compile-time items only).
    for (_decl, import_decl, import_canonical) in loaded_file.imports_with_dag_ids() {
        let import_canonical = import_canonical.clone();
        imports::process_non_instantiated_import(
            project,
            &import_canonical,
            &import_decl.path,
            &import_decl.kind,
            file_src,
            evaluated_files,
            &mut ctx,
            true, // is_import: enforce const-only
        )?;
    }

    // Process all include declarations (file-based DAG instantiation).
    // Inline DAG includes (single-segment module paths matching a dag name),
    // cross-file DAG includes, and bare module path DAG includes are handled below.
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_dag_ids() {
        // Skip cross-file DAG includes — handled in the next section.
        if include_decl.path.is_cross_file_dag() {
            continue;
        }
        // Skip bare module path DAG references — handled after inline DAGs.
        // These are multi-segment ModulePath includes where the last segment
        // matches a DAG in the resolved target file.
        if imports::is_bare_module_dag_ref(&include_decl.path, include_canonical, project) {
            continue;
        }
        let include_canonical = include_canonical.clone();
        if include_decl.param_bindings.is_empty() {
            imports::process_non_instantiated_import(
                project,
                &include_canonical,
                &include_decl.path,
                &include_decl.kind,
                file_src,
                evaluated_files,
                &mut ctx,
                false, // is_import: include allows runtime items
            )?;
        } else {
            imports::process_instantiated_include(
                project,
                file_dag_id,
                &include_canonical,
                include_decl,
                decl,
                file_src,
                evaluated_files,
                &mut ctx,
            )?;
        }
    }

    // Process inline DAG includes (include dag_name(...) { ... }).
    // These are includes with single-segment module paths that match inline DAG definitions.
    for decl in &loaded_file.ast.declarations {
        let DeclKind::Include(include_decl) = &decl.kind else {
            continue;
        };
        // Only handle single-segment module paths that match a DAG name.
        let dag_name = match &include_decl.path {
            graphcal_compiler::syntax::ast::ImportPath::ModulePath { segments, .. }
                if segments.len() == 1 =>
            {
                &segments[0].name
            }
            _ => continue,
        };
        let dag_def = match dag_definitions.get(dag_name.as_str()) {
            Some(dag) => *dag,
            None => continue, // Not an inline DAG — already handled by file-based includes
        };

        imports::process_inline_dag_include(
            dag_def,
            dag_name,
            include_decl,
            decl,
            &loaded_file.ast,
            file_src,
            &mut ctx,
            false, // same-file DAG
        )?;
    }

    // Process cross-file DAG includes (include "./file.gcl"/dag_name(...) { ... }).
    // These reference inline DAG definitions in other files.
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_dag_ids() {
        let ImportPath::CrossFileDag { dag_name, .. } = &include_decl.path else {
            continue;
        };

        let include_canonical = include_canonical.clone();

        // Find the target file's AST from the project.
        let target_loaded = project.files.get(&include_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "cross-file DAG target file not found in project: {include_canonical}",
                ),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        // Find the named DAG definition in the target file's AST.
        let target_dag_def = target_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Dag(dag) if dag.name.value.as_str() == dag_name.name => Some(dag),
                _ => None,
            })
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!(
                        "DAG `{}` not found in file `{include_canonical}`",
                        dag_name.name,
                    ),
                    src: file_src.clone(),
                    span: dag_name.span.into(),
                })
            })?;

        // Reuse the same inline DAG processing, but with the target file's AST
        // for parent scope resolution.
        imports::process_inline_dag_include(
            target_dag_def,
            &dag_name.name,
            include_decl,
            decl,
            &target_loaded.ast,
            file_src,
            &mut ctx,
            true, // cross-file DAG
        )?;
    }

    // Process bare module path DAG includes (include pkg/mod/dag_name(...) { ... }).
    // These are multi-segment ModulePath includes where the last segment is a DAG
    // defined in the resolved parent file (e.g. `pkg/mod.gcl` contains `dag dag_name`).
    for (decl, include_decl, include_canonical) in loaded_file.includes_with_dag_ids() {
        if !imports::is_bare_module_dag_ref(&include_decl.path, include_canonical, project) {
            continue;
        }

        let include_canonical = include_canonical.clone();
        let ImportPath::ModulePath { segments, .. } = &include_decl.path else {
            // is_bare_module_dag_ref only returns true for ModulePath
            continue;
        };
        // Safety: is_bare_module_dag_ref ensures segments.len() >= 2
        let Some(last_seg) = segments.last() else {
            continue;
        };
        let dag_name = &last_seg.name;

        // Find the target file's AST from the project.
        let target_loaded = project.files.get(&include_canonical).ok_or_else(|| {
            CompileError::Eval(GraphcalError::EvalError {
                message: format!(
                    "bare module DAG target file not found in project: {include_canonical}",
                ),
                src: file_src.clone(),
                span: include_decl.path.span().into(),
            })
        })?;

        // Find the named DAG definition in the target file's AST.
        let target_dag_def = target_loaded
            .ast
            .declarations
            .iter()
            .find_map(|d| match &d.kind {
                DeclKind::Dag(dag) if dag.name.value.as_str() == dag_name => Some(dag),
                _ => None,
            })
            .ok_or_else(|| {
                CompileError::Eval(GraphcalError::EvalError {
                    message: format!("DAG `{dag_name}` not found in file `{include_canonical}`"),
                    src: file_src.clone(),
                    span: include_decl.path.span().into(),
                })
            })?;

        // Reuse the same inline DAG processing, with the target file's AST
        // for parent scope resolution.
        imports::process_inline_dag_include(
            target_dag_def,
            dag_name,
            include_decl,
            decl,
            &target_loaded.ast,
            file_src,
            &mut ctx,
            true, // cross-file DAG
        )?;
    }

    // For module imports, resolve qualified references in expressions.
    let file_ast = rewrite_qualified_refs_in_ast(&loaded_file.ast, &ctx.module_map);

    // Lower to IR and finalize compilation.
    lowering::lower_and_finalize(
        project,
        file_dag_id,
        file_src,
        &file_ast,
        ctx,
        evaluated_files,
        overrides,
        override_targets,
    )
}

/// Evaluate and store a non-root file, producing an [`EvaluatedFile`] for downstream imports.
pub(super) fn evaluate_and_store_file(
    compiled: CompiledFile,
    file_dag_id: &graphcal_compiler::syntax::dag_id::DagId,
    file_src: &NamedSource<Arc<String>>,
    pub_names: HashSet<String>,
    evaluated_files: &mut HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile>,
) -> Result<(), CompileError> {
    let plan = crate::exec_plan::compile(&compiled.tir, file_src)?;
    let eval_result = evaluate_plan(&compiled.tir, &plan, &compiled.declared_types, file_src);
    let file_runtime_values =
        extract_runtime_values(&compiled.tir, &plan, &compiled.declared_types, file_src);

    // Capture dag TIRs so cross-file qualified inline calls can merge them
    // into the importer's TIR::dags under module-prefixed keys.
    let dag_tirs = compiled.tir.dags.clone();

    evaluated_files.insert(
        file_dag_id.clone(),
        EvaluatedFile {
            values: file_runtime_values,
            const_values: plan
                .const_values
                .into_iter()
                .map(|(k, v)| (k.into_inner(), v))
                .collect(),
            declared_types: compiled.declared_types,
            assertions: eval_result
                .assertions
                .into_iter()
                .map(|(name, result, span)| (name, (result, span)))
                .collect(),
            registry: compiled.tir.registry,
            pub_names,
            dag_tirs,
        },
    );
    Ok(())
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
    reason = "sequential per-file evaluation steps"
)]
pub(super) fn evaluate_project_perfile(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
    allow_defaults: bool,
) -> Result<EvalResult, CompileError> {
    // Pre-compute override routing: map each override name to the file that owns
    // the param. Walk root file's imports to find the owning file for each override.
    let override_targets = route_overrides_to_files(project, overrides)?;

    // Strict param check: when overrides are provided and --allow-defaults is not set,
    // all overridable params (root file + selectively imported) must be explicitly provided.
    if !overrides.is_empty() && !allow_defaults {
        let root_file = &project.files[&project.root];
        let root_src = &root_file.named_source;

        // Check root file's own params
        for decl in &root_file.ast.declarations {
            if let DeclKind::Param(p) = &decl.kind
                && p.value.is_some()
            {
                let is_overridden = override_targets.values().any(|(target_dag_id, orig_name)| {
                    *target_dag_id == project.root && orig_name.as_str() == p.name.value.as_str()
                });
                if !is_overridden {
                    return Err(CompileError::Eval(GraphcalError::DefaultParamNotProvided {
                        name: p.name.value.to_string(),
                        help: format!(
                            "provide via `--set '{name}=<value>'` or use `--allow-defaults`",
                            name = p.name.value,
                        ),
                        src: root_src.clone(),
                        span: decl.span.into(),
                    }));
                }
            }
        }

        // Check params from non-parameterized selective imports and includes
        let selective_imports: Vec<_> = root_file
            .imports_with_dag_ids()
            .map(|(_, d, c)| (&d.kind, c))
            .chain(root_file.includes_with_dag_ids().filter_map(|(_, d, c)| {
                if d.param_bindings.is_empty() {
                    Some((&d.kind, c))
                } else {
                    None
                }
            }))
            .collect();
        for (import_kind, import_canonical) in selective_imports {
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = import_kind {
                let dep_file = &project.files[import_canonical];
                let dep_src = &dep_file.named_source;

                // For each param in the dep that is selectively imported
                for item in names {
                    let orig_name = item.name.name.as_str();
                    // Find the param declaration in the dep
                    for dep_decl in &dep_file.ast.declarations {
                        if let DeclKind::Param(p) = &dep_decl.kind
                            && p.name.value.as_str() == orig_name
                            && p.value.is_some()
                        {
                            let local_name = item.local_name();
                            let is_overridden = overrides.keys().any(|k| k.as_str() == local_name);
                            if !is_overridden {
                                return Err(CompileError::Eval(
                                    GraphcalError::DefaultParamNotProvided {
                                        name: local_name.to_string(),
                                        help: format!(
                                            "provide via `--set '{local_name}=<value>'` or use `--allow-defaults`",
                                        ),
                                        src: dep_src.clone(),
                                        span: dep_decl.span.into(),
                                    },
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut evaluated_files: HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile> =
        HashMap::new();

    for file_dag_id in &project.load_order {
        let is_root = *file_dag_id == project.root;
        let compiled = compile_single_file_in_project(
            project,
            file_dag_id,
            &evaluated_files,
            overrides,
            &override_targets,
        )?;

        // Files with required params (no default) or required indexes cannot be
        // evaluated standalone. They are only consumed via instantiated imports
        // where `merge_dependency` provides the bindings.
        let is_library = compiled.tir.is_library();
        let has_required_indexes = compiled
            .tir
            .registry
            .indexes
            .all_indexes()
            .any(graphcal_compiler::registry::types::IndexDef::is_required);

        if !is_root && is_library {
            continue;
        }

        if is_root {
            // Reject standalone evaluation of files with required indexes.
            if has_required_indexes {
                let file_src = &project.files[file_dag_id].named_source;
                for idx_def in compiled.tir.registry.indexes.all_indexes() {
                    if idx_def.is_required() {
                        let span = project.files[file_dag_id]
                            .ast
                            .declarations
                            .iter()
                            .find_map(|d| {
                                if let DeclKind::Index(idx) = &d.kind
                                    && idx.name.value.as_str() == idx_def.name.as_str()
                                {
                                    Some(d.span.into())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| miette::SourceSpan::from((0, 0)));
                        return Err(CompileError::Eval(GraphcalError::RequiredIndexNotBound {
                            name: idx_def.name.to_string(),
                            src: file_src.clone(),
                            span,
                        }));
                    }
                }
            }
            let file_src = &project.files[file_dag_id].named_source;
            let plan = crate::exec_plan::compile(&compiled.tir, file_src)?;
            let eval_result =
                evaluate_plan(&compiled.tir, &plan, &compiled.declared_types, file_src);

            // Build a mapping from each dependency file path to the root-level
            // import statement span that (directly or transitively) brought it in.
            let dep_import_spans = build_dep_import_spans(project);

            // Aggregate assertions from all dependency files, replacing the
            // assertion's original span with the root file's import statement span.
            let mut all_assertions: Vec<(DeclName, AssertResult, Span)> = Vec::new();
            for dep_dag_id in &project.load_order {
                if *dep_dag_id == project.root {
                    continue;
                }
                if let Some(dep_eval) = evaluated_files.get(dep_dag_id) {
                    let import_span = dep_import_spans
                        .get(dep_dag_id)
                        .copied()
                        .unwrap_or(Span::new(0, 0));
                    all_assertions.extend(dep_eval.assertions.iter().map(
                        |(name, (result, _span))| (name.clone(), result.clone(), import_span),
                    ));
                }
            }
            all_assertions.extend(eval_result.assertions);

            // Prepend imported values to the output so they appear in the
            // result just like in the old single-IR approach.
            let mut all_consts = Vec::new();
            let mut all_params = Vec::new();
            let mut all_all = Vec::new();

            for (name, cat) in &compiled.imported_source_order {
                if let Some((rv, dt)) = compiled.imported_values.get(name) {
                    let value = super::super::runtime::runtime_to_value(
                        rv,
                        Some(dt),
                        &compiled.tir.registry,
                    );
                    let decl_name = DeclName::new(name.to_string());
                    match cat {
                        DeclCategory::Const => {
                            all_consts.push((decl_name.clone(), value.clone()));
                            all_all.push((
                                decl_name,
                                Ok(value),
                                super::super::types::DeclType::Const,
                            ));
                        }
                        DeclCategory::Param => {
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((
                                decl_name,
                                Ok(value),
                                super::super::types::DeclType::Param,
                            ));
                        }
                        DeclCategory::Node => {
                            // Imported nodes appear as params in the output.
                            all_params.push((decl_name.clone(), Ok(value.clone())));
                            all_all.push((
                                decl_name,
                                Ok(value),
                                super::super::types::DeclType::Node,
                            ));
                        }
                        DeclCategory::Assert
                        | DeclCategory::Plot
                        | DeclCategory::Figure
                        | DeclCategory::Layer => {}
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
                plots: eval_result.plots,
                figures: eval_result.figures,
                layers: eval_result.layers,
                assumes_map: eval_result.assumes_map,
                base_dim_symbols: eval_result.base_dim_symbols,
                domain_constraints: eval_result.domain_constraints,
            });
        }

        let file_src = &project.files[file_dag_id].named_source;
        let pub_names = extract_pub_names(&project.files[file_dag_id].ast);
        evaluate_and_store_file(
            compiled,
            file_dag_id,
            file_src,
            pub_names,
            &mut evaluated_files,
        )?;
    }

    // Should not reach here — root file should have returned above.
    let internal_src = graphcal_compiler::syntax::named_source("internal", String::new());
    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: internal_src,
        span: (0, 0).into(),
    }))
}

/// Map each dependency file to the root-level import statement span that brought it in.
///
/// Direct imports get the span of their own `import` declaration in the root file.
/// Transitive imports inherit the root-level import span of the direct import
/// that started the chain. When a transitive dependency is reachable from multiple
/// root imports, the first root import in source order wins.
pub(super) fn build_dep_import_spans(
    project: &crate::loader::LoadedProject,
) -> HashMap<graphcal_compiler::syntax::dag_id::DagId, Span> {
    let root_file = &project.files[&project.root];
    let mut spans: HashMap<graphcal_compiler::syntax::dag_id::DagId, Span> = HashMap::new();

    // Process root's direct imports/includes in source order.
    // For each, DFS into its transitive dependencies, propagating the root span.
    // `entry().or_insert()` ensures the first root import/include (in source order) to reach
    // a transitive dep determines its attribution.
    let root_decl_dag_ids: Vec<(Span, graphcal_compiler::syntax::dag_id::DagId)> = root_file
        .imports_with_dag_ids()
        .map(|(d, _, c)| (d.span, c.clone()))
        .chain(
            root_file
                .includes_with_dag_ids()
                .map(|(d, _, c)| (d.span, c.clone())),
        )
        .collect();
    for (root_span, canonical) in root_decl_dag_ids {
        let mut stack = vec![canonical];
        while let Some(dag_id) = stack.pop() {
            if dag_id == project.root {
                continue;
            }
            // Only process if not already attributed.
            if let std::collections::hash_map::Entry::Vacant(entry) = spans.entry(dag_id.clone()) {
                entry.insert(root_span);
                // Push this file's own imports/includes for transitive propagation.
                if let Some(file) = project.files.get(&dag_id) {
                    for (_decl, _imp, c) in file.imports_with_dag_ids() {
                        if !spans.contains_key(c) {
                            stack.push(c.clone());
                        }
                    }
                    for (_decl, _inc, c) in file.includes_with_dag_ids() {
                        if !spans.contains_key(c) {
                            stack.push(c.clone());
                        }
                    }
                }
            }
        }
    }

    spans
}

/// Compile a project to TIR using per-file evaluation.
///
/// Non-root files are fully evaluated to produce `RuntimeValue`s for downstream
/// imports. The root file stops at TIR and returns it.
pub(super) fn compile_to_tir_project_perfile(
    project: &crate::loader::LoadedProject,
) -> Result<graphcal_compiler::tir::typed::TIR, CompileError> {
    let empty_overrides = HashMap::new();
    let empty_targets = HashMap::new();
    let mut evaluated_files: HashMap<graphcal_compiler::syntax::dag_id::DagId, EvaluatedFile> =
        HashMap::new();

    for file_dag_id in &project.load_order {
        let is_root = *file_dag_id == project.root;
        let compiled = compile_single_file_in_project(
            project,
            file_dag_id,
            &evaluated_files,
            &empty_overrides,
            &empty_targets,
        )?;

        if is_root {
            return Ok(compiled.tir);
        }

        // Skip standalone evaluation for files with required params (no default).
        let has_required_params = compiled.tir.params.iter().any(|e| e.default_expr.is_none());
        if has_required_params {
            continue;
        }

        let file_src = &project.files[file_dag_id].named_source;
        let pub_names = extract_pub_names(&project.files[file_dag_id].ast);
        evaluate_and_store_file(
            compiled,
            file_dag_id,
            file_src,
            pub_names,
            &mut evaluated_files,
        )?;
    }

    let internal_src = graphcal_compiler::syntax::named_source("internal", String::new());
    Err(CompileError::Eval(GraphcalError::EvalError {
        message: "internal: root file not found in load_order".to_string(),
        src: internal_src,
        span: (0, 0).into(),
    }))
}

/// Route `--set` / `--input` overrides to the files that own the targeted params.
///
/// Returns a map: `override_name` → (`owning_dag_id`, `original_param_name`).
/// The `original_param_name` may differ from `override_name` when an alias is used.
pub(super) fn route_overrides_to_files(
    project: &crate::loader::LoadedProject,
    overrides: &HashMap<DeclName, graphcal_compiler::syntax::ast::Expr>,
) -> Result<HashMap<DeclName, (graphcal_compiler::syntax::dag_id::DagId, DeclName)>, CompileError> {
    if overrides.is_empty() {
        return Ok(HashMap::new());
    }

    let root_file = &project.files[&project.root];

    let mut result: HashMap<DeclName, (graphcal_compiler::syntax::dag_id::DagId, DeclName)> =
        HashMap::new();

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

        // Check if the root file imports/includes this param from a dependency.
        let mut found = false;
        let selective_decls: Vec<_> = root_file
            .imports_with_dag_ids()
            .map(|(_, d, c)| (&d.kind, c))
            .chain(
                root_file
                    .includes_with_dag_ids()
                    .map(|(_, d, c)| (&d.kind, c)),
            )
            .collect();
        for (import_kind, import_canonical) in selective_decls {
            if let graphcal_compiler::syntax::ast::ImportKind::Selective(names) = import_kind {
                for item in names {
                    let local_name = item.local_name().to_string();
                    if local_name == name_str {
                        let orig_name = &item.name.name;

                        // Verify it's actually a param in the source file.
                        let dep_file = &project.files[import_canonical];
                        let is_param = dep_file.ast.declarations.iter().any(|d| {
                            matches!(&d.kind, DeclKind::Param(p) if p.name.value.as_str() == orig_name)
                        });
                        if is_param {
                            result.insert(
                                override_name.clone(),
                                (import_canonical.clone(), DeclName::new(orig_name.clone())),
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

        if !found {
            // Check if the name matches a non-param declaration (node, const, assert)
            // in the root file to provide a better error message.
            for decl in &root_file.ast.declarations {
                let kind = match &decl.kind {
                    DeclKind::ConstNode(c) if c.name.value.as_str() == name_str => {
                        Some(DeclCategory::Const)
                    }
                    DeclKind::Node(n) if n.name.value.as_str() == name_str => {
                        Some(DeclCategory::Node)
                    }
                    DeclKind::Assert(a) if a.name.value.as_str() == name_str => {
                        Some(DeclCategory::Assert)
                    }
                    _ => None,
                };
                if let Some(actual_kind) = kind {
                    return Err(CompileError::Eval(GraphcalError::OverrideNotAParam {
                        name: override_name.clone(),
                        actual_kind,
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
///
/// Delegates to the shared [`run_eval_loop`](super::super::runtime::run_eval_loop) and
/// filters the result to only locally-defined param/node values.
pub(super) fn extract_runtime_values(
    tir: &graphcal_compiler::tir::typed::TIR,
    plan: &crate::exec_plan::ExecPlan,
    declared_types: &HashMap<String, DeclaredType>,
    src: &NamedSource<Arc<String>>,
) -> HashMap<String, RuntimeValue> {
    let builtin_consts = graphcal_compiler::registry::builtins::builtin_constants();
    let builtin_fns = graphcal_compiler::registry::builtins::builtin_functions();
    let result = super::super::runtime::run_eval_loop(
        plan,
        tir,
        declared_types,
        src,
        builtin_consts,
        builtin_fns,
    );

    // Return only locally-defined param/node values (not imported, not consts).
    let local_runtime_names: HashSet<String> = tir
        .params
        .iter()
        .map(|e| e.name.to_string())
        .chain(tir.nodes.iter().map(|e| e.name.to_string()))
        .collect();

    result
        .values
        .into_iter()
        .filter(|(name, _)| local_runtime_names.contains(name.as_str()))
        .map(|(k, v)| (k.into_inner(), v))
        .collect()
}

//! Inline-DAG preprocessing and per-file dag-body compilation.
//!
//! This module owns the orchestration logic that turns a file's inline
//! `dag X { ... }` blocks into compiled per-dag `TIR`s and inserts them into
//! the parent file's `tir.dags`. The lowering and type-resolution primitives
//! still live in the compiler crate; this module wires them together with
//! the loader-level information (canonical `DagId`, self-import path
//! resolution) that the project pipeline supplies.
//!
//! Tracks toward unifying file and inline DAGs (#536): self-import handling
//! lives at the layer that knows about the project, not at the IR-lowering
//! primitive. The compiler-side `lower_dag_body_to_ir` is now a clean
//! lowering primitive — give it pre-processed inputs and it builds the
//! dag-body IR.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration};
use graphcal_compiler::ir::lower::{
    DagBodySelfImports, ImportedValueSource, lower_dag_body_to_ir, type_system_names_from_registry,
};
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::dag_id::DagId;
use graphcal_compiler::syntax::names::DeclName;
use graphcal_compiler::tir::typed::{TIR, resolved_to_declared_type, type_resolve_single};

use crate::loader::LoadedDag;

/// Compile each inline `dag { ... }` body lifted by the loader into a
/// `DagTIR` and insert it into `tir.dags`, keyed by the loader-supplied
/// canonical [`DagId`](DagId).
///
/// `parent_pub_names` is captured from the IR before `type_resolve` consumes
/// it. Each [`LoadedDag`] supplies the body in source order plus its
/// pre-resolved imports map (path display → [`DagId`]), letting
/// [`preprocess_dag_body_self_imports`] detect self-imports by structured
/// equality against `parent_dag_id` rather than a file-level path-set.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if compiling any dag body fails (typically a
/// self-import naming an unknown name, runtime declaration, or private
/// declaration in the parent file).
pub fn compile_inline_dag_bodies(
    tir: &mut TIR,
    src: &NamedSource<Arc<String>>,
    parent_dag_id: &DagId,
    parent_pub_names: &HashSet<DeclName>,
    inline_dags: &[LoadedDag],
) -> Result<(), GraphcalError> {
    // Read parent's value decls directly from the (already type-resolved)
    // root DagTIR. With the flat registry, `tir.root()` IS the parent
    // file's body — no separate `ParentValueKind` table needed.
    let root = tir.root();
    let mut parent_consts: HashMap<String, DeclaredType> = HashMap::new();
    for entry in &root.consts {
        let Some(resolved) = root.resolved_decl_types.get(&entry.name) else {
            continue;
        };
        parent_consts.insert(
            entry.name.member().to_string(),
            resolved_to_declared_type(resolved, src)?,
        );
    }
    let mut parent_runtime_names: HashSet<String> = HashSet::new();
    for entry in &root.params {
        parent_runtime_names.insert(entry.name.member().to_string());
    }
    for entry in &root.nodes {
        parent_runtime_names.insert(entry.name.member().to_string());
    }
    let parent_type_system_names = type_system_names_from_registry(&tir.registry);

    for loaded_dag in inline_dags {
        let DagBodySelfImports {
            names: imported_names,
            decl_types: imported_decl_types,
            value_sources: imported_value_sources,
            stripped_body,
        } = preprocess_dag_body_self_imports(
            &loaded_dag.body,
            parent_dag_id,
            &parent_type_system_names,
            &parent_consts,
            &parent_runtime_names,
            parent_pub_names,
            &loaded_dag.resolved_imports,
            src,
        )?;
        let dag_body_ir = lower_dag_body_to_ir(
            &loaded_dag.name,
            &stripped_body,
            &tir.registry,
            &imported_names,
            imported_decl_types,
            imported_value_sources,
            src,
            parent_dag_id,
        )?;
        let mut compiled_dag = type_resolve_single(dag_body_ir, &loaded_dag.dag_id, src)?;
        compiled_dag.populate_pub_nodes(&loaded_dag.body);
        tir.dags.insert(loaded_dag.dag_id.clone(), compiled_dag);
    }

    Ok(())
}

/// Pre-process `import <self>.{...}` declarations inside a dag body.
///
/// A self-import is one whose `ModulePath` display string is keyed in
/// `body_resolved_imports` to `parent_dag_id` — i.e. the loader resolved
/// the path back to the dag's own enclosing parent DAG. For each
/// self-import, every brace-list item is classified against
/// `parent_type_system_names`, `parent_value_decls`, and `parent_pub_names`.
///
/// - Type-system items (dim/unit/type/union/index/dag): elided when
///   `pub`-marked. They are already accessible through the parent-registry
///   merge once visibility is satisfied.
/// - `const` items: added to `ImportedValueNames::const_names`, to
///   `imported_decl_types` with the parent's declared type, and to
///   `imported_value_sources` so evaluation can copy the concrete value
///   from the caller or the owning dependency.
/// - `param` / `node` items: rejected with `ImportRuntimeItem` — runtime
///   values must be passed via the dag's own params, regardless of pub.
/// - Items that exist in the parent but are not `pub`: rejected with
///   `ImportPrivateItem`, identical to the cross-file path.
/// - Names not found in the parent at all: rejected with
///   `ImportNameNotFound`.
///
/// Non-self imports are left in the body untouched. They are handled by
/// downstream project-pipeline stages.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if a self-import names a runtime declaration
/// (param/node), a private declaration (no `pub`), or a name that does not
/// exist in the parent.
#[expect(
    clippy::too_many_arguments,
    reason = "self-import classification needs parent's type-system, const, runtime, and pub views"
)]
pub fn preprocess_dag_body_self_imports(
    body: &[Declaration],
    parent_dag_id: &DagId,
    parent_type_system_names: &HashSet<String>,
    parent_consts: &HashMap<String, DeclaredType>,
    parent_runtime_names: &HashSet<String>,
    parent_pub_names: &HashSet<DeclName>,
    body_resolved_imports: &HashMap<String, DagId>,
    src: &NamedSource<Arc<String>>,
) -> Result<DagBodySelfImports, GraphcalError> {
    let mut names = ImportedValueNames::default();
    let mut decl_types: HashMap<ScopedName, DeclaredType> = HashMap::new();
    let mut value_sources: HashMap<ScopedName, ImportedValueSource> = HashMap::new();
    let mut stripped_body: Vec<Declaration> = Vec::with_capacity(body.len());

    for decl in body {
        let DeclKind::Import(import_decl) = &decl.kind else {
            stripped_body.push(decl.clone());
            continue;
        };

        let is_self_import = body_resolved_imports
            .get(&import_decl.path.display_path())
            .is_some_and(|dag_id| dag_id == parent_dag_id);
        if !is_self_import {
            stripped_body.push(decl.clone());
            continue;
        }

        match &import_decl.kind {
            graphcal_compiler::syntax::ast::ImportKind::Selective(items) => {
                for item in items {
                    let orig_name = &item.name.name;
                    let local_name = item.local_name().to_string();
                    let span = item.name.span;
                    let exists_as_type_system =
                        parent_type_system_names.contains(orig_name.as_str());
                    let const_dt = parent_consts.get(orig_name.as_str());
                    let is_runtime = parent_runtime_names.contains(orig_name.as_str());

                    if !exists_as_type_system && const_dt.is_none() && !is_runtime {
                        return Err(GraphcalError::ImportNameNotFound {
                            name: orig_name.clone(),
                            file_path: import_decl.path.display_path(),
                            src: src.clone(),
                            span: span.into(),
                        });
                    }

                    if !parent_pub_names.contains(orig_name.as_str()) {
                        return Err(GraphcalError::ImportPrivateItem {
                            name: orig_name.clone(),
                            file_path: import_decl.path.display_path(),
                            src: src.clone(),
                            span: span.into(),
                        });
                    }

                    if let Some(dt) = const_dt {
                        let scoped = ScopedName::Local(local_name);
                        names.const_names.push((scoped.clone(), span));
                        decl_types.insert(scoped.clone(), dt.clone());
                        value_sources.insert(
                            scoped,
                            ImportedValueSource {
                                dag_id: parent_dag_id.clone(),
                                source_name: DeclName::new(orig_name),
                            },
                        );
                    } else if is_runtime {
                        return Err(GraphcalError::ImportRuntimeItem {
                            name: orig_name.clone(),
                            src: src.clone(),
                            span: span.into(),
                        });
                    } else {
                        // Type-system items: no resolver registration
                        // needed — they're already accessible through the
                        // parent-registry merge once visibility is satisfied.
                    }
                }
            }
            graphcal_compiler::syntax::ast::ImportKind::Module { .. } => {
                stripped_body.push(decl.clone());
            }
        }
    }

    Ok(DagBodySelfImports {
        names,
        decl_types,
        value_sources,
        stripped_body,
    })
}

/// Classify the value-kind decls in a file's AST for use as the
/// `parent_consts` / `parent_runtime_names` arguments of
/// [`preprocess_dag_body_self_imports`].
///
/// Used by `process_deferred_dag_includes` for the inline-DAG include
/// path: when the parent file's TIR isn't yet type-resolved, the AST
/// tells us which names exist and which kind they are. Const types are
/// placeholder `Dimensionless` — the cross-file path overrides them with
/// real types from the parent's `EvaluatedFile.declared_types`; the
/// same-file path lets the importer's own resolved types win at
/// dim-check time.
pub fn classify_value_decls_in_ast(
    ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> (HashMap<String, DeclaredType>, HashSet<String>) {
    let placeholder =
        || DeclaredType::Scalar(graphcal_compiler::syntax::dimension::Dimension::dimensionless());
    let mut consts: HashMap<String, DeclaredType> = HashMap::new();
    let mut runtime_names: HashSet<String> = HashSet::new();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::ConstNode(c) => {
                consts.insert(c.name.value.to_string(), placeholder());
            }
            DeclKind::Param(p) => {
                runtime_names.insert(p.name.value.to_string());
            }
            DeclKind::Node(n) => {
                runtime_names.insert(n.name.value.to_string());
            }
            _ => {}
        }
    }
    (consts, runtime_names)
}

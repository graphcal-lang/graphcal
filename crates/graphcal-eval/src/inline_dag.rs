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
    DagBodySelfImports, ImportedValueSource, ParentValueKind, lower_dag_body_to_ir,
    type_system_names_from_registry,
};
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::dag_id::DagId;
use graphcal_compiler::syntax::dimension::Dimension;
use graphcal_compiler::tir::typed::{
    DagKey, TIR, populate_pub_nodes, resolved_to_declared_type, type_resolve_single,
};

use crate::loader::LoadedDag;

/// Compile each inline `dag { ... }` body lifted by the loader into a
/// [`LoadedDag`] and insert the resulting per-dag `TIR`s into `tir.dags`,
/// keyed by [`DagKey::local`].
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
    parent_pub_names: &HashSet<String>,
    inline_dags: &[LoadedDag],
) -> Result<(), GraphcalError> {
    let parent_value_decls = build_parent_value_decls(tir, src)?;
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
            &parent_value_decls,
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
        let mut compiled_dag = type_resolve_single(dag_body_ir, src)?;
        populate_pub_nodes(&mut compiled_dag, &loaded_dag.body);
        tir.dags
            .insert(DagKey::local(loaded_dag.name.clone()), compiled_dag);
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
pub fn preprocess_dag_body_self_imports(
    body: &[Declaration],
    parent_dag_id: &DagId,
    parent_type_system_names: &HashSet<String>,
    parent_value_decls: &HashMap<String, ParentValueKind>,
    parent_pub_names: &HashSet<String>,
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
                    let value_kind = parent_value_decls.get(orig_name.as_str());

                    if !exists_as_type_system && value_kind.is_none() {
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

                    match value_kind {
                        Some(ParentValueKind::Const(dt)) => {
                            let scoped = ScopedName::Local(local_name);
                            names.const_names.push((scoped.clone(), span));
                            decl_types.insert(scoped.clone(), dt.clone());
                            value_sources.insert(
                                scoped,
                                ImportedValueSource {
                                    dag_id: parent_dag_id.clone(),
                                    source_name: orig_name.clone(),
                                },
                            );
                        }
                        Some(ParentValueKind::Param(_) | ParentValueKind::Node(_)) => {
                            return Err(GraphcalError::ImportRuntimeItem {
                                name: orig_name.clone(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                        None => {}
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

/// Build the `name → ParentValueKind` table consumed by
/// [`preprocess_dag_body_self_imports`] from a fully type-resolved parent
/// `TIR`.
///
/// Walks the parent file's TIR-level const/param/node entries and resolves
/// each declared type to a concrete `DeclaredType`. Declarations that carry
/// a generic dim/index/type parameter cannot appear at file scope (generics
/// are dag-body-only), so the conversion is total in well-formed input.
fn build_parent_value_decls(
    tir: &TIR,
    src: &NamedSource<Arc<String>>,
) -> Result<HashMap<String, ParentValueKind>, GraphcalError> {
    let mut out = HashMap::new();
    for entry in &tir.consts {
        let Some(resolved) = tir.resolved_decl_types.get(&entry.name) else {
            continue;
        };
        let dt = resolved_to_declared_type(resolved, src)?;
        out.insert(entry.name.member().to_string(), ParentValueKind::Const(dt));
    }
    for entry in &tir.params {
        let Some(resolved) = tir.resolved_decl_types.get(&entry.name) else {
            continue;
        };
        let dt = resolved_to_declared_type(resolved, src)?;
        out.insert(entry.name.member().to_string(), ParentValueKind::Param(dt));
    }
    for entry in &tir.nodes {
        let Some(resolved) = tir.resolved_decl_types.get(&entry.name) else {
            continue;
        };
        let dt = resolved_to_declared_type(resolved, src)?;
        out.insert(entry.name.member().to_string(), ParentValueKind::Node(dt));
    }
    Ok(out)
}

/// Build the `name → ParentValueKind` table from the importer's AST,
/// without resolving types.
///
/// Used to classify dag-body self-imports before compiling an inline DAG
/// include. The declared type payload is not consumed on this include path:
/// the dag IR is merged into the importer before type resolution, so the
/// importer's own resolved declared types drive downstream dim-checking.
pub fn build_importer_value_decls(
    importer_ast: &graphcal_compiler::desugar::desugared_ast::File,
) -> HashMap<String, ParentValueKind> {
    let unused_type = || DeclaredType::Scalar(Dimension::dimensionless());
    let mut out = HashMap::new();
    for decl in &importer_ast.declarations {
        match &decl.kind {
            DeclKind::ConstNode(c) => {
                out.insert(
                    c.name.value.to_string(),
                    ParentValueKind::Const(unused_type()),
                );
            }
            DeclKind::Param(p) => {
                out.insert(
                    p.name.value.to_string(),
                    ParentValueKind::Param(unused_type()),
                );
            }
            DeclKind::Node(n) => {
                out.insert(
                    n.name.value.to_string(),
                    ParentValueKind::Node(unused_type()),
                );
            }
            _ => {}
        }
    }
    out
}

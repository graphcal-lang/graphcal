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

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::resolved_ast::{DeclKind, Declaration, File, ImportItemNamespace};
use graphcal_compiler::ir::lower::{DagBodySelfImports, ImportedValueSource, lower_dag_body_to_ir};
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::module_resolve::ModuleResolver;
use graphcal_compiler::syntax::names::DeclName;
use graphcal_compiler::tir::typed::{
    ModuleTypeRegistry, TIR, resolved_to_declared_type, type_resolve_single_with_modules,
};

use crate::import_surface::{ImportItemPresence, file_import_item_presence};
use crate::loader::LoadedDag;

/// Public parent-file value declarations visible to an inline DAG self-import
/// classifier.
#[derive(Debug, Clone, Default)]
pub struct ParentValueDecls {
    consts: HashMap<DeclName, DeclaredType>,
    runtime: HashSet<DeclName>,
}

impl ParentValueDecls {
    fn public_const_type(&self, name: &str) -> Option<&DeclaredType> {
        self.consts.get(name)
    }

    fn is_public_runtime(&self, name: &str) -> bool {
        self.runtime.contains(name)
    }
}

/// Compile each inline `dag { ... }` body lifted by the loader into a
/// `DagTIR` and insert it into `tir.dags`, keyed by the loader-supplied
/// canonical [`DagId`](DagId).
///
/// `parent_pub_names` is captured from the IR before module-aware TIR
/// construction consumes it. Each [`LoadedDag`] supplies the body in source order plus its
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
    parent_ast: &File,
    parent_pub_names: &HashSet<DeclName>,
    inline_dags: &[LoadedDag],
    module_resolver: &ModuleResolver,
    module_types: &ModuleTypeRegistry,
) -> Result<(), GraphcalError> {
    // Read parent's value decls directly from the (already type-resolved)
    // root DagTIR. With the flat registry, `tir.root()` IS the parent
    // file's body — no separate `ParentValueKind` table needed.
    let parent_values = classify_value_decls_in_tir(tir, parent_pub_names, src)?;

    for loaded_dag in inline_dags {
        let DagBodySelfImports {
            names: imported_names,
            decl_types: imported_decl_types,
            value_sources: imported_value_sources,
            stripped_body,
        } = preprocess_dag_body_self_imports(
            &loaded_dag.body,
            parent_dag_id,
            parent_ast,
            &parent_values,
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
        let mut compiled_dag = type_resolve_single_with_modules(
            dag_body_ir,
            &loaded_dag.dag_id,
            src,
            module_resolver,
            module_types,
        )?;
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
/// self-import, every brace-list item is classified in the namespace it
/// requested (`type` or default) against the parent AST.
///
/// - `type` items are visibility-checked as struct/tagged-union type names
///   only. They do not import same-named constructors.
/// - Default `const` items are added to `ImportedValueNames::const_names`, to
///   `imported_decl_types` with the parent's declared type, and to
///   `imported_value_sources` so evaluation can copy the concrete value
///   from the caller or the owning dependency.
/// - Default `param` / non-const `node` items are rejected with
///   `ImportRuntimeItem` — runtime values must be passed via the dag's own
///   params.
/// - Other default compile-time items (dim/unit/index/constructor/dag/assert)
///   require no value resolver registration here; module-aware lowering uses
///   the loader-built import scope for those namespaces.
/// - Items that exist in the requested namespace but are not `pub` are
///   rejected with `ImportPrivateItem`, identical to the cross-file path.
/// - Names not found in the requested namespace are rejected with
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
    parent_ast: &File,
    parent_values: &ParentValueDecls,
    body_resolved_imports: &HashMap<
        crate::loader::ModulePathKey,
        crate::loader::InlineBodyImportResolution,
    >,
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
            .get(&crate::loader::ModulePathKey::from_path(&import_decl.path))
            .is_some_and(|resolution| {
                matches!(
                    resolution,
                    crate::loader::InlineBodyImportResolution::Resolved(dag_id)
                        if dag_id == parent_dag_id
                )
            });
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

                    match file_import_item_presence(parent_ast, orig_name.as_str(), item.namespace)
                    {
                        ImportItemPresence::Missing => {
                            return Err(GraphcalError::ImportNameNotFound {
                                name: orig_name.to_string(),
                                file_path: import_decl.path.display_path(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                        ImportItemPresence::Private => {
                            return Err(GraphcalError::ImportPrivateItem {
                                name: orig_name.to_string(),
                                file_path: import_decl.path.display_path(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                        ImportItemPresence::Public => {}
                    }

                    match item.namespace {
                        ImportItemNamespace::Type => {
                            // Type imports affect only the type namespace. A
                            // same-named constructor must be imported as a
                            // separate default item.
                        }
                        ImportItemNamespace::Default => {
                            if let Some(dt) = parent_values.public_const_type(orig_name.as_str()) {
                                let scoped = ScopedName::local(local_name);
                                names.const_names.push((scoped.clone(), span));
                                decl_types.insert(scoped.clone(), dt.clone());
                                value_sources.insert(
                                    scoped,
                                    ImportedValueSource {
                                        dag_id: parent_dag_id.clone(),
                                        source_name: DeclName::new(orig_name),
                                    },
                                );
                            } else if parent_values.is_public_runtime(orig_name.as_str()) {
                                return Err(GraphcalError::ImportRuntimeItem {
                                    name: orig_name.to_string(),
                                    src: src.clone(),
                                    span: span.into(),
                                });
                            }
                        }
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
/// [`preprocess_dag_body_self_imports`]. Pairs with
/// [`classify_value_decls_in_tir`] — same shape, different input source.
///
/// Used by `process_deferred_dag_includes` for the inline-DAG include
/// path: when the parent file's TIR isn't yet type-resolved, the AST
/// tells us which names exist and which kind they are. Const types are
/// placeholder `Dimensionless` — the cross-file path overrides them with
/// real types from the parent's `EvaluatedFile.declared_types`; the
/// same-file path lets the importer's own resolved types win at
/// dim-check time.
pub fn classify_value_decls_in_ast(
    ast: &graphcal_compiler::desugar::resolved_ast::File,
) -> ParentValueDecls {
    let placeholder =
        || DeclaredType::Scalar(graphcal_compiler::syntax::dimension::Dimension::dimensionless());
    let mut values = ParentValueDecls::default();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::ConstNode(c) if c.visibility.is_public() => {
                values.consts.insert(c.name.value.clone(), placeholder());
            }
            DeclKind::Param(p) => {
                // Params are always visible/bindable across import/include
                // boundaries, but they are runtime values and cannot be
                // imported into an inline DAG body.
                values.runtime.insert(p.name.value.clone());
            }
            DeclKind::Node(n) if n.visibility.is_public() => {
                values.runtime.insert(n.name.value.clone());
            }
            _ => {}
        }
    }
    values
}

/// Classify the value-kind decls of a type-resolved root [`TIR`] into the
/// same `(consts, runtime_names)` shape returned by
/// [`classify_value_decls_in_ast`]. Pairs with that helper — TIR-stage
/// callers (where the parent file is already type-resolved) get real
/// declared types from `resolved_decl_types` instead of the AST-side
/// `Dimensionless` placeholders.
///
/// # Errors
///
/// Returns a [`GraphcalError`] if any resolved const type cannot be lowered
/// back to a [`DeclaredType`].
fn classify_value_decls_in_tir(
    tir: &TIR,
    parent_pub_names: &HashSet<DeclName>,
    src: &NamedSource<Arc<String>>,
) -> Result<ParentValueDecls, GraphcalError> {
    let root = tir.root();
    let mut values = ParentValueDecls::default();
    for entry in &root.consts {
        let name = DeclName::new(entry.name.member());
        if !parent_pub_names.contains(&name) {
            continue;
        }
        let Some(resolved) = root.resolved_decl_types.get(&entry.name) else {
            continue;
        };
        values
            .consts
            .insert(name, resolved_to_declared_type(resolved, src)?);
    }
    for entry in &root.params {
        values.runtime.insert(DeclName::new(entry.name.member()));
    }
    for entry in &root.nodes {
        let name = DeclName::new(entry.name.member());
        if parent_pub_names.contains(&name) {
            values.runtime.insert(name);
        }
    }
    Ok(values)
}

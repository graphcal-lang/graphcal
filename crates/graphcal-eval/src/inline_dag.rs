//! Inline-DAG preprocessing helpers.
//!
//! Project-level compilation treats file roots and inline DAGs as DAG modules;
//! this module keeps the self-import classification logic that needs access to
//! parent-file visibility and values.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration, File};
use graphcal_compiler::ir::lower::{DagBodySelfImports, ImportedValueSource};
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::names::DeclName;
use graphcal_compiler::tir::typed::{TIR, resolved_to_declared_type};

use crate::import_surface::{ImportItemPresence, file_import_item_presence};

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
                            match parent_values.public_const_type(orig_name.as_str()) {
                                Some(dt) => {
                                    let scoped = ScopedName::local(local_name);
                                    names.const_names.push((scoped.clone(), span));
                                    decl_types.insert(scoped.clone(), dt.clone());
                                    value_sources.insert(
                                        scoped,
                                        ImportedValueSource {
                                            dag_id: parent_dag_id.clone(),
                                            source_name: DeclName::expect_valid(orig_name),
                                        },
                                    );
                                }
                                None if parent_values.is_public_runtime(orig_name.as_str()) => {
                                    return Err(GraphcalError::ImportRuntimeItem {
                                        name: orig_name.to_string(),
                                        src: src.clone(),
                                        span: span.into(),
                                    });
                                }
                                None => {}
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
    ast: &graphcal_compiler::desugar::desugared_ast::File,
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
pub fn classify_value_decls_in_tir(
    tir: &TIR,
    parent_pub_names: &HashSet<DeclName>,
    src: &NamedSource<Arc<String>>,
) -> Result<ParentValueDecls, GraphcalError> {
    let root = tir.root();
    let mut values = ParentValueDecls::default();
    for entry in &root.consts {
        let name = DeclName::expect_valid(entry.name.member());
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
        values
            .runtime
            .insert(DeclName::expect_valid(entry.name.member()));
    }
    for entry in &root.nodes {
        let name = DeclName::expect_valid(entry.name.member());
        if parent_pub_names.contains(&name) {
            values.runtime.insert(name);
        }
    }
    Ok(values)
}

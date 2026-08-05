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
use graphcal_compiler::ir::imported_binding::ImportedBinding;
use graphcal_compiler::ir::lower::DagBodySelfImports;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::declared_type::DeclaredType;
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::registry::resolve_types::ExternalDeclSurface;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::tir::typed::resolved_to_declared_type;

use crate::import_surface::{
    ImportItemPresence, file_import_item_presence, import_item_not_found_error,
};

/// Parent-file value declarations externally nameable by an inline DAG
/// self-import classifier. Runtime entries include exported nodes and param
/// input ports as distinct source roles, though both are rejected as imports.
#[derive(Debug, Clone, Default)]
pub struct ParentValueDecls {
    consts: HashMap<DeclName, DeclaredType>,
    runtime: HashSet<DeclName>,
}

impl ParentValueDecls {
    fn external_const_type(&self, name: &str) -> Option<&DeclaredType> {
        self.consts.get(name)
    }

    fn is_external_runtime(&self, name: &str) -> bool {
        self.runtime.contains(name)
    }
}

/// Pre-process `import <self>.{...}` declarations inside a dag body.
///
/// A self-import is one whose `ModulePath` display string is keyed in
/// `body_resolved_imports` to `parent_dag_id` — i.e. the loader resolved
/// the path back to the dag's own enclosing parent DAG. For each
/// self-import, every brace-list item is classified in the namespace it
/// requested against the parent AST.
///
/// - Marked `type`, `dim`, `unit`, and `index` items are visibility-checked in
///   exactly that namespace. A type item does not import a same-named constructor.
/// - Bare term `const` items are added to `ImportedValueNames::const_names` and
///   receive one canonical imported binding carrying the parent's declaration
///   identity and declared type. Evaluation may fill its deferred value later.
/// - Bare term `param` / non-const `node` items are rejected with
///   `ImportRuntimeItem` — runtime values must be passed via the dag's own
///   params.
/// - Other compile-time items require no value resolver registration here;
///   module-aware lowering uses the loader-built import scope for their typed
///   namespaces.
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
/// (a param input port or node), a private non-param declaration, or a name
/// that does not exist in the parent.
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
    let mut bindings: HashMap<ScopedName, ImportedBinding> = HashMap::new();
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
                    let local_name = DeclName::from_atom(item.local_name_atom().clone());
                    let span = item.name.span;

                    match file_import_item_presence(parent_ast, orig_name.as_str(), item.namespace)
                    {
                        ImportItemPresence::Missing => {
                            return Err(import_item_not_found_error(
                                parent_ast,
                                orig_name,
                                item.namespace,
                                &import_decl.path.display_path(),
                                src,
                                span,
                            ));
                        }
                        ImportItemPresence::Private => {
                            return Err(GraphcalError::ImportPrivateItem {
                                name: orig_name.to_string(),
                                file_path: import_decl.path.display_path(),
                                src: src.clone(),
                                span: span.into(),
                            });
                        }
                        ImportItemPresence::ExplicitExport | ImportItemPresence::InputPort => {}
                    }

                    match item.namespace {
                        ImportItemNamespace::Type
                        | ImportItemNamespace::Dimension
                        | ImportItemNamespace::Unit
                        | ImportItemNamespace::Index => {
                            // Type-system imports are registered by the project
                            // pipeline. A same-named constructor is a separate
                            // bare term item.
                        }
                        ImportItemNamespace::Term => {
                            match parent_values.external_const_type(orig_name.as_str()) {
                                Some(dt) => {
                                    let scoped = ScopedName::local(local_name);
                                    names.const_names.push((scoped.clone(), span));
                                    bindings.insert(
                                        scoped,
                                        ImportedBinding::deferred(
                                            graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
                                                parent_dag_id.clone(),
                                                DeclName::from_atom(orig_name.clone()),
                                            ),
                                            dt.clone(),
                                        ),
                                    );
                                }
                                None if parent_values.is_external_runtime(orig_name.as_str()) => {
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
        bindings,
        stripped_body,
    })
}

/// Classify the value-kind decls in a file's AST for use as the
/// `parent_consts` / `parent_runtime_names` arguments of
/// [`preprocess_dag_body_self_imports`]. Pairs with
/// [`classify_value_decls_in_dag`] — same shape, different input source.
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
        || DeclaredType::Quantity(graphcal_compiler::dimension::Dimension::dimensionless());
    let mut values = ParentValueDecls::default();
    for decl in &ast.declarations {
        match &decl.kind {
            DeclKind::ConstNode(c) if c.visibility.is_public() => {
                values.consts.insert(c.name.value.clone(), placeholder());
            }
            DeclKind::Param(p) => {
                // Params are named input ports across import/include boundaries,
                // but they are runtime values and cannot be imported into an
                // inline DAG body.
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

/// Classify the value-kind declarations of one type-resolved DAG into the
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
pub fn classify_value_decls_in_dag(
    root: &graphcal_compiler::tir::typed::DagTIR,
    parent_external_surface: &ExternalDeclSurface,
    src: &NamedSource<Arc<String>>,
) -> Result<ParentValueDecls, GraphcalError> {
    let mut values = ParentValueDecls::default();
    for entry in root.consts() {
        let name = entry.name.member().clone();
        if !parent_external_surface.is_explicit_export(&name) {
            continue;
        }
        let Some(resolved) = root.resolved_decl_types().get(&entry.name) else {
            continue;
        };
        values
            .consts
            .insert(name, resolved_to_declared_type(resolved, src)?);
    }
    for entry in root.params() {
        let name = entry.name.member().clone();
        if parent_external_surface.is_input_port(&name) {
            values.runtime.insert(name);
        }
    }
    for entry in root.nodes() {
        let name = entry.name.member().clone();
        if parent_external_surface.is_explicit_export(&name) {
            values.runtime.insert(name);
        }
    }
    Ok(values)
}

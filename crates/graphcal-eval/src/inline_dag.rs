//! Inline-DAG preprocessing helpers.
//!
//! Project-level compilation treats file roots and inline DAGs as DAG modules;
//! this module keeps the self-import classification logic that needs access to
//! parent-file visibility and values.

use std::collections::HashMap;
use std::sync::Arc;

use miette::NamedSource;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration, File};
use graphcal_compiler::ir::imported_binding::HirImportedBinding;
use graphcal_compiler::ir::lower::DagBodySelfImports;
use graphcal_compiler::ir::resolve::{ImportedValueNames, ScopedName};
use graphcal_compiler::registry::error::GraphcalError;
use graphcal_compiler::syntax::ast::ImportItemNamespace;
use graphcal_compiler::syntax::decl_name::DeclName;

use crate::import_surface::{
    ImportItemPresence, PureImportTermDisposition, file_import_item_presence,
    import_item_not_found_error, pure_import_term_disposition,
};

/// Pre-process `import <self>::{...}` declarations inside a dag body.
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
///   receive one canonical HIR binding to the parent's declaration. Static
///   checking attaches the resolved type and any available value later.
/// - Bare term `param` / non-const `node` items are rejected with
///   `ImportRuntimeItem` — runtime values must be passed via the dag's own
///   params. Assertions and visualization requests are likewise rejected with
///   the same diagnostics as cross-file pure imports.
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
#[expect(
    clippy::too_many_lines,
    reason = "one pass classifies and strips every namespace of selective self-import"
)]
pub fn preprocess_dag_body_self_imports(
    body: &[Declaration],
    parent_dag_id: &DagId,
    parent_ast: &File,
    body_resolved_imports: &HashMap<
        crate::loader::ModulePathKey,
        crate::loader::InlineBodyImportResolution,
    >,
    src: &NamedSource<Arc<String>>,
) -> Result<DagBodySelfImports, GraphcalError> {
    let mut names = ImportedValueNames::default();
    let mut bindings: HashMap<ScopedName, HirImportedBinding> = HashMap::new();
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
                    crate::loader::InlineBodyImportResolution::Resolved(target)
                        if target.target() == parent_dag_id
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
                            let disposition =
                                pure_import_term_disposition(&parent_ast.declarations, orig_name)
                                    .ok_or_else(|| {
                                    import_item_not_found_error(
                                        parent_ast,
                                        orig_name,
                                        item.namespace,
                                        &import_decl.path.display_path(),
                                        src,
                                        span,
                                    )
                                })?;
                            match disposition {
                                PureImportTermDisposition::BindConstant => {
                                    let scoped = ScopedName::local(local_name);
                                    names.const_names.push((scoped.clone(), span));
                                    bindings.insert(
                                        scoped,
                                        HirImportedBinding::new(
                                            graphcal_compiler::syntax::decl_name::ResolvedDeclName::from_def(
                                                parent_dag_id.clone(),
                                                DeclName::from_atom(orig_name.clone()),
                                            ),
                                        ),
                                    );
                                }
                                PureImportTermDisposition::ResolverOnly => {}
                                PureImportTermDisposition::Reject(reason) => {
                                    return Err(reason.diagnostic(orig_name, src, span));
                                }
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

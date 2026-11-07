//! Inline-DAG lifting shared by every loader mode.
//!
//! Loader modes differ only in how an import outside the current file
//! resolves, which callers supply as `resolve_external`. DAG discovery,
//! source-body location, and same-file lookup (classified by
//! [`SameFileScope`]) are one algorithm here.

use graphcal_compiler::syntax::decl_name::DeclName;

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::syntax::ast::{DeclKind, ModulePath};

use super::loaded_file::{DagBodyLocator, LoadedDag};
use graphcal_compiler::syntax::module_path_key::ModulePathKey;

use graphcal_compiler::source_registry::RegisteredSource;

use super::module_path::ModuleTarget;
use super::same_file_reference::{LexicalSite, PathScope, SameFileScope};
use crate::compile_error::CompileError;
use crate::load_error::LoadError;

struct InlineDagLiftContext<'a, ResolveExternal> {
    file_dag_id: &'a DagId,
    same_file: &'a SameFileScope,
    /// The file's source, for diagnostics.
    src: &'a RegisteredSource,
    resolve_external: ResolveExternal,
}

/// Walk inline `dag X { ... }` bodies of the file `file_dag_id` and lift each
/// into a [`LoadedDag`] with pre-resolved imports, in source preorder.
///
/// Paths that `same_file` classifies as same-file references resolve locally;
/// any other path is passed to `resolve_external`, which either resolves it,
/// rejects it (an unresolvable body path is an error, exactly like a
/// file-root one), or — only in a load without cross-file resolution —
/// leaves it without a target.
///
/// # Errors
///
/// Returns the first error in source preorder: a body path of a package file
/// spelled as the file's own stem (M037), or an error `resolve_external`
/// reports.
pub(super) fn lift_inline_dags<ResolveExternal>(
    ast: &File,
    file_dag_id: &DagId,
    same_file: &SameFileScope,
    src: &RegisteredSource,
    resolve_external: ResolveExternal,
) -> Result<Vec<LoadedDag<ModuleTarget>>, CompileError>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, CompileError>,
{
    let context = InlineDagLiftContext {
        file_dag_id,
        same_file,
        src,
        resolve_external,
    };
    let mut out = Vec::new();
    lift_inline_dags_from_declarations(
        &ast.declarations,
        file_dag_id,
        &mut Vec::new(),
        &context,
        &[],
        &mut out,
    )?;
    Ok(out)
}

fn lift_inline_dags_from_declarations<ResolveExternal>(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    lexical_parent_path: &mut Vec<DeclName>,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag<ModuleTarget>>,
) -> Result<(), CompileError>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, CompileError>,
{
    for (index, decl) in declarations.iter().enumerate() {
        let DeclKind::Dag(dag) = &decl.kind else {
            continue;
        };
        let dag_id = lexical_parent_id.inline_dag_child(dag.name.value.clone());
        lexical_parent_path.push(dag.name.value.clone());
        let resolved_imports =
            resolve_inline_body_imports(&dag.body, lexical_parent_path, context)?;
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
            interface: graphcal_compiler::ir::module_interface::ModuleInterface::new(&dag.body),
        });
        lift_inline_dags_from_declarations(
            &dag.body,
            &dag_id,
            lexical_parent_path,
            context,
            &body_path,
            out,
        )?;
        lexical_parent_path.pop();
    }
    Ok(())
}

/// Targets of the import/include paths in `body`, the body of the inline DAG
/// at `owner` (its path from the file root).
fn resolve_inline_body_imports<ResolveExternal>(
    body: &[Declaration],
    owner: &[DeclName],
    context: &InlineDagLiftContext<'_, ResolveExternal>,
) -> Result<HashMap<ModulePathKey, ModuleTarget>, CompileError>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, CompileError>,
{
    let mut resolved = HashMap::new();
    for path in body.iter().filter_map(|body_decl| match &body_decl.kind {
        DeclKind::Import(import_decl) => Some(import_decl.path()),
        DeclKind::Include(include_decl) => Some(&include_decl.path),
        _ => None,
    }) {
        let target = match context
            .same_file
            .classify(path, LexicalSite::DagBody(owner))
        {
            PathScope::SameFile(module) => Some(module.target_in(context.file_dag_id)),
            PathScope::Package => (context.resolve_external)(path)?,
            PathScope::RejectedStem { module_path } => {
                return Err(package_file_stem_error(
                    path,
                    module_path.as_ref(),
                    context.src,
                ));
            }
        };
        if let Some(target) = target {
            resolved.insert(path.key(), target);
        }
    }
    Ok(resolved)
}

/// A DAG-body path of a package file spelled as the file's own stem.
///
/// In a package the path names the package or a dependency, but standalone
/// files and earlier releases read it as the file itself, so it is rejected
/// rather than silently naming another module.
fn package_file_stem_error(
    path: &ModulePath,
    module_path: Option<&ModulePathKey>,
    src: &RegisteredSource,
) -> CompileError {
    let stem = path.leaf().name.atom().clone();
    let refer_to_file = module_path.map_or_else(
        || "This file has no module path, so a DAG body cannot import from it.".to_owned(),
        |module_path| format!("To refer to this file, write its module path `{module_path}`."),
    );
    CompileError::Load(LoadError::PackageFileStemInDagBody {
        help: format!(
            "in a package, a one-segment path names the package or one of its dependencies, \
             never the current file, so `{stem}` would name another module than earlier \
             releases did. {refer_to_file} To import from the package or dependency `{stem}`, \
             rename this file."
        ),
        stem,
        src: src.clone(),
        span: path.span().into(),
    })
}

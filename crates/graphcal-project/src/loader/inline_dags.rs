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

use super::module_path::ModuleTarget;
use super::same_file_reference::{LexicalSite, PathScope, SameFileScope};

struct InlineDagLiftContext<'a, ResolveExternal> {
    file_dag_id: &'a DagId,
    same_file: &'a SameFileScope,
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
/// Returns the first error `resolve_external` reports, in source preorder.
pub(super) fn lift_inline_dags<ResolveExternal, E>(
    ast: &File,
    file_dag_id: &DagId,
    same_file: &SameFileScope,
    resolve_external: ResolveExternal,
) -> Result<Vec<LoadedDag<ModuleTarget>>, E>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, E>,
{
    let context = InlineDagLiftContext {
        file_dag_id,
        same_file,
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

fn lift_inline_dags_from_declarations<ResolveExternal, E>(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    lexical_parent_path: &mut Vec<DeclName>,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag<ModuleTarget>>,
) -> Result<(), E>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, E>,
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
fn resolve_inline_body_imports<ResolveExternal, E>(
    body: &[Declaration],
    owner: &[DeclName],
    context: &InlineDagLiftContext<'_, ResolveExternal>,
) -> Result<HashMap<ModulePathKey, ModuleTarget>, E>
where
    ResolveExternal: Fn(&ModulePath) -> Result<Option<ModuleTarget>, E>,
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
        };
        if let Some(target) = target {
            resolved.insert(path.key(), target);
        }
    }
    Ok(resolved)
}

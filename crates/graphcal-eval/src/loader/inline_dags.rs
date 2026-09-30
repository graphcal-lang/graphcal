//! Inline-DAG lifting shared by every loader mode.
//!
//! Loader modes differ only in how an import outside the current file
//! resolves, which callers supply as `resolve_external`. DAG discovery,
//! source-body location, same-file lookup, and file-root self-import handling
//! are one algorithm here.

use graphcal_compiler::syntax::decl_name::DeclName;

use std::collections::{HashMap, HashSet};

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{Declaration, File};
use graphcal_compiler::syntax::ast::{DeclKind, ModulePath};

use super::loaded_file::{DagBodyLocator, LoadedDag};
use super::module_path::{InlineBodyImportResolution, ModulePathKey, ResolvedModuleTarget};

struct InlineDagLiftContext<'a, ResolveExternal> {
    file_dag_id: &'a DagId,
    same_file_dag_ids: &'a HashSet<DagId>,
    file_stem: &'a str,
    resolve_external: ResolveExternal,
}

/// Walk inline `dag X { ... }` bodies of the file `file_dag_id` and lift each
/// into a [`LoadedDag`] with pre-resolved imports, in source preorder.
///
/// Same-file DAG references and `file_stem` self references resolve locally;
/// any other path is passed to `resolve_external`, and `None` records the
/// import as [`InlineBodyImportResolution::Unresolved`].
pub(super) fn lift_inline_dags<ResolveExternal>(
    ast: &File,
    file_dag_id: &DagId,
    file_stem: &str,
    resolve_external: ResolveExternal,
) -> Vec<LoadedDag>
where
    ResolveExternal: Fn(&ModulePath) -> Option<ResolvedModuleTarget>,
{
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, file_dag_id);
    let context = InlineDagLiftContext {
        file_dag_id,
        same_file_dag_ids: &same_file_dag_ids,
        file_stem,
        resolve_external,
    };
    let mut out = Vec::new();
    lift_inline_dags_from_declarations(&ast.declarations, file_dag_id, &context, &[], &mut out);
    out
}

fn lift_inline_dags_from_declarations<ResolveExternal>(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
    parent_path: &[usize],
    out: &mut Vec<LoadedDag>,
) where
    ResolveExternal: Fn(&ModulePath) -> Option<ResolvedModuleTarget>,
{
    declarations.iter().enumerate().for_each(|(index, decl)| {
        let DeclKind::Dag(dag) = &decl.kind else {
            return;
        };
        let dag_id = lexical_parent_id.inline_dag_child(dag.name.value.clone());
        let resolved_imports = resolve_inline_body_imports(&dag.body, &dag_id, context);
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
            interface: graphcal_compiler::ir::module_interface::ModuleInterface::new(&dag.body),
        });
        lift_inline_dags_from_declarations(&dag.body, &dag_id, context, &body_path, out);
    });
}

fn resolve_inline_body_imports<ResolveExternal>(
    body: &[Declaration],
    lexical_parent_id: &DagId,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
) -> HashMap<ModulePathKey, InlineBodyImportResolution>
where
    ResolveExternal: Fn(&ModulePath) -> Option<ResolvedModuleTarget>,
{
    body.iter()
        .filter_map(|body_decl| match &body_decl.kind {
            DeclKind::Import(import_decl) => Some(import_decl.path()),
            DeclKind::Include(include_decl) => Some(&include_decl.path),
            _ => None,
        })
        .map(|path| {
            (
                ModulePathKey::from_path(path),
                resolve_inline_body_import(path, lexical_parent_id, context),
            )
        })
        .collect()
}

fn resolve_inline_body_import<ResolveExternal>(
    path: &ModulePath,
    lexical_parent_id: &DagId,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
) -> InlineBodyImportResolution
where
    ResolveExternal: Fn(&ModulePath) -> Option<ResolvedModuleTarget>,
{
    let same_file_target =
        resolve_same_file_inline_dag_path(path, lexical_parent_id, context.same_file_dag_ids)
            .map(|target| ResolvedModuleTarget::in_file(context.file_dag_id.clone(), target));
    let file_root_target = (path.segments.len() == 1
        && path.segments[0].name.as_str() == context.file_stem)
        .then(|| ResolvedModuleTarget::file_root(context.file_dag_id.clone()));

    same_file_target
        .or(file_root_target)
        .or_else(|| (context.resolve_external)(path))
        .map_or(
            InlineBodyImportResolution::Unresolved,
            InlineBodyImportResolution::Resolved,
        )
}

fn resolve_same_file_inline_dag_path(
    path: &ModulePath,
    lexical_parent_id: &DagId,
    same_file_dag_ids: &HashSet<DagId>,
) -> Option<DagId> {
    let [leaf] = path.segments() else {
        return None;
    };
    let name = DeclName::classify(leaf.name.atom().clone());
    let child = lexical_parent_id.inline_dag_child(name.clone());
    if same_file_dag_ids.contains(&child) {
        return Some(child);
    }
    lexical_parent_id.parent().and_then(|parent| {
        let sibling = parent.inline_dag_child(name);
        same_file_dag_ids.contains(&sibling).then_some(sibling)
    })
}

pub(super) fn collect_inline_dag_ids(
    declarations: &[Declaration],
    lexical_parent_id: &DagId,
) -> HashSet<DagId> {
    declarations
        .iter()
        .flat_map(|decl| match &decl.kind {
            DeclKind::Dag(dag) => {
                let dag_id = lexical_parent_id.inline_dag_child(dag.name.value.clone());
                let mut ids = collect_inline_dag_ids(&dag.body, &dag_id);
                ids.insert(dag_id);
                ids
            }
            _ => HashSet::new(),
        })
        .collect()
}

//! Shared inline-DAG lifting for package, loose-file, and virtual projects.
//!
//! The three loader modes differ only in how an import outside the current
//! file resolves. DAG discovery, source-body location, same-file lookup, and
//! file-root self-import handling are one algorithm here.

use super::{
    Arc, DagBodyLocator, DagId, DeclKind, Declaration, File, FileSystemReader, HashMap, HashSet,
    InlineBodyImportResolution, LoadedDag, ModulePath, ModulePathKey, NamedSource,
    PackageInlineLiftContext, PackageManifest, Path, PathBuf, ResolvedModuleTarget,
    resolve_import_path, resolve_package_import_path,
};

struct InlineDagLiftContext<'a, ResolveExternal> {
    file_dag_id: &'a DagId,
    same_file_dag_ids: &'a HashSet<DagId>,
    file_stem: &'a str,
    resolve_external: ResolveExternal,
}

pub(super) fn lift_package_inline_dags(
    ast: &File,
    self_dag_id: &DagId,
    context: &PackageInlineLiftContext<'_>,
) -> Vec<LoadedDag> {
    let resolve_external = |path: &ModulePath| {
        let resolved =
            resolve_package_import_path(path, context.package_id, context.context, context.src)
                .ok()?;
        let source_file =
            if resolved.path == context.canonical_path && resolved.package == *context.package_id {
                Some(context.file_dag_id.clone())
            } else {
                context
                    .path_to_dag_id
                    .get(&(resolved.package.clone(), resolved.path.clone()))
                    .cloned()
            }?;
        Some(resolved.target_from(&source_file))
    };
    lift_inline_dags_with(
        ast,
        self_dag_id,
        &InlineDagLiftContext {
            file_dag_id: context.file_dag_id,
            same_file_dag_ids: context.same_file_dag_ids,
            file_stem: context.file_stem,
            resolve_external,
        },
    )
}

/// Walk inline `dag X { ... }` bodies and lift each into a [`LoadedDag`] with
/// pre-resolved imports for a loose filesystem project.
#[expect(
    clippy::too_many_arguments,
    reason = "loader-side resolution needs the same context as file-level imports"
)]
pub(super) fn lift_inline_dags<F: FileSystemReader>(
    ast: &File,
    self_dag_id: &DagId,
    canonical_path: &Path,
    project_root: &Path,
    src: &NamedSource<Arc<String>>,
    manifest: Option<&PackageManifest>,
    path_to_dag_id: &HashMap<PathBuf, DagId>,
    fs: &F,
) -> Vec<LoadedDag> {
    let file_stem = canonical_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, self_dag_id);
    let resolve_external = |path: &ModulePath| {
        let resolved = resolve_import_path(path, project_root, src, manifest, fs).ok()?;
        let source_file = if resolved.file == canonical_path {
            Some(self_dag_id.clone())
        } else {
            path_to_dag_id.get(&resolved.file).cloned()
        }?;
        Some(resolved.target_from(&source_file))
    };
    lift_inline_dags_with(
        ast,
        self_dag_id,
        &InlineDagLiftContext {
            file_dag_id: self_dag_id,
            same_file_dag_ids: &same_file_dag_ids,
            file_stem,
            resolve_external,
        },
    )
}

/// Stem-only variant for [`LoadedProject::from_source`], where no filesystem
/// resolver is available.
pub(super) fn lift_inline_dags_by_stem(
    ast: &File,
    path: &Path,
    self_dag_id: &DagId,
) -> Vec<LoadedDag> {
    let file_stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let same_file_dag_ids = collect_inline_dag_ids(&ast.declarations, self_dag_id);
    lift_inline_dags_with(
        ast,
        self_dag_id,
        &InlineDagLiftContext {
            file_dag_id: self_dag_id,
            same_file_dag_ids: &same_file_dag_ids,
            file_stem,
            resolve_external: |_path: &ModulePath| None,
        },
    )
}

fn lift_inline_dags_with<ResolveExternal>(
    ast: &File,
    self_dag_id: &DagId,
    context: &InlineDagLiftContext<'_, ResolveExternal>,
) -> Vec<LoadedDag>
where
    ResolveExternal: Fn(&ModulePath) -> Option<ResolvedModuleTarget>,
{
    let mut out = Vec::new();
    lift_inline_dags_from_declarations(&ast.declarations, self_dag_id, context, &[], &mut out);
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
        let dag_id = lexical_parent_id.child(dag.name.value.as_str());
        let resolved_imports = resolve_inline_body_imports(&dag.body, &dag_id, context);
        let mut body_path = parent_path.to_vec();
        body_path.push(index);
        out.push(LoadedDag {
            dag_id: dag_id.clone(),
            parent_dag_id: context.file_dag_id.clone(),
            body_locator: DagBodyLocator::at_child(parent_path, index),
            resolved_imports,
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
            DeclKind::Import(import_decl) => Some(&import_decl.path),
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
    let file_root_target = (path.segments.len() == 1 && path.segments[0].name == context.file_stem)
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
    let child = lexical_parent_id.child(leaf.name.as_str());
    if same_file_dag_ids.contains(&child) {
        return Some(child);
    }
    lexical_parent_id.parent().and_then(|parent| {
        let sibling = parent.child(leaf.name.as_str());
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
                let dag_id = lexical_parent_id.child(dag.name.value.as_str());
                let mut ids = collect_inline_dag_ids(&dag.body, &dag_id);
                ids.insert(dag_id);
                ids
            }
            _ => HashSet::new(),
        })
        .collect()
}

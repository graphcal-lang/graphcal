//! Definitions and export surfaces reachable through a file's imports.

use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use crate::project_symbols::{ProjectDocumentSymbols, ProjectSymbolIndex};
use crate::symbol_identity::{SourceSymbolPath, VisibleBinding};
use crate::symbol_table::{self, DefinitionInfo, SymbolCategory, SymbolKey, SymbolTable};
use graphcal_compiler::cancellation::{CancellationToken, Cancelled};
use graphcal_compiler::syntax::names::NameAtom;

/// A definition from an imported file, for cross-file go-to-definition and hover.
pub struct ImportedDefinition {
    /// URI of the file containing the definition.
    pub uri: Url,
    /// Source text of the imported file (needed for span-to-range conversion).
    /// Shared via `Arc` to avoid cloning the full source per imported symbol.
    pub source: Arc<String>,
    /// The definition info (name, category, spans, type description).
    pub definition: DefinitionInfo,
}

/// Collect canonical export surfaces for every import path in the root file.
pub fn collect_import_surfaces(
    project: &graphcal_project::loader::LoadedProject,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    cancellation: &CancellationToken,
) -> std::result::Result<
    HashMap<
        graphcal_compiler::syntax::non_empty::NonEmpty<String>,
        Vec<graphcal_compiler::resolve::exports::ExportedImportItem>,
    >,
    Cancelled,
> {
    cancellation.checkpoint()?;
    let mut surfaces = HashMap::new();
    let root_file = project.root_file();

    for (_, import, target) in root_file.imports_with_targets() {
        cancellation.checkpoint()?;
        let Ok(items) = module_resolver.exported_import_items(target.target()) else {
            continue;
        };
        let path = import
            .path()
            .segments
            .clone()
            .map(|segment| segment.name.to_string());
        surfaces.insert(path, items);
    }
    Ok(surfaces)
}

/// Collect canonical definitions, visible bindings, and occurrences for every
/// source in one loader-resolved project closure.
///
/// Definitions remain keyed by canonical semantic identity. Authored module
/// qualifiers and selective aliases are separate bindings, including
/// transitive re-exports resolved through the compiler's module scope.
pub struct ImportedSymbols {
    pub definitions: HashMap<SymbolKey, ImportedDefinition>,
    pub bindings: Vec<VisibleBinding>,
    pub project_index: ProjectSymbolIndex,
}

pub struct BuiltProjectDocument {
    pub uri: Url,
    pub source: Arc<String>,
    pub table: SymbolTable,
}

pub fn build_project_symbol_documents(
    root_uri: &Url,
    project: &graphcal_project::loader::LoadedProject,
    tir: Option<&graphcal_compiler::tir::typed::CheckedTir>,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    cancellation: &CancellationToken,
) -> std::result::Result<HashMap<graphcal_compiler::dag_id::DagId, BuiltProjectDocument>, Cancelled>
{
    project
        .files()
        .iter()
        .map(|loaded_file| {
            let file_id = loaded_file.dag_id();
            cancellation.checkpoint()?;
            let mut table = symbol_table::build_from_ast(
                loaded_file.ast(),
                loaded_file.source(),
                file_id,
                module_resolver,
            );
            if let Some(tir) = tir {
                symbol_table::enrich_from_tir(&mut table, tir, file_id);
            }
            let uri = if file_id == project.root_id() {
                root_uri.clone()
            } else {
                loaded_file_uri(loaded_file, root_uri)
            };
            Ok((
                file_id.clone(),
                BuiltProjectDocument {
                    uri,
                    source: Arc::clone(loaded_file.source()),
                    table,
                },
            ))
        })
        .collect()
}

/// The names one `import` / `include` introduces into its owner.
#[derive(Clone, Copy)]
pub enum ImportedNames<'a> {
    Selective(&'a [graphcal_compiler::desugar::desugared_ast::ImportItem]),
    Module(
        Option<
            &'a graphcal_compiler::syntax::span::Spanned<
                graphcal_compiler::syntax::module_name::ModuleAliasName,
            >,
        >,
    ),
}

impl<'a> ImportedNames<'a> {
    pub fn of_import(import: &'a graphcal_compiler::desugar::desugared_ast::ImportDecl) -> Self {
        match import {
            graphcal_compiler::desugar::desugared_ast::ImportDecl::Selective { items, .. } => {
                Self::Selective(items)
            }
            graphcal_compiler::desugar::desugared_ast::ImportDecl::Module { alias, .. } => {
                Self::Module(alias.as_ref())
            }
        }
    }

    pub fn of_include(kind: &'a graphcal_compiler::desugar::desugared_ast::ImportKind) -> Self {
        match kind {
            graphcal_compiler::desugar::desugared_ast::ImportKind::Selective(items) => {
                Self::Selective(items)
            }
            graphcal_compiler::desugar::desugared_ast::ImportKind::Module { alias } => {
                Self::Module(alias.as_ref())
            }
        }
    }
}

pub fn collect_file_imported_symbols(
    file_id: &graphcal_compiler::dag_id::DagId,
    loaded_file: &graphcal_project::loader::LoadedFile,
    documents: &HashMap<graphcal_compiler::dag_id::DagId, BuiltProjectDocument>,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    cancellation: &CancellationToken,
) -> std::result::Result<ImportedSymbols, Cancelled> {
    let mut imported = ImportedSymbols {
        definitions: HashMap::new(),
        bindings: Vec::new(),
        project_index: ProjectSymbolIndex::default(),
    };
    let imports = loaded_file
        .imports_with_targets()
        .map(|(_, decl, target)| (decl.path(), ImportedNames::of_import(decl), target, true));
    let includes = loaded_file
        .includes_with_targets()
        .map(|(_, decl, target)| {
            (
                &decl.path,
                ImportedNames::of_include(&decl.kind),
                target,
                false,
            )
        });
    for (path, names, resolved_module, is_import) in imports.chain(includes) {
        cancellation.checkpoint()?;
        let Some(target) = documents.get(resolved_module.source_file()) else {
            continue;
        };
        match names {
            ImportedNames::Selective(items) => {
                if is_import {
                    imported.bindings.extend(items.iter().filter_map(|item| {
                        resolve_selective_binding(file_id, item, module_resolver)
                    }));
                }
                collect_selective_import_definitions(
                    &mut imported,
                    &target.table,
                    items,
                    resolved_module.target(),
                    &target.uri,
                    &target.source,
                    cancellation,
                )?;
            }
            ImportedNames::Module(alias) => {
                let module_name = alias.map_or_else(
                    || path.leaf().name.atom().clone(),
                    |alias_ident| alias_ident.value.atom().clone(),
                );
                collect_module_import_definitions(
                    &mut imported,
                    &target.table,
                    &module_name,
                    resolved_module.target(),
                    &target.uri,
                    &target.source,
                    cancellation,
                )?;
            }
        }
    }
    Ok(imported)
}

pub fn collect_imported_definitions(
    root_uri: &Url,
    project: &graphcal_project::loader::LoadedProject,
    tir: Option<&graphcal_compiler::tir::typed::CheckedTir>,
    module_resolver: &graphcal_compiler::resolve::ModuleResolver,
    cancellation: &CancellationToken,
) -> std::result::Result<ImportedSymbols, Cancelled> {
    cancellation.checkpoint()?;
    let documents =
        build_project_symbol_documents(root_uri, project, tir, module_resolver, cancellation)?;
    let mut imported_by_file = project
        .files()
        .iter()
        .map(|loaded_file| {
            let file_id = loaded_file.dag_id();
            collect_file_imported_symbols(
                file_id,
                loaded_file,
                &documents,
                module_resolver,
                cancellation,
            )
            .map(|imported| (file_id.clone(), imported))
        })
        .collect::<std::result::Result<HashMap<_, _>, _>>()?;

    let project_documents = documents.into_iter().map(|(file_id, document)| {
        let bindings = imported_by_file
            .get(&file_id)
            .map_or_else(Vec::new, |imported| imported.bindings.clone());
        ProjectDocumentSymbols::new(document.uri, document.source, document.table, bindings)
    });
    let project_index = if root_uri.to_file_path().is_ok() {
        ProjectSymbolIndex::loaded_dependency_closure(root_uri.clone(), project_documents)
    } else {
        ProjectSymbolIndex::standalone(root_uri.clone(), project_documents)
    };
    let mut root_imported = imported_by_file
        .remove(project.root_id())
        .unwrap_or_else(|| ImportedSymbols {
            definitions: HashMap::new(),
            bindings: Vec::new(),
            project_index: ProjectSymbolIndex::default(),
        });
    for binding in &root_imported.bindings {
        if root_imported.definitions.contains_key(binding.target()) {
            continue;
        }
        let Some(definition) = project_index.definition(binding.target()) else {
            continue;
        };
        let Some(document) = project_index.document(&definition.occurrence.uri) else {
            continue;
        };
        root_imported.definitions.insert(
            binding.target().clone(),
            ImportedDefinition {
                uri: definition.occurrence.uri,
                source: Arc::clone(&document.source),
                definition: definition.definition.clone(),
            },
        );
    }
    root_imported.project_index = project_index;
    Ok(root_imported)
}

pub fn resolve_selective_binding(
    owner: &graphcal_compiler::dag_id::DagId,
    item: &graphcal_compiler::syntax::ast::ImportItem,
    resolver: &graphcal_compiler::resolve::ModuleResolver,
) -> Option<VisibleBinding> {
    use graphcal_compiler::syntax::ast::ImportItemNamespace;
    use graphcal_compiler::syntax::names::NamePath;

    let local = item.local_name_atom().clone();
    let path = NamePath::local(local.clone());
    let target = match item.namespace {
        ImportItemNamespace::Term => match resolver
            .resolve_decl_path(owner, &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
        {
            Ok(declaration) => SymbolKey::Declaration(declaration),
            Err(_) => SymbolKey::Constructor(
                resolver
                    .resolve_constructor_path(owner, &path)
                    .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
                    .ok()?,
            ),
        },
        ImportItemNamespace::Type => resolver
            .resolve_struct_type_path(owner, &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map(SymbolKey::StructType)
            .ok()?,
        ImportItemNamespace::Dimension => resolver
            .resolve_dimension_path(owner, &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map(SymbolKey::Dimension)
            .ok()?,
        ImportItemNamespace::Unit => resolver
            .resolve_unit_path(owner, &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map(SymbolKey::Unit)
            .ok()?,
        ImportItemNamespace::Index => resolver
            .resolve_index_path(owner, &path)
            .map(graphcal_compiler::resolve::symbols::SymbolRef::into_resolved)
            .map(SymbolKey::Index)
            .ok()?,
    };
    Some(VisibleBinding::new(target, SourceSymbolPath::local(local)))
}

pub fn loaded_file_uri(loaded_file: &graphcal_project::loader::LoadedFile, root_uri: &Url) -> Url {
    Url::from_file_path(loaded_file.path()).unwrap_or_else(|()| {
        #[expect(
            clippy::print_stderr,
            clippy::unnecessary_debug_formatting,
            reason = "developer-visible warning for an unreachable fallback"
        )]
        {
            eprintln!(
                "graphcal-lsp: Url::from_file_path failed for {:?}; falling back to root URI",
                loaded_file.path(),
            );
        }
        root_uri.clone()
    })
}

pub fn collect_selective_import_definitions(
    result: &mut ImportedSymbols,
    imported_table: &SymbolTable,
    items: &[graphcal_compiler::syntax::ast::ImportItem],
    target_module: &graphcal_compiler::dag_id::DagId,
    imported_uri: &Url,
    source: &Arc<String>,
    cancellation: &CancellationToken,
) -> std::result::Result<(), Cancelled> {
    for import_item in items {
        cancellation.checkpoint()?;
        for (key, definition) in &imported_table.definitions {
            cancellation.checkpoint()?;
            let Some(spelling) = selective_import_spelling(
                key,
                definition.category,
                import_item.namespace,
                target_module,
                import_item.name.name.as_str(),
                import_item.local_name_atom(),
            ) else {
                continue;
            };
            insert_imported_def(
                &mut result.definitions,
                key.clone(),
                imported_uri,
                source,
                definition,
            );
            result
                .bindings
                .push(VisibleBinding::new(key.clone(), spelling));
        }
    }
    Ok(())
}

pub fn collect_module_import_definitions(
    result: &mut ImportedSymbols,
    imported_table: &SymbolTable,
    module_name: &NameAtom,
    target_module: &graphcal_compiler::dag_id::DagId,
    imported_uri: &Url,
    source: &Arc<String>,
    cancellation: &CancellationToken,
) -> std::result::Result<(), Cancelled> {
    for (key, definition) in &imported_table.definitions {
        cancellation.checkpoint()?;
        let Some(spelling) =
            module_import_spelling(key, definition.category, module_name, target_module)
        else {
            continue;
        };
        insert_imported_def(
            &mut result.definitions,
            key.clone(),
            imported_uri,
            source,
            definition,
        );
        result
            .bindings
            .push(VisibleBinding::new(key.clone(), spelling));
    }
    Ok(())
}

/// Source-visible spelling contributed by one selective import.
///
/// The returned spelling is deliberately separate from `key`: aliases are
/// lexical bindings, not semantic identity. Attached members (index variants,
/// constructor fields, generic parameters, and DAG body declarations) retain
/// their parent as a structured qualifier.
pub fn selective_import_spelling(
    key: &SymbolKey,
    category: SymbolCategory,
    namespace: graphcal_compiler::desugar::desugared_ast::ImportItemNamespace,
    target_module: &graphcal_compiler::dag_id::DagId,
    original: &str,
    local: &NameAtom,
) -> Option<SourceSymbolPath> {
    if !selective_import_allows_category(namespace, category) {
        return None;
    }

    let primary = key.owner() == Some(target_module) && key.leaf_name() == original;
    if primary {
        return Some(SourceSymbolPath::local(local.clone()));
    }

    let attached = match key {
        SymbolKey::IndexVariant(id) => {
            id.index().owner() == target_module && id.index().as_str() == original
        }
        SymbolKey::Field(id) => {
            id.owner().owner() == target_module && id.owner().as_str() == original
        }
        SymbolKey::GenericParam(id) => {
            id.owner().owner() == target_module && id.owner().as_str() == original
        }
        SymbolKey::Declaration(name) => {
            name.owner().parent().as_ref() == Some(target_module)
                && name
                    .owner()
                    .leaf()
                    .inline_dag()
                    .is_some_and(|dag| dag.as_str() == original)
        }
        _ => false,
    };
    if !attached {
        return None;
    }
    let leaf = NameAtom::parse(key.leaf_name()).ok()?;
    match key {
        SymbolKey::IndexVariant(_) => Some(SourceSymbolPath::index_label(
            Vec::new(),
            local.clone(),
            leaf,
        )),
        SymbolKey::Field(_) | SymbolKey::GenericParam(_) => Some(SourceSymbolPath::associated(
            Vec::new(),
            local.clone(),
            leaf,
        )),
        _ => Some(SourceSymbolPath::module_member(vec![local.clone()], leaf)),
    }
}

pub const fn selective_import_allows_category(
    namespace: graphcal_compiler::desugar::desugared_ast::ImportItemNamespace,
    category: SymbolCategory,
) -> bool {
    match namespace {
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Term => matches!(
            category,
            SymbolCategory::Param
                | SymbolCategory::Node
                | SymbolCategory::Const
                | SymbolCategory::Constructor
                | SymbolCategory::Field
                | SymbolCategory::Assert
                | SymbolCategory::Plot
                | SymbolCategory::Figure
                | SymbolCategory::Layer
                | SymbolCategory::Dag
                | SymbolCategory::LocalVar
        ),
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Type => matches!(
            category,
            SymbolCategory::StructType | SymbolCategory::Field | SymbolCategory::GenericParam
        ),
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Dimension => {
            matches!(category, SymbolCategory::Dimension)
        }
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Unit => {
            matches!(category, SymbolCategory::Unit)
        }
        graphcal_compiler::desugar::desugared_ast::ImportItemNamespace::Index => {
            matches!(
                category,
                SymbolCategory::Index | SymbolCategory::IndexVariant
            )
        }
    }
}

/// Source-visible spelling contributed by a module import.
pub fn module_import_spelling(
    key: &SymbolKey,
    category: SymbolCategory,
    module_name: &NameAtom,
    target_module: &graphcal_compiler::dag_id::DagId,
) -> Option<SourceSymbolPath> {
    // Importing an inline DAG makes the DAG declaration itself callable by the
    // local module alias (`@alias(...)`).
    if let SymbolKey::Declaration(name) = key
        && target_module.parent().as_ref() == Some(name.owner())
        && target_module.leaf().inline_dag() == Some(&name.to_unowned_def_name())
    {
        return Some(SourceSymbolPath::local(module_name.clone()));
    }

    let owner = key.owner()?;
    let qualifier = module_relative_qualifier(module_name, owner, target_module)?;
    let leaf = NameAtom::parse(key.leaf_name()).ok()?;
    match key {
        SymbolKey::IndexVariant(id) => Some(SourceSymbolPath::index_label(
            qualifier,
            NameAtom::parse(id.index().as_str()).ok()?,
            leaf,
        )),
        SymbolKey::Field(id) => Some(SourceSymbolPath::associated(
            qualifier,
            NameAtom::parse(id.owner().as_str()).ok()?,
            leaf,
        )),
        SymbolKey::GenericParam(id) => Some(SourceSymbolPath::associated(
            qualifier,
            NameAtom::parse(id.owner().as_str()).ok()?,
            leaf,
        )),
        SymbolKey::Declaration(_) if category == SymbolCategory::Dag => {
            let mut segments = qualifier;
            segments.push(leaf);
            SourceSymbolPath::dag_path(segments)
        }
        _ => Some(SourceSymbolPath::module_member(qualifier, leaf)),
    }
}

pub fn module_relative_qualifier(
    module_name: &NameAtom,
    owner: &graphcal_compiler::dag_id::DagId,
    target_module: &graphcal_compiler::dag_id::DagId,
) -> Option<Vec<NameAtom>> {
    let relative = owner
        .scopes_below(target_module)?
        .into_iter()
        .map(|scope| scope.alias().map(|alias| alias.atom().clone()))
        .collect::<Option<Vec<_>>>()?;
    let mut qualifier = Vec::with_capacity(relative.len() + 1);
    qualifier.push(module_name.clone());
    qualifier.extend(relative);
    Some(qualifier)
}

pub fn insert_imported_def(
    result: &mut HashMap<SymbolKey, ImportedDefinition>,
    key: SymbolKey,
    uri: &Url,
    source: &Arc<String>,
    def: &DefinitionInfo,
) {
    result.insert(
        key,
        ImportedDefinition {
            uri: uri.clone(),
            source: Arc::clone(source),
            definition: def.clone(),
        },
    );
}

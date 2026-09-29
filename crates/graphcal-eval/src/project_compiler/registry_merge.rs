//! Frontend nominal-type seeding and concrete-instance type composition.
//!
//! Dimensions, units, and indexes are canonical definitions evaluated through
//! the module resolver; only syntax-backed nominal type definitions still
//! reach an importer's frontend type table by source name.

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;
use graphcal_compiler::syntax::ast::ImportItemNamespace;

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "type composition consumes project compiler model types"
)]
use super::*;

/// Install a template's nominal types, specialized through one include's
/// Static bindings, into the including module's frontend type table.
///
/// Bound types are replaced by the importer's own type and are skipped.
pub(super) fn merge_instance_types(
    types: &mut TypeRegistry,
    dep_types: &TypeRegistry,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) {
    for type_def in dep_types.all_types() {
        if type_bindings.contains_key(type_def.name()) {
            continue;
        }
        let mut specialized = type_def.clone();
        graphcal_compiler::ir::lower::specialize_type_definition(
            &mut specialized,
            index_bindings,
            type_bindings,
            dim_bindings,
        );
        types.register_type(specialized);
    }
}

/// Install imported nominal types into a module's frontend type table:
/// selective imports register the selected declarations from each
/// dependency's AST, projection aliases bind their local names, and module
/// imports merge each dependency's explicitly exported types.
///
/// Runs before the module's own types register.
pub(super) fn seed_imported_types(
    types: &mut TypeRegistry,
    project: &crate::loader::LoadedProject,
    imported_types: &HashMap<graphcal_compiler::dag_id::DagId, HashSet<StructTypeName>>,
    frontend_type_imports: &[FrontendTypeImport<'_>],
    projected_type_aliases: &[graphcal_compiler::syntax::span::Spanned<ProjectedTypeAlias>],
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (dep_dag_id, names) in imported_types {
        if let Some(dep_loaded) = project.files().get(dep_dag_id) {
            graphcal_compiler::ir::lower::register_selected_types(
                dep_loaded.ast(),
                types,
                dep_loaded.named_source(),
                names,
            )?;
        } else {
            let Some((owner_file, inline_dag)) = project.inline_dag(dep_dag_id) else {
                return Err(GraphcalError::internal_error(
                    format!("selected type owner `{dep_dag_id}` is unavailable"),
                    file_src,
                    DiagnosticAnchor::WholeFile,
                ));
            };
            let inline_body = graphcal_compiler::desugar::desugared_ast::File {
                declarations: inline_dag.body(owner_file).to_vec(),
            };
            graphcal_compiler::ir::lower::register_selected_types(
                &inline_body,
                types,
                owner_file.named_source(),
                names,
            )?;
        }
    }
    for projection in projected_type_aliases {
        types
            .register_type_alias(
                projection.value.alias.clone(),
                projection.value.target.clone(),
            )
            .map_err(|error| GraphcalError::CyclicDependency {
                name: error.alias.to_string(),
                src: file_src.clone(),
                span: projection.span.into(),
            })?;
    }
    for import in frontend_type_imports {
        for type_def in import.types.all_types() {
            let name = type_def.name().atom();
            if import
                .pure_import_rejections
                .as_ref()
                .is_some_and(|rejections| rejections.rejects(name, ImportItemNamespace::Type))
                || !import.external_surface.is_static_explicit_export(name)
            {
                continue;
            }
            types.register_type(type_def.clone());
        }
    }
    Ok(())
}

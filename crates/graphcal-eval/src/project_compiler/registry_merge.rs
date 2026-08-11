//! Frontend registry seeding and concrete-instance registry composition.

use graphcal_compiler::diagnostic_anchor::DiagnosticAnchor;

#[allow(
    clippy::wildcard_imports,
    clippy::allow_attributes,
    reason = "registry composition consumes project compiler model types"
)]
use super::*;

pub(super) fn merge_registry_into_builder(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
) -> Result<(), UnitMergeConflict> {
    merge_registry_into_builder_filtered(
        builder,
        dep_registry,
        index_bindings,
        type_bindings,
        dim_bindings,
        None,
        None,
        DynamicUnitBoundary::ConcreteInstance,
    )
}

/// Merge imported type-system declarations into a registry builder: selective
/// imports register the selected declarations from each dependency's AST, and
/// module imports merge each dependency's `pub` registry entries under the
/// import alias.
///
/// Runs as the registry-seed hook of file lowering — before the file's own
/// declarations register — so local definitions (e.g. `const unit halfmile: Length
/// = 0.5 u.mile;`) resolve against the imported entries.
pub(super) fn seed_imported_type_system(
    builder: &mut RegistryBuilder,
    project: &crate::loader::LoadedProject,
    imported_type_system_names: &HashMap<
        graphcal_compiler::dag_id::DagId,
        graphcal_compiler::ir::lower::SelectedDeclarations,
    >,
    frontend_registry_imports: &[FrontendRegistryImport<'_>],
    module_artifacts: &HashMap<graphcal_compiler::dag_id::DagId, LoweringModuleInterface>,
    file_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    for (dep_dag_id, names) in imported_type_system_names {
        let dep_loaded = &project.files()[dep_dag_id];
        let artifact = module_artifacts.get(dep_dag_id).ok_or_else(|| {
            GraphcalError::internal_error(
                format!("HIR interface for imported module `{dep_dag_id}` is unavailable"),
                file_src,
                DiagnosticAnchor::WholeFile,
            )
        })?;
        register_selected_resolved_dimensions_and_units(
            builder,
            artifact.frontend_registry(),
            names,
            dep_loaded.named_source(),
        )?;
        graphcal_compiler::ir::lower::register_selected_declarations(
            dep_loaded.ast(),
            builder,
            dep_loaded.named_source(),
            &names.without_resolved_dimensions_and_units(),
            dep_dag_id,
        )?;
    }
    for import in frontend_registry_imports {
        merge_registry_into_builder_export_filtered(builder, import).map_err(|conflict| {
            GraphcalError::ConflictingImportedUnit {
                name: conflict.name,
                src: file_src.clone(),
                span: import.import_span.into(),
            }
        })?;
    }
    Ok(())
}

/// Import selected dimensions and units from the dependency's resolved registry.
///
/// Re-registering only a selected declaration's source AST loses sibling
/// dimensions used by a dimension definition (`Weighted = Base * Mass`) or by
/// a unit annotation (`point: Score`). Both declarations are closed semantic
/// values after the dependency compiles, so copy them together with the base
/// dimension metadata needed to preserve canonical identities and formatting.
fn register_selected_resolved_dimensions_and_units(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    selected: &graphcal_compiler::ir::lower::SelectedDeclarations,
    dep_src: &NamedSource<Arc<String>>,
) -> Result<(), GraphcalError> {
    fn register_base_dimension_metadata(
        builder: &mut RegistryBuilder,
        dep_registry: &Registry,
        dimension: &graphcal_compiler::dimension::Dimension,
    ) {
        for (base_id, _) in dimension.iter() {
            if let Some(base_name) = dep_registry.dimensions.base_dim_names().get(base_id) {
                // Multiple package instances may use the same display leaf for
                // distinct nominal base IDs. Register every ID's metadata even
                // though the flat boundary registry keeps only one leaf value.
                builder.register_base_dimension(
                    graphcal_compiler::syntax::dimension::DimName::expect_valid(base_name),
                    base_id.clone(),
                );
            }
            if let Some(symbol) = dep_registry.dimensions.base_dim_symbols().get(base_id) {
                builder.set_base_dim_symbol(base_id.clone(), symbol.clone());
            }
        }
    }

    for name in selected.dimensions() {
        let dimension = dep_registry
            .dimensions
            .get_dimension(name.as_str())
            .cloned()
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("resolved dependency registry is missing selected dimension `{name}`"),
                    dep_src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        register_base_dimension_metadata(builder, dep_registry, &dimension);
        builder.register_dimension(name.clone(), dimension);
    }

    for name in selected.units() {
        use graphcal_compiler::syntax::dimension::UnitRef;

        let unit_ref = UnitRef::local(name.clone());
        let info = dep_registry
            .units
            .get_unit(&unit_ref)
            .cloned()
            .ok_or_else(|| {
                GraphcalError::internal_error(
                    format!("resolved dependency registry is missing selected unit `{name}`"),
                    dep_src,
                    DiagnosticAnchor::WholeFile,
                )
            })?;
        register_base_dimension_metadata(builder, dep_registry, &info.dimension);
        builder.register_unit_with_scale(unit_ref, info.dimension, info.scale, info.constness);
    }

    Ok(())
}

/// Merge type-system declarations from a dependency's frozen registry into a
/// builder, restricted to its explicit export surface. Param input ports are
/// intentionally irrelevant here because they are not type-system exports.
fn merge_registry_into_builder_export_filtered(
    builder: &mut RegistryBuilder,
    import: &FrontendRegistryImport<'_>,
) -> Result<(), UnitMergeConflict> {
    merge_registry_into_builder_filtered(
        builder,
        import.registry,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        Some(import.external_surface),
        Some(&import.unit_alias),
        import.dynamic_unit_boundary,
    )
}

/// A unit reference reached the importing file with two different definitions.
///
/// Includes inline the dependency's body into the importer, so the bodies of
/// two included modules share one unit scope; a silent last-write-wins merge
/// would make their references resolve to whichever include happened to land
/// last. The conflict is surfaced as a loud error instead.
#[derive(Debug)]
pub(in crate::project_compiler) struct UnitMergeConflict {
    pub name: graphcal_compiler::syntax::dimension::UnitRef,
}

#[expect(
    clippy::too_many_arguments,
    reason = "registry merge keeps typed binding and module-boundary policies explicit"
)]
fn merge_registry_into_builder_filtered(
    builder: &mut RegistryBuilder,
    dep_registry: &Registry,
    index_bindings: &IndexBindings,
    type_bindings: &HashMap<StructTypeName, StructTypeName>,
    dim_bindings: &HashMap<DimName, DimName>,
    external_surface: Option<&ExternalDeclSurface>,
    unit_alias: Option<&ModuleAliasName>,
    dynamic_unit_boundary: DynamicUnitBoundary,
) -> Result<(), UnitMergeConflict> {
    // Import base-dimension metadata for display formatting and registry
    // invariants. This includes private transitive dependencies of exported
    // dimensions and units; module resolution still prevents those names from
    // becoming source-visible in the importer.
    for (id, name) in dep_registry.dimensions.base_dim_names() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        builder.register_base_dimension(
            graphcal_compiler::syntax::dimension::DimName::expect_valid(name),
            id.clone(),
        );
    }

    // Import named dimensions (derived dimensions like Velocity = Length/Time).
    for (name, dim) in dep_registry.dimensions.all_dimensions() {
        if dim_bindings.contains_key(name.as_str()) {
            continue;
        }
        if external_surface.is_some_and(|surface| {
            !surface.is_explicit_export(&DeclName::from_atom(name.atom().clone()))
        }) {
            continue;
        }
        builder.register_dimension(name.clone(), dim.clone());
    }

    // Import base dimension symbols (for SI unit string display).
    for (id, symbol) in dep_registry.dimensions.base_dim_symbols() {
        builder.set_base_dim_symbol(id.clone(), symbol.clone());
    }

    // Import units.
    //
    // Module imports (`unit_alias` present) expose only the dependency's own
    // `pub` units, re-keyed under the import alias (`alias.unit`); the
    // dependency's alias-qualified imports and non-pub units stay internal to
    // it, and nothing lands in the importer's bare unit scope. Bare names in
    // the importer come only from its own declarations, selective imports,
    // and the prelude.
    //
    // Include merges (`unit_alias` absent) copy the dependency's full unit
    // scope unchanged because the dependency's body is inlined into the
    // importer and its unit references must keep resolving. Re-merging an
    // identical definition (diamond includes, prelude units present in every
    // dep registry) is idempotent; a *different* definition under the same
    // reference is a conflict.
    for (name, info) in dep_registry.units.all_units() {
        if info.scale.is_dynamic() && !dynamic_unit_boundary.includes_dynamic_units() {
            continue;
        }
        let target = if let Some(alias) = unit_alias {
            if name.is_qualified() {
                continue;
            }
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(name.name().atom().clone()))
            }) {
                continue;
            }
            graphcal_compiler::syntax::dimension::UnitRef::qualified(
                alias.clone(),
                name.name().clone(),
            )
        } else {
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(name.name().atom().clone()))
            }) {
                continue;
            }
            name.clone()
        };
        let merged_scale = info.scale.clone();
        let constness = info.constness;
        if let Some(existing) = builder.get_unit(&target) {
            if unit_definitions_compatible(existing, &info.dimension, &merged_scale, constness) {
                continue;
            }
            return Err(UnitMergeConflict { name: target });
        }
        builder.register_unit_with_scale(target, info.dimension.clone(), merged_scale, constness);
    }

    // Import indexes — skip bound indexes (they are replaced by the importer's index).
    // Module imports use explicit-export filtering and keep required indexes in the
    // dependency's frontend module scope only; pulling an unbound `pub(bind)` index
    // into the importer would incorrectly make the importer a library even if
    // it only needs a qualified type from the dependency.
    for idx_def in dep_registry.indexes.declared_indexes() {
        if external_surface.is_some() && idx_def.is_required() {
            continue;
        }
        if !index_bindings.contains_key(idx_def.name.as_str()) {
            if external_surface.is_some_and(|surface| {
                !surface.is_explicit_export(&DeclName::from_atom(idx_def.name.atom().clone()))
            }) {
                continue;
            }
            builder.register_index(idx_def.clone());
        }
    }
    // Structural indexes have no module visibility or alias. Preserve their
    // typed identities so imported indexed types and values can resolve them.
    for finite_index in dep_registry.indexes.finite_indexes() {
        builder.ensure_finite_index(finite_index.cardinality());
    }

    // Import struct types — skip bound types (they are replaced by the importer's type).
    for type_def in dep_registry.types.all_types() {
        if type_bindings.contains_key(type_def.name().as_str()) {
            continue;
        }
        if external_surface.is_some_and(|surface| {
            !surface.is_explicit_export(&DeclName::from_atom(type_def.name().atom().clone()))
        }) {
            continue;
        }
        builder.register_type(type_def.clone());
    }
    Ok(())
}

/// Two unit definitions are compatible when they agree on dimension and
/// scale. Dynamic scales cannot be compared structurally; two dynamic
/// definitions are assumed to be the same declaration reached through a
/// diamond import (a genuinely different pair still differs in dimension or
/// base scale in practice).
fn unit_definitions_compatible(
    existing: &graphcal_compiler::registry::types::UnitInfo,
    dim: &graphcal_compiler::dimension::Dimension,
    scale: &graphcal_compiler::registry::types::UnitScale,
    constness: graphcal_compiler::syntax::ast::UnitConstness,
) -> bool {
    use graphcal_compiler::registry::types::UnitScale;
    if existing.dimension != *dim || existing.constness != constness {
        return false;
    }
    match (&existing.scale, scale) {
        (UnitScale::Static(a), UnitScale::Static(b))
        | (
            UnitScale::Dynamic {
                base_unit_scale: a, ..
            },
            UnitScale::Dynamic {
                base_unit_scale: b, ..
            },
        ) => a == b,
        (UnitScale::Static(_), UnitScale::Dynamic { .. })
        | (UnitScale::Dynamic { .. }, UnitScale::Static(_)) => false,
    }
}

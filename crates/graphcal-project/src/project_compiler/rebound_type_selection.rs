//! Selective-include check that every exposed template type depending on a
//! rebound port is selected, and therefore specialized (V008).

use std::collections::HashMap;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::desugar::desugared_ast::{DeclKind, Declaration};
use graphcal_compiler::ir::model::IncludeAliasDeclaration;
use graphcal_compiler::ir::static_dependencies::{
    IncludeSurfaceItem, ModuleDeclarations, StaticScope, UnspecializedReboundType,
    static_input_kind, unspecialized_rebound_types,
};
use graphcal_compiler::resolve::ModuleResolver;
use graphcal_compiler::semantic_error::SemanticError;
use graphcal_compiler::semantic_error::visibility::{ReboundTypeDependency, VisibilityError};
use graphcal_compiler::source_id::SourceId;
use graphcal_compiler::syntax::decl_name::DeclName;
use graphcal_compiler::syntax::names::{NameAtom, NamePath};
use graphcal_compiler::syntax::non_empty::NonEmpty;
use graphcal_compiler::syntax::span::Span;

use crate::compile_error::PipelineError;

use super::model::IncludeInstanceRequest;

/// The template one include instantiates, with the declarations and scope its
/// exposed signatures resolve in.
pub(super) struct IncludedTemplate<'a> {
    pub(super) owner: &'a DagId,
    pub(super) declarations: &'a [Declaration],
    pub(super) resolver: &'a ModuleResolver,
}

/// Reject a selective include that exposes a template type whose definition
/// depends on a port the include rebinds, unless it selects that type.
///
/// Only a selected type is projected as an importer-side specialization; an
/// unselected type keeps the template's definition, which disagrees with the
/// instance's values once a port it mentions is rebound.
pub(super) fn check_rebound_type_selection(
    template: &IncludedTemplate<'_>,
    request: &IncludeInstanceRequest,
    alias_declarations: &HashMap<DeclName, IncludeAliasDeclaration>,
    importer_src: SourceId,
    include_span: Span,
) -> Result<(), PipelineError> {
    let rebound = request.static_bindings.substitution();
    if rebound.is_empty() {
        return Ok(());
    }
    let scope = StaticScope::new(template.owner, template.resolver);
    let values = request
        .selective_names
        .iter()
        .flatten()
        .filter_map(|alias| {
            alias_declarations.get(&alias.original).map(|declaration| {
                (
                    alias.original.atom().clone(),
                    IncludeSurfaceItem::Value(&declaration.type_ann),
                )
            })
        });
    let types = request.selected_types.iter().filter_map(|selected| {
        template
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.kind {
                DeclKind::Type(type_decl) if type_decl.name.value == *selected => {
                    Some(&declaration.kind)
                }
                _ => None,
            })
            .map(|declaration| {
                (
                    selected.atom().clone(),
                    IncludeSurfaceItem::Static(declaration),
                )
            })
    });
    let selected = request
        .selected_types
        .iter()
        .filter_map(|selected| {
            template
                .resolver
                .resolve_struct_type_path(template.owner, &NamePath::local(selected.atom().clone()))
                .ok()
                .map(|symbol| symbol.resolved().clone())
        })
        .collect::<Vec<_>>();
    let missing = unspecialized_rebound_types(
        ModuleDeclarations::new(template.declarations, scope),
        values.chain(types),
        &rebound,
        &selected,
    );
    let dependency = |missing: &UnspecializedReboundType<NameAtom>| ReboundTypeDependency {
        nominal: missing.nominal().to_unowned_def_name(),
        port_kind: static_input_kind(missing.port()),
        port: missing.port().atom().clone(),
    };
    let Some((first, rest)) = missing.split_first() else {
        return Ok(());
    };
    let item = first.item().clone();
    let types = NonEmpty::new(dependency(first), rest.iter().map(dependency).collect());
    Err(PipelineError::Semantic(SemanticError::located(
        importer_src,
        include_span,
        VisibilityError::IncludeMustSelectReboundType { item, types },
    )))
}

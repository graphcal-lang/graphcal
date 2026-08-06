//! Presentation-only assembly for evaluated project outputs.

use graphcal_compiler::ir::resolve::DeclCategory;
use graphcal_compiler::syntax::module_name::ScopedName;

use crate::eval::types::{DeclType, EvalResult, NodeError, Value};
use crate::project_compiler::IncludeDebugNameMap;

/// One typed value in normal result assembly.
pub(super) type OutputValue = (ScopedName, Result<Value, NodeError>, DeclType);

pub(super) const fn output_decl_type(category: DeclCategory) -> Option<DeclType> {
    match category {
        DeclCategory::Const => Some(DeclType::Const),
        DeclCategory::Param => Some(DeclType::Param),
        DeclCategory::Node => Some(DeclType::Node),
        DeclCategory::Assert | DeclCategory::Plot | DeclCategory::Figure | DeclCategory::Layer => {
            None
        }
    }
}

pub(super) fn remap_include_debug_name(
    name: &ScopedName,
    aliases: &IncludeDebugNameMap,
) -> ScopedName {
    let Some((first, rest)) = name.qualifier().split_first() else {
        return name.clone();
    };
    let Some(display) = aliases.get(first) else {
        return name.clone();
    };
    ScopedName::qualified_path(
        std::iter::once(display.clone()).chain(rest.iter().cloned()),
        name.member().clone(),
    )
}

/// Replace private synthetic include scopes with unambiguous human-readable
/// target leaves at the presentation boundary.
pub(super) fn apply_include_debug_names(result: &mut EvalResult, aliases: &IncludeDebugNameMap) {
    if aliases.is_empty() {
        return;
    }

    result
        .consts
        .iter_mut()
        .chain(result.params.iter_mut())
        .chain(result.nodes.iter_mut())
        .for_each(|(name, _)| *name = remap_include_debug_name(name, aliases));
    result
        .all
        .iter_mut()
        .for_each(|(name, _, _)| *name = remap_include_debug_name(name, aliases));
    result.output_surface = std::mem::take(&mut result.output_surface)
        .into_iter()
        .map(|name| remap_include_debug_name(&name, aliases))
        .collect();
    result
        .assertions
        .iter_mut()
        .for_each(|(name, _, _)| *name = remap_include_debug_name(name, aliases));
    result
        .plots
        .iter_mut()
        .for_each(|plot| plot.name = remap_include_debug_name(&plot.name, aliases));
    result
        .plot_errors
        .iter_mut()
        .for_each(|plot| plot.name = remap_include_debug_name(&plot.name, aliases));
    result.figures.iter_mut().for_each(|figure| {
        figure.name = remap_include_debug_name(&figure.name, aliases);
        figure
            .plot_names
            .iter_mut()
            .for_each(|name| *name = remap_include_debug_name(name, aliases));
    });
    result.layers.iter_mut().for_each(|layer| {
        layer.name = remap_include_debug_name(&layer.name, aliases);
        layer
            .plot_names
            .iter_mut()
            .for_each(|name| *name = remap_include_debug_name(name, aliases));
    });
    result.assumes_map = std::mem::take(&mut result.assumes_map)
        .into_iter()
        .map(|(name, assumers)| {
            (
                remap_include_debug_name(&name, aliases),
                assumers
                    .into_iter()
                    .map(|assumer| remap_include_debug_name(&assumer, aliases))
                    .collect(),
            )
        })
        .collect();
    result.domain_constraints = std::mem::take(&mut result.domain_constraints)
        .into_iter()
        .map(|(name, constraint)| (remap_include_debug_name(&name, aliases), constraint))
        .collect();
}

pub(super) fn push_output_value(
    (name, result, decl_type): OutputValue,
    consts: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    params: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    nodes: &mut Vec<(ScopedName, Result<Value, NodeError>)>,
    all: &mut Vec<OutputValue>,
) {
    match decl_type {
        DeclType::Const => consts.push((name.clone(), result.clone())),
        DeclType::Param => params.push((name.clone(), result.clone())),
        DeclType::Node => nodes.push((name.clone(), result.clone())),
    }
    all.push((name, result, decl_type));
}

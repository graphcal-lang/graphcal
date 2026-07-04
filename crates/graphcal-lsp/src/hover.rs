//! textDocument/hover handler.

use graphcal_compiler::desugar::desugared_ast::BindableVisibility;
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::convert::LineIndex;
use crate::resolve::{SymbolLocation, resolve_symbol_at};
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory};

/// Resolve hover information for a position in an analyzed document.
pub fn hover(analysis: &AnalysisResult, offset: usize) -> Option<Hover> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    let definition = match &resolved.location {
        SymbolLocation::Local(def) => *def,
        SymbolLocation::Imported(imported) => &imported.definition,
    };
    let content = format_hover(definition);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: content,
        }),
        range: Some(LineIndex::new(&analysis.source).span_to_range(resolved.cursor_span)),
    })
}

/// Prefix for the declaration keyword in a hover label, based on visibility.
///
/// Returns `"pub "` for `Public`, `"pub(bind) "` for `PublicBind`, and
/// the empty string for `Private` or unknown visibility. `param` never
/// carries an annotation (axiom A5), so we always use the empty string
/// there regardless of the stored visibility.
const fn visibility_prefix(vis: Option<BindableVisibility>) -> &'static str {
    match vis {
        Some(BindableVisibility::Public) => "pub ",
        Some(BindableVisibility::PublicBind) => "pub(bind) ",
        Some(BindableVisibility::Private) | None => "",
    }
}

/// Format hover content for a definition.
fn format_hover(def: &DefinitionInfo) -> String {
    let vis = visibility_prefix(def.visibility);
    match def.category {
        SymbolCategory::Param => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            // `param` is always implicitly bindable (axiom A5) and never
            // carries a `pub` / `pub(bind)` annotation — drop the prefix.
            format!("```graphcal\nparam {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Node => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            format!("```graphcal\n{vis}node {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Const => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            format!("```graphcal\n{vis}const {}: {type_str}\n```", def.name)
        }
        SymbolCategory::Dimension => {
            let fallback = format!("dim {}", def.name);
            let desc = def.type_description.as_deref().unwrap_or(&fallback);
            format!("```graphcal\n{vis}{desc}\n```")
        }
        SymbolCategory::Unit => {
            let desc = def.type_description.as_deref().unwrap_or("");
            format!("```graphcal\n{vis}unit {}: {desc}\n```", def.name)
        }
        SymbolCategory::Index => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            format!(
                "```graphcal\n{vis}index {} = {desc}\n```\n(named index labels are index positions for access, keys, and match patterns)",
                def.name
            )
        }
        SymbolCategory::StructType => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            format!("```graphcal\n{vis}type {} = {desc}\n```", def.name)
        }
        SymbolCategory::Constructor => {
            let detail = def.type_description.as_deref().unwrap_or("constructor");
            format!("```graphcal\n{vis}{}(...)\n```\n{detail}", def.name)
        }
        SymbolCategory::IndexVariant => {
            let detail = def.detail.as_deref().unwrap_or("");
            format!("`{}` ({detail})", def.name)
        }
        SymbolCategory::Field => {
            format!("`{}`", def.name)
        }
        SymbolCategory::LocalVar => {
            let detail = def.detail.as_deref().unwrap_or("");
            if detail.is_empty() {
                format!("`{}`", def.name)
            } else {
                format!("`{}` ({detail})", def.name)
            }
        }
        SymbolCategory::BuiltinFn | SymbolCategory::ExternFn => {
            let fallback = format!("fn {}", def.name);
            let sig = def.type_description.as_deref().unwrap_or(&fallback);
            let detail = def.detail.as_deref().unwrap_or("");
            format!("```graphcal\n{sig}\n```\n{detail}")
        }
        SymbolCategory::BuiltinConst => {
            let type_str = def.type_description.as_deref().unwrap_or("Dimensionless");
            format!(
                "```graphcal\nconst {}: {type_str}\n```\n(builtin)",
                def.name
            )
        }
        SymbolCategory::Assert => {
            format!("```graphcal\n{vis}assert {}: Bool\n```", def.name)
        }
        SymbolCategory::Plot => {
            let type_str = def.type_description.as_deref().unwrap_or("plot");
            format!("```graphcal\n{vis}plot {}\n```\n{}", def.name, type_str)
        }
        SymbolCategory::Figure => {
            format!("```graphcal\n{vis}figure {}\n```", def.name)
        }
        SymbolCategory::Layer => {
            format!("```graphcal\n{vis}layer {}\n```", def.name)
        }
        SymbolCategory::Dag => {
            format!("```graphcal\n{vis}dag {} {{ ... }}\n```", def.name)
        }
    }
}

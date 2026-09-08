//! textDocument/hover handler.

use graphcal_compiler::desugar::desugared_ast::BindableVisibility;
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::client_capabilities::HoverFormat;
use crate::convert::LineIndex;
use crate::resolve::{SymbolLocation, resolve_symbol_at};
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory};

/// Resolve Markdown hover information for a position in an analyzed document.
///
/// Functional tests use the server's richest representation. Protocol callers
/// should use [`hover_with_format`] with the client's negotiated format.
#[cfg(test)]
fn hover(analysis: &AnalysisResult, offset: usize) -> Option<Hover> {
    hover_with_format(analysis, offset, HoverFormat::Markdown)
}

/// Resolve hover information in a client-supported markup format.
pub fn hover_with_format(
    analysis: &AnalysisResult,
    offset: usize,
    format: HoverFormat,
) -> Option<Hover> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    let definition = match &resolved.location {
        SymbolLocation::Local(def) => *def,
        SymbolLocation::Imported(imported) => &imported.definition,
    };
    let description = describe_hover(definition);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: match format {
                HoverFormat::PlainText => MarkupKind::PlainText,
                HoverFormat::Markdown => MarkupKind::Markdown,
            },
            value: description.render(format, definition.doc.as_deref()),
        }),
        range: Some(LineIndex::new(&analysis.source).span_to_range(resolved.cursor_span)),
    })
}

struct HoverDescription {
    synopsis: String,
    detail: Option<String>,
    inline: bool,
}

impl HoverDescription {
    const fn block(synopsis: String) -> Self {
        Self {
            synopsis,
            detail: None,
            inline: false,
        }
    }

    const fn inline(synopsis: String) -> Self {
        Self {
            synopsis,
            detail: None,
            inline: true,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(detail);
        }
        self
    }

    fn render(&self, format: HoverFormat, documentation: Option<&str>) -> String {
        let mut content = match (format, self.inline) {
            (HoverFormat::Markdown, true) => format!("`{}`", self.synopsis),
            (HoverFormat::Markdown, false) => {
                format!("```graphcal\n{}\n```", self.synopsis)
            }
            (HoverFormat::PlainText, _) => self.synopsis.clone(),
        };
        if let Some(detail) = &self.detail {
            content.push('\n');
            content.push_str(detail);
        }
        if let Some(documentation) = documentation {
            match format {
                HoverFormat::Markdown => content.push_str("\n\n---\n\n"),
                HoverFormat::PlainText => content.push_str("\n\n"),
            }
            content.push_str(documentation);
        }
        content
    }
}

/// Prefix for the declaration keyword in a hover label, based on visibility.
///
/// Returns `"pub "` for `Public`, `"pub(bind) "` for `PublicBind`, and
/// the empty string for `Private` or unknown visibility. `param` declares an
/// input port rather than an export, so it never carries this annotation.
const fn visibility_prefix(vis: Option<BindableVisibility>) -> &'static str {
    match vis {
        Some(BindableVisibility::Public) => "pub ",
        Some(BindableVisibility::PublicBind) => "pub(bind) ",
        Some(BindableVisibility::Private) | None => "",
    }
}

/// Describe hover content independently of the client's presentation format.
fn describe_hover(def: &DefinitionInfo) -> HoverDescription {
    let vis = visibility_prefix(def.visibility);
    match def.category {
        SymbolCategory::Param => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            // `param` declares an annotation-free input port rather than an
            // ordinary export, so drop any visibility prefix.
            HoverDescription::block(format!("param {}: {type_str}", def.name))
        }
        SymbolCategory::Node => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            HoverDescription::block(format!("{vis}node {}: {type_str}", def.name))
        }
        SymbolCategory::Const => {
            let type_str = def.type_description.as_deref().unwrap_or("(unknown type)");
            HoverDescription::block(format!("{vis}const {}: {type_str}", def.name))
        }
        SymbolCategory::Dimension => {
            let fallback = format!("dim {}", def.name);
            let desc = def.type_description.as_deref().unwrap_or(&fallback);
            HoverDescription::block(format!("{vis}{desc}"))
        }
        SymbolCategory::Unit => {
            let desc = def.type_description.as_deref().unwrap_or("");
            HoverDescription::block(format!("{vis}unit {}: {desc}", def.name))
        }
        SymbolCategory::Index => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            HoverDescription::block(format!("{vis}index {} = {desc}", def.name)).with_detail(
                "(named index labels are index positions for access, keys, and match patterns)",
            )
        }
        SymbolCategory::StructType => {
            let desc = def.type_description.as_deref().unwrap_or("...");
            HoverDescription::block(format!("{vis}type {} = {desc}", def.name))
        }
        SymbolCategory::Constructor => {
            let detail = def.type_description.as_deref().unwrap_or("constructor");
            HoverDescription::block(format!("{vis}{}(...)", def.name)).with_detail(detail)
        }
        SymbolCategory::IndexVariant | SymbolCategory::LocalVar => {
            HoverDescription::inline(def.name.clone())
                .with_detail(def.detail.as_deref().unwrap_or(""))
        }
        SymbolCategory::Field => HoverDescription::inline(def.name.clone()),
        SymbolCategory::GenericParam => {
            let sort = def.type_description.as_deref().unwrap_or("generic");
            let detail = def.detail.as_deref().unwrap_or("generic parameter");
            HoverDescription::block(format!("{}: {sort}", def.name)).with_detail(detail)
        }
        SymbolCategory::BuiltinFn | SymbolCategory::ExternFn => {
            let fallback = format!("fn {}", def.name);
            let sig = def.type_description.as_deref().unwrap_or(&fallback);
            HoverDescription::block(sig.to_string())
                .with_detail(def.detail.as_deref().unwrap_or(""))
        }
        SymbolCategory::BuiltinConst => {
            let type_str = def.type_description.as_deref().unwrap_or("Dimensionless");
            HoverDescription::block(format!("const {}: {type_str}", def.name))
                .with_detail("(builtin)")
        }
        SymbolCategory::Assert => {
            HoverDescription::block(format!("{vis}assert {}: Bool", def.name))
        }
        SymbolCategory::Plot => {
            let type_str = def.type_description.as_deref().unwrap_or("plot");
            HoverDescription::block(format!("{vis}plot {}", def.name)).with_detail(type_str)
        }
        SymbolCategory::Figure => HoverDescription::block(format!("{vis}figure {}", def.name)),
        SymbolCategory::Layer => HoverDescription::block(format!("{vis}layer {}", def.name)),
        SymbolCategory::Dag => HoverDescription::block(format!("{vis}dag {} {{ ... }}", def.name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_extremum_builtins_hover() {
        let source = "\
index Case = { A, B };
param values: Dimensionless[Case] = for c: Case { 1.0 };
node lower: Dimensionless = least(1.0, 2.0);
node upper: Dimensionless = greatest(1.0, 2.0);
node low: Dimensionless = minimum(@values);
node high: Dimensionless = maximum(@values);
";
        let uri = tower_lsp::lsp_types::Url::parse("untitled:hover.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);

        for builtin in ["least", "greatest", "minimum", "maximum"] {
            let result = hover(&analysis, source.find(builtin).unwrap()).unwrap();
            let HoverContents::Markup(markup) = result.contents else {
                panic!("builtin hover should be Markdown");
            };
            assert!(
                markup.value.contains(&format!("fn {builtin}")),
                "hover should name canonical builtin `{builtin}`: {}",
                markup.value
            );
        }
    }

    #[test]
    fn doc_comment_renders_in_hover() {
        let source = "\
/// Specific impulse of the qualified engine.
/// Second line of the caption.
param isp: Dimensionless = 320.0;
node undocumented: Dimensionless = @isp * 2.0;
";
        let uri = tower_lsp::lsp_types::Url::parse("untitled:doc-hover.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);

        let result = hover(&analysis, source.find("isp:").unwrap()).unwrap();
        let HoverContents::Markup(markup) = result.contents else {
            panic!("param hover should be Markdown");
        };
        assert!(
            markup
                .value
                .contains("Specific impulse of the qualified engine.\nSecond line of the caption."),
            "hover should include the attached doc block: {}",
            markup.value
        );

        let result = hover(&analysis, source.find("undocumented:").unwrap()).unwrap();
        let HoverContents::Markup(markup) = result.contents else {
            panic!("node hover should be Markdown");
        };
        assert!(
            !markup.value.contains("Specific impulse"),
            "undocumented node must not inherit another declaration's doc: {}",
            markup.value
        );
    }

    #[test]
    fn indexed_recurrence_state_has_clean_analysis_and_multi_axis_hover() {
        let source = include_str!("../../../tests/fixtures/valid/indexed_state_recurrence.gcl");
        let uri = tower_lsp::lsp_types::Url::parse("untitled:indexed-recurrence.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean analysis, got diagnostics: {:?}",
            analysis.diagnostics
        );

        let offset = source.find("trajectory:").expect("trajectory declaration");
        let result = hover(&analysis, offset).expect("trajectory hover");
        let HoverContents::Markup(markup) = result.contents else {
            panic!("trajectory hover should be Markdown");
        };
        assert!(
            markup.value.contains("Dimensionless[TimeStep, Element]"),
            "hover should preserve source axis order: {}",
            markup.value
        );
    }
}

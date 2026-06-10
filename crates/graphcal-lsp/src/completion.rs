//! textDocument/completion handler.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{CompletionContext, determine_completion_context};
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory};

/// Top-level declaration keywords.
///
/// Mirrors the grammar keywords that can introduce a declaration at the file
/// level.
const TOP_LEVEL_KEYWORDS: &[&str] = &[
    "param", "node", "const", "type", "dim", "unit", "index", "assert", "dag", "plot", "figure",
    "layer", "import", "include",
];

/// Built-in type keywords available in type annotation position.
const TYPE_KEYWORDS: &[&str] = &["Dimensionless", "Bool", "Int", "Datetime"];

/// Iterate over all visible definitions: local (from symbol table) and imported.
fn all_definitions(analysis: &AnalysisResult) -> impl Iterator<Item = &DefinitionInfo> {
    let local = analysis.symbol_table.definitions.values();
    let imported = analysis
        .imported_definitions
        .values()
        .map(|imp| &imp.definition);
    local.chain(imported)
}

/// Produce completion items for the given cursor position.
///
/// `source` is the latest editor text (which may be newer than
/// `analysis.source`): the cursor context must reflect the just-typed
/// trigger character, while the items come from the cached analysis.
pub fn completion(
    analysis: &AnalysisResult,
    source: &str,
    offset: usize,
) -> Option<Vec<CompletionItem>> {
    let context = determine_completion_context(source, offset);

    let items = match context {
        CompletionContext::GraphRef => complete_graph_refs(analysis),
        CompletionContext::TypeAnnotation => complete_types(analysis),
        CompletionContext::TopLevel => complete_top_level(),
        CompletionContext::Expression => complete_expression(analysis),
    };

    if items.is_empty() { None } else { Some(items) }
}

/// Build completion items for definitions whose category maps to a kind
/// via `category_to_kind`. Definitions without a `name_span` are skipped
/// (they are synthetic / not user-visible).
fn build_definition_items(
    analysis: &AnalysisResult,
    category_to_kind: impl Fn(SymbolCategory) -> Option<CompletionItemKind>,
) -> Vec<CompletionItem> {
    all_definitions(analysis)
        .filter(|def| !def.name_span.is_empty())
        .filter_map(|def| {
            let kind = category_to_kind(def.category)?;
            Some(CompletionItem {
                label: def.name.clone(),
                kind: Some(kind),
                detail: def.type_description.clone(),
                ..Default::default()
            })
        })
        .collect()
}

/// Build completion items for static keyword lists (always `KEYWORD` kind).
fn keyword_items(keywords: &[&str]) -> Vec<CompletionItem> {
    keywords
        .iter()
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

/// Complete param, node, and const node names (after `@`).
fn complete_graph_refs(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    build_definition_items(analysis, |cat| match cat {
        SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const => {
            Some(CompletionItemKind::VARIABLE)
        }
        _ => None,
    })
}

/// Complete type names (after `:`).
fn complete_types(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    let mut items = keyword_items(TYPE_KEYWORDS);
    items.extend(build_definition_items(analysis, |cat| match cat {
        SymbolCategory::Dimension => Some(CompletionItemKind::CLASS),
        SymbolCategory::StructType => Some(CompletionItemKind::STRUCT),
        SymbolCategory::Index => Some(CompletionItemKind::ENUM),
        _ => None,
    }));
    items
}

/// Complete top-level keywords.
fn complete_top_level() -> Vec<CompletionItem> {
    keyword_items(TOP_LEVEL_KEYWORDS)
}

/// Complete expression-level items: constants, functions, boolean keywords.
fn complete_expression(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    let mut items = keyword_items(&["true", "false"]);
    items.extend(build_definition_items(analysis, |cat| match cat {
        SymbolCategory::Const | SymbolCategory::BuiltinConst => Some(CompletionItemKind::CONSTANT),
        SymbolCategory::BuiltinFn => Some(CompletionItemKind::FUNCTION),
        SymbolCategory::Constructor => Some(CompletionItemKind::CONSTRUCTOR),
        _ => None,
    }));
    items
}

#[cfg(test)]
mod tests {
    use super::{TOP_LEVEL_KEYWORDS, completion};

    #[test]
    fn top_level_keywords_do_not_include_removed_fn() {
        assert!(
            !TOP_LEVEL_KEYWORDS.contains(&"fn"),
            "`fn` was removed from the language; completions must not suggest it"
        );
    }

    #[test]
    fn top_level_keywords_include_core_decl_kinds() {
        for required in [
            "param", "node", "const", "type", "dim", "unit", "index", "dag", "plot", "figure",
            "layer", "import", "include",
        ] {
            assert!(
                TOP_LEVEL_KEYWORDS.contains(&required),
                "missing top-level keyword: {required}"
            );
        }
    }

    #[test]
    fn imported_symbol_completion_uses_local_alias() {
        // Regression: completion items for imported symbols used the
        // defining file's spelling — `import helper.lib.{y as renamed};`
        // offered `y`, which does not resolve in the importing file.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/helper")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"helper\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/helper/lib.gcl"),
            "pub const node y: Dimensionless = 2.0;",
        )
        .unwrap();
        let main_path = dir.path().join("src/helper/main.gcl");
        let main_text =
            "import helper.lib.{y as renamed};\nnode z: Dimensionless = @renamed + 1.0;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        // Cursor right after the `@` in `@renamed`.
        let offset = main_text.find("@renamed").unwrap() + 1;
        let items = completion(&analysis, main_text, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"renamed"),
            "completion must offer the local alias `renamed`: {labels:?}; \
             imported keys: {:?}",
            analysis.imported_definitions.keys().collect::<Vec<_>>()
        );
        assert!(
            !labels.contains(&"y"),
            "completion must not offer the original spelling `y`: {labels:?}"
        );
    }
}

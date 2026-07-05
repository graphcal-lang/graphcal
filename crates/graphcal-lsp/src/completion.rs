//! textDocument/completion handler.

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{CompletionContext, determine_completion_context};
use crate::server::AnalysisResult;
use crate::symbol_table::{DefinitionInfo, SymbolCategory, SymbolKey};

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
        CompletionContext::GraphRef => complete_graph_refs(analysis, offset),
        CompletionContext::TypeAnnotation => complete_types(analysis),
        CompletionContext::ConversionTarget => complete_conversion_targets(analysis),
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

/// Completion kind for a definition referenceable through `@`.
const fn graph_ref_kind(cat: SymbolCategory) -> Option<CompletionItemKind> {
    match cat {
        SymbolCategory::Param | SymbolCategory::Node | SymbolCategory::Const => {
            Some(CompletionItemKind::VARIABLE)
        }
        _ => None,
    }
}

/// Build a completion item for a graph-referenceable definition.
fn graph_ref_item(def: &DefinitionInfo) -> Option<CompletionItem> {
    if def.name_span.is_empty() {
        return None;
    }
    let kind = graph_ref_kind(def.category)?;
    Some(CompletionItem {
        label: def.name.clone(),
        kind: Some(kind),
        detail: def.type_description.clone(),
        ..Default::default()
    })
}

/// Complete param, node, and const node names (after `@`), respecting the
/// cursor's lexical scope.
///
/// Inside a `dag` body, top-level declarations are not referenceable: only
/// the dag's own params/nodes/consts (registered under `Qualified` keys
/// whose single qualifier segment is the dag name) and imported names are
/// offered. At the top level the dag members are excluded for the same
/// reason — offering identifiers that cannot compile is a usability trap.
fn complete_graph_refs(analysis: &AnalysisResult, offset: usize) -> Vec<CompletionItem> {
    let enclosing_dag = analysis.symbol_table.enclosing_dag_at(offset);

    let local = analysis
        .symbol_table
        .definitions
        .iter()
        .filter(|(key, _)| {
            enclosing_dag.map_or_else(
                // Top level: only top-level declarations (dag members are
                // registered under `Qualified` keys and not in scope here).
                || matches!(key, SymbolKey::TopLevel(_)),
                // Inside a dag body: only that dag's own members.
                |dag_name| {
                    matches!(
                        key,
                        SymbolKey::Qualified { module, .. }
                            if matches!(&module[..], [segment] if segment == dag_name)
                    )
                },
            )
        })
        .map(|(_, def)| def);
    // Imported names are referenceable in both scopes (a dag body may not
    // reach top-level declarations, but imports stay visible). Members of
    // imported dags (`Qualified` with more than one segment) need call
    // arguments and are not bare `@`-referenceable.
    let imported = analysis
        .imported_definitions
        .iter()
        .filter(|(key, _)| match key {
            SymbolKey::TopLevel(_) => true,
            SymbolKey::Qualified { module, .. } => module.len() == 1,
            _ => false,
        })
        .map(|(_, imp)| &imp.definition);

    local.chain(imported).filter_map(graph_ref_item).collect()
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

/// Complete unit names after `->` (conversion target, #648 U5).
///
/// Offers every in-scope unit: the prelude's plus user-defined and imported
/// `unit` declarations. The dimension checker rejects a wrong-dimension pick
/// with D006, so offering all units keeps the list useful while mid-edit
/// source (which often does not parse) cannot be type-inferred.
fn complete_conversion_targets(analysis: &AnalysisResult) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = graphcal_compiler::registry::prelude::PRELUDE_UNIT_NAMES
        .iter()
        .map(|name| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::UNIT),
            detail: Some("prelude unit".to_string()),
            ..Default::default()
        })
        .collect();
    items.extend(build_definition_items(analysis, |cat| match cat {
        SymbolCategory::Unit => Some(CompletionItemKind::UNIT),
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
        SymbolCategory::BuiltinFn | SymbolCategory::ExternFn => Some(CompletionItemKind::FUNCTION),
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
    fn conversion_target_offers_units() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let main_text = "const unit mile: Length = 1609.344 m;\n\
                         param a: Length = 1500.0 m;\n\
                         node b: Length = @a -> km;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        // Cursor right after `-> `, at the start of `km`.
        let offset = main_text.find("-> km").unwrap() + 3;
        let items = completion(&analysis, main_text, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["m", "km", "s", "mile"] {
            assert!(
                labels.contains(&expected),
                "conversion-target completion must offer `{expected}`: {labels:?}"
            );
        }
        assert!(
            !labels.contains(&"sqrt"),
            "conversion-target completion must not offer functions: {labels:?}"
        );
    }

    #[test]
    fn module_imported_unit_completes_as_qualified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/units.gcl"),
            "pub const unit mile: Length = 1609.344 m;",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let main_text = "import app.units as u;\n\
                         param a: Length = 3218.688 m;\n\
                         node b: Length = @a -> u.mile;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        let offset = main_text.find("-> u.mile").unwrap() + 3;
        let items = completion(&analysis, main_text, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        eprintln!("LABELS: {labels:?}");
        assert!(labels.contains(&"u.mile"));
        assert!(!labels.contains(&"mile"));
    }

    #[test]
    fn module_imported_dim_and_type_complete_as_qualified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/lib.gcl"),
            "pub dim Speed = Length / Time;\n\
             pub type Point { Point(x: Dimensionless, y: Dimensionless) }\n\
             pub index Axis = { X, Y };\n",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let main_text = "import app.lib as m;\nparam v: m.Speed = 3.0 m/s;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        // Cursor right after `: `, at the start of the type annotation.
        let offset = main_text.find(": m.Speed").unwrap() + 2;
        let items = completion(&analysis, main_text, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["m.Speed", "m.Point", "m.Axis"] {
            assert!(
                labels.contains(&expected),
                "type completion must offer the qualified `{expected}`: {labels:?}"
            );
        }
        for bare in ["Speed", "Point", "Axis"] {
            assert!(
                !labels.contains(&bare),
                "type completion must not offer the bare `{bare}`: {labels:?}"
            );
        }
    }

    #[test]
    fn module_imported_const_completes_as_qualified() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/lib.gcl"),
            "pub const node g0: Dimensionless = 9.81;",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let main_text = "import app.lib as m;\nnode z: Dimensionless = 1.0 + 2.0;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        // Cursor in expression position, right after `1.0 + `.
        let offset = main_text.find("+ 2.0").unwrap() + 2;
        let items = completion(&analysis, main_text, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"m.g0"),
            "expression completion must offer the qualified `m.g0`: {labels:?}"
        );
        assert!(
            !labels.contains(&"g0"),
            "expression completion must not offer the bare `g0`: {labels:?}"
        );
    }

    /// Issue #835: `@` completion respects lexical scope. Inside a `dag`
    /// body only the dag's own params/nodes are offered (a top-level name
    /// would not compile there); at the top level the dag's members are
    /// excluded for the same reason.
    #[test]
    fn graph_ref_completion_respects_dag_scope() {
        let source = "\
param outer: Mass = 1.0 kg;
dag d {
    param inner: Mass;
    node doubled: Mass = @inner * 2.0;
}
include d(inner: @outer).{ doubled as result };
";
        let uri = tower_lsp::lsp_types::Url::parse("untitled:test.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);

        // Inside the dag body, right after the `@` of `@inner`.
        let inside_offset = source.find("@inner").unwrap() + 1;
        let items = completion(&analysis, source, inside_offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["inner", "doubled"] {
            assert!(
                labels.contains(&expected),
                "dag-body completion must offer `{expected}`: {labels:?}"
            );
        }
        assert!(
            !labels.contains(&"outer"),
            "dag-body completion must not offer the out-of-scope top-level `outer`: {labels:?}"
        );

        // At the top level, right after the `@` of `@outer`.
        let top_offset = source.find("@outer").unwrap() + 1;
        let items = completion(&analysis, source, top_offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"outer"),
            "top-level completion must offer `outer`: {labels:?}"
        );
        for excluded in ["inner", "doubled"] {
            assert!(
                !labels.contains(&excluded),
                "top-level completion must not offer the dag member `{excluded}`: {labels:?}"
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

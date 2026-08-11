//! textDocument/completion handler.

use std::borrow::Cow;

use graphcal_compiler::dag_id::DagId;
use graphcal_compiler::syntax::lexer::tokenize;
use graphcal_compiler::syntax::names::NameAtom;
use graphcal_compiler::syntax::token::Token;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::cursor_context::{
    CompletionContext, CoordinateIndexCompletionContext, ImportItemCompletionContext,
    determine_completion_context, determine_coordinate_index_completion_context,
};
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

/// Built-ins available while editing a type annotation. `Fin` is offered for
/// nested Index positions such as `T[Fin(N)]`.
const TYPE_KEYWORDS: &[&str] = &[
    "Dimensionless",
    "Bool",
    "Int",
    "Datetime",
    "Complex",
    "Key",
    "Fin",
];

struct VisibleDefinition<'a> {
    label: Cow<'a, str>,
    definition: &'a DefinitionInfo,
}

/// Iterate over all visible definitions without treating an import alias as
/// semantic identity. Multiple bindings of one canonical target intentionally
/// produce multiple completion labels.
fn all_definitions(analysis: &AnalysisResult) -> impl Iterator<Item = VisibleDefinition<'_>> {
    let local = analysis
        .symbol_table
        .definitions
        .values()
        .map(|definition| VisibleDefinition {
            label: Cow::Borrowed(definition.name.as_str()),
            definition,
        });
    let imported = analysis.imported_bindings.iter().filter_map(|binding| {
        let imported = analysis.imported_definitions.get(binding.target())?;
        Some(VisibleDefinition {
            label: Cow::Owned(binding.spelling().to_string()),
            definition: &imported.definition,
        })
    });
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
    if let Some(context) = determine_coordinate_index_completion_context(source, offset) {
        let keywords = match context {
            CoordinateIndexCompletionContext::Constructor => &["range", "linspace"][..],
            CoordinateIndexCompletionContext::StepLabel => &["step"][..],
            CoordinateIndexCompletionContext::PointsLabel => &["points"][..],
        };
        return Some(keyword_items(keywords));
    }

    let context = determine_completion_context(source, offset);
    let items = match context {
        CompletionContext::ImportItem(context) => complete_import_items(analysis, &context),
        CompletionContext::GraphRef => complete_graph_refs(analysis, source, offset),
        CompletionContext::TypeAnnotation => complete_types(analysis),
        CompletionContext::ConversionTarget => complete_conversion_targets(analysis),
        CompletionContext::TopLevel => complete_top_level(),
        CompletionContext::Expression => complete_expression(analysis),
    };

    if items.is_empty() { None } else { Some(items) }
}

/// Completion kind for one exported import-surface category.
const fn exported_import_item_kind(
    kind: graphcal_compiler::syntax::module_resolve::ExportedImportItemKind,
) -> CompletionItemKind {
    use graphcal_compiler::syntax::module_resolve::{DeclSymbolKind, ExportedImportItemKind};

    match kind {
        ExportedImportItemKind::Decl(DeclSymbolKind::Const) => CompletionItemKind::CONSTANT,
        ExportedImportItemKind::Decl(DeclSymbolKind::Dag) => CompletionItemKind::FUNCTION,
        ExportedImportItemKind::Decl(DeclSymbolKind::Assert) => CompletionItemKind::EVENT,
        ExportedImportItemKind::Decl(
            DeclSymbolKind::Param | DeclSymbolKind::Node | DeclSymbolKind::Plot,
        ) => CompletionItemKind::VARIABLE,
        ExportedImportItemKind::Decl(DeclSymbolKind::Figure | DeclSymbolKind::Layer) => {
            CompletionItemKind::MODULE
        }
        ExportedImportItemKind::Constructor => CompletionItemKind::CONSTRUCTOR,
        ExportedImportItemKind::Dimension => CompletionItemKind::CLASS,
        ExportedImportItemKind::Unit => CompletionItemKind::UNIT,
        ExportedImportItemKind::Type => CompletionItemKind::STRUCT,
        ExportedImportItemKind::Index => CompletionItemKind::ENUM,
    }
}

/// Complete a selective import with canonical marker-bearing insert text.
fn complete_import_items(
    analysis: &AnalysisResult,
    context: &ImportItemCompletionContext,
) -> Vec<CompletionItem> {
    analysis
        .import_surfaces
        .get(&context.module_path)
        .into_iter()
        .flatten()
        .filter(|item| {
            context
                .namespace
                .is_none_or(|namespace| item.kind.namespace() == namespace)
        })
        .map(|item| {
            let insert = context
                .namespace
                .map_or_else(|| item.render(), |_| item.name.to_string());
            CompletionItem {
                label: insert.clone(),
                kind: Some(exported_import_item_kind(item.kind)),
                detail: Some(item.kind.namespace().to_string()),
                insert_text: Some(insert),
                ..Default::default()
            }
        })
        .collect()
}

/// Build completion items for definitions whose category maps to a kind
/// via `category_to_kind`. Source-less built-ins remain user-visible; other
/// synthetic definitions without a `name_span` are skipped.
fn build_definition_items(
    analysis: &AnalysisResult,
    category_to_kind: impl Fn(SymbolCategory) -> Option<CompletionItemKind>,
) -> Vec<CompletionItem> {
    all_definitions(analysis)
        .filter(|visible| {
            !visible.definition.name_span.is_empty() || visible.definition.is_builtin()
        })
        .filter_map(|visible| {
            let kind = category_to_kind(visible.definition.category)?;
            Some(CompletionItem {
                label: visible.label.into_owned(),
                kind: Some(kind),
                detail: visible.definition.type_description.clone(),
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
fn graph_ref_item(def: &DefinitionInfo, label: String) -> Option<CompletionItem> {
    if def.name_span.is_empty() {
        return None;
    }
    let kind = graph_ref_kind(def.category)?;
    Some(CompletionItem {
        label,
        kind: Some(kind),
        detail: def.type_description.clone(),
        ..Default::default()
    })
}

#[derive(Debug)]
enum BraceScope {
    Dag(NameAtom),
    Other,
}

#[derive(Debug, Default)]
enum DagHeader {
    #[default]
    None,
    AwaitingName,
    Named(NameAtom),
    Invalid,
}

/// Derive the current DAG owner from the latest token stream without relying
/// on declaration spans from a possibly older successful parse.
fn current_graph_owner(root: &DagId, source: &str, offset: usize) -> Option<DagId> {
    let mut braces = Vec::new();
    let mut dag_header = DagHeader::None;
    for &(token, span) in tokenize(source).tokens() {
        if span.offset() >= offset {
            break;
        }
        if token == Token::Dag {
            dag_header = DagHeader::AwaitingName;
            continue;
        }
        if matches!(dag_header, DagHeader::AwaitingName) {
            dag_header = if token.is_identifier() {
                source
                    .get(span.offset()..span.offset() + span.len())
                    .and_then(|spelling| NameAtom::parse(spelling).ok())
                    .map_or(DagHeader::Invalid, DagHeader::Named)
            } else {
                DagHeader::Invalid
            };
            continue;
        }
        if matches!(dag_header, DagHeader::Named(_)) && token != Token::LBrace {
            dag_header = DagHeader::Invalid;
        }
        match token {
            Token::LBrace => match std::mem::take(&mut dag_header) {
                DagHeader::None => braces.push(BraceScope::Other),
                DagHeader::Named(name) => braces.push(BraceScope::Dag(name)),
                DagHeader::AwaitingName | DagHeader::Invalid => return None,
            },
            Token::RBrace => {
                braces.pop()?;
                dag_header = DagHeader::None;
            }
            Token::Semicolon => dag_header = DagHeader::None,
            _ => {}
        }
    }

    Some(
        braces
            .into_iter()
            .fold(root.clone(), |owner, scope| match scope {
                BraceScope::Dag(name) => owner.child(name.as_str()),
                BraceScope::Other => owner,
            }),
    )
}

/// Complete param, node, and const node names (after `@`), respecting the
/// cursor's lexical scope.
///
/// Inside a `dag` body, top-level declarations are not referenceable: only
/// the dag's own params/nodes/consts (registered under `Qualified` keys
/// whose single qualifier segment is the dag name) and imported names are
/// offered. At the top level the dag members are excluded for the same
/// reason — offering identifiers that cannot compile is a usability trap.
fn complete_graph_refs(
    analysis: &AnalysisResult,
    source: &str,
    offset: usize,
) -> Vec<CompletionItem> {
    let Some(expected_owner) = current_graph_owner(analysis.symbol_table.owner(), source, offset)
    else {
        return Vec::new();
    };

    let local = analysis
        .symbol_table
        .definitions
        .iter()
        .filter(|(key, _)| key.owner() == Some(&expected_owner))
        .map(|(_, def)| (def, def.name.clone()));
    // Imported names are referenceable in both scopes (a dag body may not
    // reach top-level declarations, but imports stay visible). Members of
    // imported dags (`Qualified` with more than one segment) need call
    // arguments and are not bare `@`-referenceable.
    let imported = analysis.imported_bindings.iter().filter_map(|binding| {
        (binding.spelling().qualifier().len() <= 1)
            .then(|| analysis.imported_definitions.get(binding.target()))
            .flatten()
            .map(|imported| (&imported.definition, binding.spelling().to_string()))
    });

    local
        .chain(imported)
        .filter_map(|(definition, label)| graph_ref_item(definition, label))
        .collect()
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
    fn complex_type_and_functions_are_completed() {
        let source = "node z: Complex<Length> = complex(1.0 m, 2.0 m);";
        let uri = tower_lsp::lsp_types::Url::parse("file:///tmp/completion-complex.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);

        let type_items =
            completion(&analysis, source, source.find("Complex").unwrap()).unwrap_or_default();
        assert!(type_items.iter().any(|item| item.label == "Complex"));

        let expression_items =
            completion(&analysis, source, source.find("complex").unwrap()).unwrap_or_default();
        for function in [
            "complex",
            "polar",
            "to_complex",
            "re",
            "im",
            "phase",
            "conj",
        ] {
            assert!(
                expression_items.iter().any(|item| item.label == function),
                "missing complex function completion `{function}`"
            );
        }
    }

    #[test]
    fn stale_semantic_candidates_use_current_tolerant_dag_scope() {
        let analyzed = "const node top: Dimensionless = 1.0;\n\
                        dag inner {\n\
                            const node local: Dimensionless = 2.0;\n\
                            node result: Dimensionless = @local;\n\
                        }\n";
        let uri = tower_lsp::lsp_types::Url::parse("file:///tmp/completion-stale.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, analyzed);
        let current = format!("node incomplete:\r\n// UTF-16 shift 🚀\r\n{analyzed}");
        let offset = current.find("@local").unwrap() + 1;

        let labels = completion(&analysis, &current, offset)
            .unwrap_or_default()
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();

        assert!(labels.iter().any(|label| label == "local"));
        assert!(!labels.iter().any(|label| label == "top"));
    }

    #[test]
    fn index_positions_offer_fin_constructor() {
        let source = "param values: Dimensionless[Fin(3)] = table[Fin(3)] { 1.0; 2.0; 3.0; };\n";
        let uri = tower_lsp::lsp_types::Url::parse("file:///tmp/completion-fin.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);
        for offset in [source.find("Fin").unwrap(), source.rfind("Fin").unwrap()] {
            let items = completion(&analysis, source, offset).unwrap_or_default();
            assert!(
                items.iter().any(|item| item.label == "Fin"),
                "Index position should offer the explicit Fin constructor: {items:?}"
            );
        }
    }

    #[test]
    fn coordinate_index_positions_offer_contextual_constructor_words() {
        let source = "index ByStep = range(0.0, 1.0, step: 0.5);\nindex ByCount = linspace(0.0, 1.0, points: 3);\n";
        let uri =
            tower_lsp::lsp_types::Url::parse("file:///tmp/completion-coordinate.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);

        let constructors =
            completion(&analysis, source, source.find("range").unwrap()).unwrap_or_default();
        assert!(constructors.iter().any(|item| item.label == "range"));
        assert!(constructors.iter().any(|item| item.label == "linspace"));

        let step = completion(&analysis, source, source.find("step").unwrap()).unwrap_or_default();
        assert_eq!(
            step.iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["step"]
        );

        let points =
            completion(&analysis, source, source.find("points").unwrap()).unwrap_or_default();
        assert_eq!(
            points
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["points"]
        );
    }

    #[test]
    fn selective_import_completion_inserts_and_filters_category_markers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"app\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/lib.gcl"),
            "pub const node JPY: Dimensionless = 1.0;\n\
             pub base unit JPY: Dimensionless;\n\
             pub dim Information = Dimensionless;\n\
             pub type Student { Student }\n\
             pub index Category = { A };\n",
        )
        .unwrap();
        let main_path = dir.path().join("src/app/main.gcl");
        let analyzed_source = "import app.lib.{ JPY };\n";
        std::fs::write(&main_path, analyzed_source).unwrap();
        let uri = tower_lsp::lsp_types::Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, analyzed_source);

        let unmarked = "import app.lib.{ ";
        let labels = completion(&analysis, unmarked, unmarked.len())
            .unwrap_or_default()
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>();
        for expected in [
            "JPY",
            "Student",
            "type Student",
            "dim Information",
            "unit JPY",
            "index Category",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing canonical import completion `{expected}`: {labels:?}"
            );
        }

        let marked = "import app.lib.{ unit ";
        let items = completion(&analysis, marked, marked.len()).unwrap_or_default();
        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["JPY"]
        );
        assert_eq!(items[0].insert_text.as_deref(), Some("JPY"));
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
        for expected in ["m", "km", "s", "min", "h", "mile"] {
            assert!(
                labels.contains(&expected),
                "conversion-target completion must offer `{expected}`: {labels:?}"
            );
        }
        assert!(
            !labels.contains(&"hour"),
            "conversion-target completion must not offer the removed `hour` unit: {labels:?}"
        );
        assert!(
            !labels.contains(&"sqrt"),
            "conversion-target completion must not offer functions: {labels:?}"
        );
    }

    #[test]
    fn canonical_extremum_functions_complete() {
        let source = "node result: Dimensionless = greatest(1.0, 2.0);";
        let uri = tower_lsp::lsp_types::Url::parse("untitled:builtins.gcl").unwrap();
        let analysis = crate::server::run_analysis_for_test(&uri, source);
        let offset = source.find("greatest").unwrap();
        let items = completion(&analysis, source, offset).unwrap_or_default();
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();

        for expected in ["least", "greatest", "minimum", "maximum"] {
            assert!(
                labels.contains(&expected),
                "expression completion must offer `{expected}`: {labels:?}"
            );
        }
        for obsolete in ["min", "max"] {
            assert!(
                !labels.contains(&obsolete),
                "expression completion must not offer obsolete `{obsolete}()`: {labels:?}"
            );
        }
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

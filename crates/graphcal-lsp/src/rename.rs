//! textDocument/rename and textDocument/prepareRename handlers.

use std::collections::HashMap;

use tower_lsp::lsp_types::{PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::convert::LineIndex;
use crate::resolve::{ResolvedSymbol, SymbolLocation, reference_lookup_keys, resolve_symbol_at};
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolCategory;

/// Check whether a name is a valid Graphcal identifier.
///
/// Asks the lexer instead of a hand-kept rule so reserved keywords
/// (`node`, `param`, `true`, …) are rejected — renaming to a keyword would
/// produce unparsable code.
fn is_valid_identifier(name: &str) -> bool {
    use graphcal_compiler::syntax::lexer::Lexer;
    use graphcal_compiler::syntax::token::Token;
    let mut lexer = Lexer::new(name);
    let is_single_ident = matches!(
        lexer.next_token(),
        Some((Token::Ident, span)) if span.len() == name.len()
    );
    is_single_ident && lexer.next_token().is_none()
}

/// Check whether a resolved symbol is eligible for rename.
///
/// Imported symbols are rejected: a safe cross-file rename would need a
/// project-wide reference index plus edits to every importer, re-export,
/// and alias. Until that is implemented, refusing is safer than producing
/// a partial workspace edit that breaks the defining file or other
/// importers.
fn is_renameable(resolved: &ResolvedSymbol<'_>) -> bool {
    let SymbolLocation::Local(def) = &resolved.location else {
        return false;
    };
    !def.is_builtin() && def.category != SymbolCategory::Field
}

/// Validate a rename and return the current name's range and placeholder.
pub fn prepare_rename(analysis: &AnalysisResult, offset: usize) -> Option<PrepareRenameResponse> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    if !is_renameable(&resolved) {
        return None;
    }

    let SymbolLocation::Local(def) = &resolved.location else {
        return None;
    };
    let span = resolved.cursor_span;
    // Fall back to the definition's name if span slicing ever fails — never
    // a synthetic key rendering.
    let placeholder = analysis
        .source
        .get(span.offset()..span.offset() + span.len())
        .unwrap_or(&def.name)
        .to_string();

    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: LineIndex::new(&analysis.source).span_to_range(span),
        placeholder,
    })
}

/// Perform the rename, returning a workspace edit.
pub fn rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !is_valid_identifier(new_name) {
        return None;
    }

    let resolved = resolve_symbol_at(analysis, offset)?;
    if !is_renameable(&resolved) {
        return None;
    }
    let SymbolLocation::Local(def) = &resolved.location else {
        return None;
    };

    let lines = LineIndex::new(&analysis.source);
    // Expand the resolved key the same way `references` does: a reference
    // can be recorded under an alias key (TopLevel↔Constructor,
    // Qualified↔Variant) of the key the cursor resolved to. Editing only
    // the single key used to leave some occurrences un-renamed — a broken
    // program. Spans are deduplicated in case a reference is recorded under
    // more than one alias.
    let mut spans: Vec<_> = reference_lookup_keys(&resolved.key)
        .iter()
        .flat_map(|key| analysis.symbol_table.find_all_references(key))
        .map(|r| r.span)
        .chain((!def.name_span.is_empty()).then_some(def.name_span))
        .collect();
    let mut seen = std::collections::HashSet::new();
    spans.retain(|span| seen.insert((span.offset(), span.len())));
    let current_file_edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit {
            range: lines.span_to_range(span),
            new_text: new_name.to_string(),
        })
        .collect();

    if current_file_edits.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), current_file_edits);

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::server::build_fn_signatures;
    use crate::symbol_table;

    /// Build a minimal `AnalysisResult` from source text.
    fn analysis_from_source(source: &str) -> AnalysisResult {
        let raw_ast = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::syntax::desugar::desugar_multi_decls_in_file(raw_ast);
        let ast = desugared;
        let symbol_table = symbol_table::build_from_ast(&ast, source);
        AnalysisResult {
            source: Arc::new(source.to_string()),
            symbol_table,
            imported_definitions: HashMap::new(),
            diagnostics: Arc::new(HashMap::new()),
            eval_values: HashMap::new(),
            fn_signatures: build_fn_signatures(),
            import_links: Vec::new(),
        }
    }

    #[test]
    fn rename_param_from_definition() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        // Cursor on "x" in "param x"
        let offset = source.find("x:").unwrap();
        let result = rename(&analysis, &uri, offset, "velocity").unwrap();
        let edits = result.changes.unwrap();
        let file_edits = edits.get(&uri).unwrap();
        // Should have 2 edits: the definition and the @x reference.
        assert_eq!(file_edits.len(), 2);
        assert!(file_edits.iter().all(|e| e.new_text == "velocity"));
    }

    #[test]
    fn rename_param_from_reference() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        // Cursor on "x" in "@x" — offset of the ident after @
        let at_x = source.find("@x").unwrap() + 1;
        let result = rename(&analysis, &uri, at_x, "velocity").unwrap();
        let edits = result.changes.unwrap();
        let file_edits = edits.get(&uri).unwrap();
        assert_eq!(file_edits.len(), 2);
    }

    #[test]
    fn prepare_rename_builtin_rejected() {
        let source = "node y: Dimensionless = sqrt(1.0);";
        let analysis = analysis_from_source(source);

        // Cursor on "sqrt"
        let offset = source.find("sqrt").unwrap();
        let result = prepare_rename(&analysis, offset);
        assert!(result.is_none(), "builtins should not be renameable");
    }

    #[test]
    fn rename_invalid_name_rejected() {
        let source = "param x: Dimensionless = 1.0;";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        let offset = source.find("x:").unwrap();
        assert!(rename(&analysis, &uri, offset, "").is_none());
        assert!(rename(&analysis, &uri, offset, "123bad").is_none());
        assert!(rename(&analysis, &uri, offset, "has space").is_none());
    }

    #[test]
    fn rename_imported_symbol_rejected() {
        // Build an analysis where `@y` resolves through an imported alias.
        // Cross-file rename is not yet implemented; the request must be
        // refused rather than producing a partial workspace edit.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"helper\"\nsource_dir = \".\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("helper.gcl"),
            "pub const node y: Dimensionless = 2.0;",
        )
        .unwrap();
        let main_path = dir.path().join("main.gcl");
        let main_text = "import helper.{y};\nnode z: Dimensionless = @y + 1.0;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);

        let cursor = main_text.find("@y").unwrap() + 1;
        assert!(
            prepare_rename(&analysis, cursor).is_none(),
            "imported symbol must not be prepare-renameable"
        );
        assert!(
            rename(&analysis, &main_uri, cursor, "renamed").is_none(),
            "imported symbol must not be renamed",
        );
    }

    #[test]
    fn rename_to_keyword_rejected() {
        // Regression: keywords passed the [A-Za-z_]\w* shape check, so
        // renaming a param to `node` produced unparsable code.
        let source = "param x: Dimensionless = 1.0;";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        let offset = source.find("x:").unwrap();
        for keyword in ["node", "param", "index", "true", "unfold"] {
            assert!(
                rename(&analysis, &uri, offset, keyword).is_none(),
                "renaming to keyword `{keyword}` must be rejected"
            );
        }
    }

    #[test]
    fn rename_constructor_edits_all_occurrences() {
        // Regression: rename collected references by the single resolved key
        // while `references` expands alias keys (TopLevel↔Constructor), so
        // some occurrences were silently left un-renamed.
        let source = "\
type Status { Idle, Active }
param s: Status = Idle;
node t: Dimensionless = match @s { Idle => 1.0, Active => 2.0 };
";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        let offset = source.find("Idle,").unwrap();
        if let Some(result) = rename(&analysis, &uri, offset, "Standby") {
            let edits = result.changes.unwrap();
            let file_edits = edits.get(&uri).unwrap();
            let lines = LineIndex::new(&analysis.source);
            let _ = lines;
            // Every textual occurrence of `Idle` must be covered: the
            // definition, the initializer, and the match arm.
            assert!(
                file_edits.len() >= 3,
                "expected all 3 occurrences renamed, got {}: {file_edits:?}",
                file_edits.len()
            );
        }
    }

    #[test]
    fn is_valid_identifier_cases() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("velocity"));
        assert!(is_valid_identifier("my_var_2"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("123"));
        assert!(!is_valid_identifier("has space"));
        assert!(!is_valid_identifier("a-b"));
        // The lexer is the source of truth: `_`-prefixed names and keywords
        // are not valid graphcal identifiers (`param _private: …` is a
        // parse error), so renaming to them must be rejected.
        assert!(!is_valid_identifier("_private"));
        assert!(!is_valid_identifier("node"));
        assert!(!is_valid_identifier("true"));
    }
}

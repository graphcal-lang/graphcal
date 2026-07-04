//! textDocument/codeAction handler.
//!
//! Provides quick-fix code actions for visibility-related diagnostics:
//! - V002: Add `pub(bind)` to a required `index` / `type` / `dim`.
//! - V003: Add `pub` to a private item referenced by a public declaration.
//! - V006: Add `pub` to a leaked symbol referenced by a re-exported declaration.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Diagnostic, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::convert::LineIndex;
use crate::server::AnalysisResult;
use crate::symbol_table::SymbolKey;

/// All declaration keywords that can be preceded by `pub`.
///
/// Used by `find_keyword_position` to locate the insertion point for `pub `.
const ALL_DECL_KEYWORDS: &[&str] = &[
    "param", "node", "const", "index", "dim", "unit", "type", "base", "dag", "plot", "assert",
    "import", "include",
];

/// Produce code actions for the given diagnostics.
pub fn code_actions(
    params: &CodeActionParams,
    analysis: &AnalysisResult,
    current_source: &str,
) -> Option<CodeActionResponse> {
    let source = current_source;
    let mut actions = Vec::new();

    for diag in &params.context.diagnostics {
        let code = match &diag.code {
            Some(NumberOrString::String(s)) => s.as_str(),
            _ => continue,
        };

        match code {
            "graphcal::V002" => {
                if let Some(action) =
                    make_add_pub_bind_action_v002(diag, source, &params.text_document.uri)
                {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            // V003 and V006 share a fix shape: add `pub` to the private
            // item named in the diagnostic's structured data.
            "graphcal::V003" | "graphcal::V006" => {
                if let Some(action) =
                    make_add_pub_action(diag, analysis, current_source, &params.text_document.uri)
                {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            _ => {}
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// The referenced private name a V003/V006 diagnostic carries in its
/// structured `data` payload (attached in `diagnostics.rs`). The name is
/// typed end to end — no re-parsing of the rendered message.
fn referenced_name_from_data(diag: &Diagnostic) -> Option<String> {
    diag.data
        .as_ref()?
        .get("referencedName")?
        .as_str()
        .map(str::to_string)
}

/// Line (0-based) of the declaration named `name`, resolved through the
/// symbol table rather than by grepping source lines (which also matched
/// declaration-shaped text inside comments and string literals).
fn declaration_line(analysis: &AnalysisResult, name: &str) -> Option<u32> {
    let def = analysis
        .symbol_table
        .definitions
        .get(&SymbolKey::TopLevel(name.to_string()))?;
    if def.name_span.is_empty() {
        return None;
    }
    let lines = LineIndex::new(&analysis.source);
    Some(lines.span_to_range(def.name_span).start.line)
}

/// For V003/V006: read the private name from the diagnostic data, find its
/// declaration via the symbol table, and insert `pub ` before the keyword.
fn make_add_pub_action(
    diag: &Diagnostic,
    analysis: &AnalysisResult,
    current_source: &str,
    uri: &Url,
) -> Option<CodeAction> {
    let ref_name = referenced_name_from_data(diag)?;
    let decl_line = declaration_line(analysis, &ref_name)?;
    make_add_visibility_action(
        diag,
        current_source,
        uri,
        decl_line,
        "pub ",
        format!("Add `pub` to `{ref_name}`"),
    )
}

/// Build a quick-fix code action that inserts `prefix` (e.g. `"pub "` or
/// `"pub(bind) "`) before the declaration keyword on `decl_line`.
fn make_add_visibility_action(
    diag: &Diagnostic,
    source: &str,
    uri: &Url,
    decl_line: u32,
    prefix: &str,
    title: String,
) -> Option<CodeAction> {
    let insert_pos = find_keyword_position(source, decl_line)?;
    let edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: prefix.to_string(),
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(true),
        ..Default::default()
    })
}

/// For V002: insert `pub(bind) ` before the declaration keyword on the line
/// containing the diagnostic. V002 fires on required `index` / `type` / `dim`
/// declarations — the bindable interface of the library — so the fix always
/// lifts them all the way to `pub(bind)`.
fn make_add_pub_bind_action_v002(diag: &Diagnostic, source: &str, uri: &Url) -> Option<CodeAction> {
    make_add_visibility_action(
        diag,
        source,
        uri,
        diag.range.start.line,
        "pub(bind) ",
        "Add `pub(bind)` to this declaration".to_string(),
    )
}

/// Find the position of the declaration keyword on a given line.
///
/// Skips leading whitespace and returns the position at which a keyword
/// (`param`, `node`, `const`, `index`, `dim`, `unit`, `type`, `base`, `dag`,
/// `plot`, `assert`, `import`, `include`) starts.
fn find_keyword_position(source: &str, line: u32) -> Option<Position> {
    let line_start = line_start_offset(source, line)?;
    let line_str = source.get(line_start..)?.lines().next().unwrap_or("");

    let trimmed = line_str.trim_start();
    let indent = line_str.len() - trimmed.len();
    let _keyword = ALL_DECL_KEYWORDS
        .iter()
        .find(|keyword| keyword_matches(trimmed, keyword))?;

    Some(LineIndex::new(source).position(line_start + indent))
}

fn keyword_matches(trimmed: &str, keyword: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix(keyword) else {
        return false;
    };
    rest.chars()
        .next()
        .is_none_or(|ch| ch.is_whitespace() || ch == '(')
}

fn line_start_offset(source: &str, line: u32) -> Option<usize> {
    let mut offset = 0usize;
    for (idx, segment) in source.split_inclusive('\n').enumerate() {
        if idx == line as usize {
            return Some(offset);
        }
        offset += segment.len();
    }
    (line as usize == source.lines().count()).then_some(offset)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;

    use tower_lsp::lsp_types::{
        CodeActionContext, PartialResultParams, TextDocumentIdentifier, WorkDoneProgressParams,
    };

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
        let symbol_table = symbol_table::build_for_buffer(&ast, source);
        AnalysisResult {
            source: Arc::new(source.to_string()),
            symbol_table,
            imported_definitions: StdHashMap::new(),
            diagnostics: Arc::new(StdHashMap::new()),
            eval_values: StdHashMap::new(),
            fn_signatures: build_fn_signatures(),
            import_links: Vec::new(),
            buffer_parsed: true,
        }
    }

    #[test]
    fn find_keyword_position_uses_utf16_columns() {
        let source = "\u{3000}param x: Dimensionless = 1.0;";
        let pos = find_keyword_position(source, 0).unwrap();
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 1);
    }

    #[test]
    fn find_keyword_position_rejects_prefix_only_keyword_match() {
        assert!(find_keyword_position("nodeish x", 0).is_none());
    }

    fn make_diag(
        code: &str,
        message: &str,
        range: Range,
        data: Option<serde_json::Value>,
    ) -> Diagnostic {
        Diagnostic {
            range,
            severity: Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(code.to_string())),
            source: Some("graphcal".to_string()),
            message: message.to_string(),
            data,
            ..Default::default()
        }
    }

    fn make_params(uri: &Url, diagnostics: Vec<Diagnostic>) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    #[test]
    fn v002_adds_pub_bind_to_required_index() {
        let analysis = analysis_from_source("index Phase;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required index `Phase` must be declared `pub(bind)`",
            Range {
                start: Position {
                    line: 0,
                    character: 6,
                },
                end: Position {
                    line: 0,
                    character: 11,
                },
            },
            None,
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(action.title, "Add `pub(bind)` to this declaration");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.is_preferred, Some(true));

        let edit = action.edit.as_ref().unwrap();
        let changes = edit.changes.as_ref().unwrap();
        let edits = &changes[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "pub(bind) ");
        assert_eq!(
            edits[0].range.start,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn v002_adds_pub_bind_with_indentation() {
        let analysis = analysis_from_source("    index Phase;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required index `Phase` must be declared `pub(bind)`",
            Range {
                start: Position {
                    line: 0,
                    character: 10,
                },
                end: Position {
                    line: 0,
                    character: 15,
                },
            },
            None,
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits[0].new_text, "pub(bind) ");
        assert_eq!(
            edits[0].range.start,
            Position {
                line: 0,
                character: 4
            }
        );
    }

    #[test]
    fn v003_adds_pub_to_private_dim() {
        let analysis = analysis_from_source(
            "dim Velocity = Length / Time;\npub node speed: Velocity = 10.0 m/s;",
        );
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V003",
            "`pub node` `speed` references private dim `Velocity` in its signature",
            Range {
                start: Position {
                    line: 1,
                    character: 16,
                },
                end: Position {
                    line: 1,
                    character: 24,
                },
            },
            Some(serde_json::json!({ "referencedName": "Velocity" })),
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(action.title, "Add `pub` to `Velocity`");
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits[0].new_text, "pub ");
        // Should insert at line 0, character 0 (before `dim`)
        assert_eq!(
            edits[0].range.start,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn v003_declaration_in_comment_is_not_matched() {
        // Regression: the declaration line used to be found by grepping
        // source lines, which also matched declaration-shaped text inside
        // comments. The symbol table only knows real declarations.
        let analysis = analysis_from_source(
            "// dim Velocity = old commented-out version\n\
             dim Velocity = Length / Time;\n\
             pub node speed: Velocity = 10.0 m/s;",
        );
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V003",
            "`pub node` `speed` references private dim `Velocity` in its signature",
            Range::default(),
            Some(serde_json::json!({ "referencedName": "Velocity" })),
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        // The real declaration is on line 1, not the comment on line 0.
        assert_eq!(edits[0].range.start.line, 1);
    }

    #[test]
    fn v006_adds_pub_to_leaked_symbol() {
        let analysis = analysis_from_source("type Inner { Inner }\nnode x: Dimensionless = 1.0;\n");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V006",
            "re-exported type `Widget`'s signature references private type `Inner`",
            Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 12,
                },
            },
            Some(serde_json::json!({ "referencedName": "Inner" })),
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(action.title, "Add `pub` to `Inner`");
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits[0].new_text, "pub ");
        // `type Inner` is on line 0 — the edit goes at the start of that line.
        assert_eq!(
            edits[0].range.start,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn v003_without_data_produces_no_action() {
        // Diagnostics from older publishes (or other producers) without the
        // structured payload yield no quick fix — never a wrong one.
        let analysis = analysis_from_source("dim Velocity = Length / Time;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V003",
            "`pub node` `speed` references private dim `Velocity` in its signature",
            Range::default(),
            None,
        );
        let params = make_params(&uri, vec![diag]);
        assert!(code_actions(&params, &analysis, &analysis.source).is_none());
    }

    #[test]
    fn no_actions_for_unrelated_diagnostic() {
        let analysis = analysis_from_source("node x: Dimensionless = @nonexistent;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::N002",
            "unknown reference `nonexistent`",
            Range::default(),
            None,
        );
        let params = make_params(&uri, vec![diag]);
        assert!(code_actions(&params, &analysis, &analysis.source).is_none());
    }
}

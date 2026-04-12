//! textDocument/codeAction handler.
//!
//! Provides quick-fix code actions for visibility-related diagnostics:
//! - V002: Add `pub` to required param/index
//! - V003: Add `pub` to private item referenced by a public declaration

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Diagnostic, NumberOrString, Position, Range, TextEdit, Url, WorkspaceEdit,
};

/// All declaration keywords that can be preceded by `pub`.
///
/// Used by `find_keyword_position` to locate the insertion point for `pub `.
/// `"base"` matches `"base dim ..."` via `starts_with`.
const ALL_DECL_KEYWORDS: &[&str] = &[
    "param", "node", "const", "index", "dim", "unit", "type", "base", "dag", "plot", "assert",
    "import", "include",
];

/// Declaration keywords for name-based lookup.
///
/// Each entry includes a trailing space to match `keyword name...` patterns.
/// Only includes keywords whose declarations can trigger V003 (items that
/// appear in type annotations of `pub` declarations).
const NAMED_DECL_KEYWORDS: &[&str] = &[
    "dim ",
    "type ",
    "index ",
    "base dim ",
    "unit ",
    "param ",
    "node ",
    "const ",
];

/// Produce code actions for the given diagnostics.
pub fn code_actions(params: &CodeActionParams, source: &str) -> Option<CodeActionResponse> {
    let mut actions = Vec::new();

    for diag in &params.context.diagnostics {
        let code = match &diag.code {
            Some(NumberOrString::String(s)) => s.as_str(),
            _ => continue,
        };

        match code {
            "graphcal::V002" => {
                if let Some(action) =
                    make_add_pub_action_v002(diag, source, &params.text_document.uri)
                {
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
            "graphcal::V003" => {
                if let Some(action) =
                    make_add_pub_action_v003(diag, source, &params.text_document.uri)
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

/// For V002: insert `pub ` before the declaration keyword on the line containing the diagnostic.
///
/// The diagnostic span points to the name (e.g., `x` in `param x: Dimensionless;`).
/// We find the start of the line and insert `pub ` right after leading whitespace.
fn make_add_pub_action_v002(diag: &Diagnostic, source: &str, uri: &Url) -> Option<CodeAction> {
    let insert_pos = find_keyword_position(source, diag.range.start.line)?;

    let edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: "pub ".to_string(),
    };

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    Some(CodeAction {
        title: "Add `pub` to this declaration".to_string(),
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

/// For V003: insert `pub ` before the private item's declaration.
///
/// The diagnostic message follows the pattern:
/// "`pub {kind}` `{pub_name}` references private `ref_kind` `{ref_name}` in its type annotation"
///
/// We extract `ref_name` from the message and search the source for its declaration,
/// then insert `pub ` before the keyword.
fn make_add_pub_action_v003(diag: &Diagnostic, source: &str, uri: &Url) -> Option<CodeAction> {
    // Extract ref_name from the message: "... references private {kind} `{ref_name}` ..."
    let ref_name = extract_private_ref_name(&diag.message)?;

    // Find the declaration of ref_name in the source.
    let decl_line = find_declaration_line(source, &ref_name)?;
    let insert_pos = find_keyword_position(source, decl_line)?;

    let edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: "pub ".to_string(),
    };

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    Some(CodeAction {
        title: format!("Add `pub` to `{ref_name}`"),
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

/// Find the position of the declaration keyword on a given line.
///
/// Skips leading whitespace and returns the position at which a keyword
/// (`param`, `node`, `const`, `index`, `dim`, `unit`, `type`, `base`, `dag`,
/// `plot`, `assert`, `import`, `include`) starts.
#[expect(
    clippy::cast_possible_truncation,
    reason = "line numbers and character offsets fit in u32 for typical source files"
)]
fn find_keyword_position(source: &str, line: u32) -> Option<Position> {
    let line_str = source.lines().nth(line as usize)?;

    // Find the start of the keyword (skip leading whitespace).
    let trimmed = line_str.trim_start();
    let indent = line_str.len() - trimmed.len();

    // Verify this looks like a declaration keyword.
    let starts_with_keyword = ALL_DECL_KEYWORDS.iter().any(|kw| trimmed.starts_with(kw));

    if !starts_with_keyword {
        return None;
    }

    Some(Position {
        line,
        character: indent as u32,
    })
}

/// Extract the private item name from a V003 diagnostic message.
///
/// Message format: "`pub {kind}` `{pub_name}` references private `ref_kind` `{ref_name}` in ..."
fn extract_private_ref_name(message: &str) -> Option<String> {
    // Find "references private ... `{ref_name}`"
    let marker = "references private ";
    let after_marker = message.find(marker).map(|i| &message[i + marker.len()..])?;
    // Skip the ref_kind word(s) to reach `ref_name`
    let backtick_start = after_marker.find('`')? + 1;
    let rest = &after_marker[backtick_start..];
    let backtick_end = rest.find('`')?;
    Some(rest[..backtick_end].to_string())
}

/// Find the 0-based line number where an item with the given name is declared.
///
/// Searches for lines containing a declaration keyword followed by the name.
#[expect(
    clippy::cast_possible_truncation,
    reason = "line index fits in u32 for typical source files"
)]
fn find_declaration_line(source: &str, name: &str) -> Option<u32> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        for kw in NAMED_DECL_KEYWORDS {
            if let Some(rest) = trimmed.strip_prefix(kw) {
                // Check if the name matches (followed by space, '=', ':', ';', or end of line).
                if rest.starts_with(name)
                    && rest[name.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| matches!(c, ' ' | '=' | ':' | ';' | '\t'))
                {
                    return Some(i as u32);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        reason = "test code"
    )]

    use tower_lsp::lsp_types::{
        CodeActionContext, PartialResultParams, TextDocumentIdentifier, WorkDoneProgressParams,
    };

    use super::*;

    fn make_diag(code: &str, message: &str, range: Range) -> Diagnostic {
        Diagnostic {
            range,
            severity: Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(code.to_string())),
            source: Some("graphcal".to_string()),
            message: message.to_string(),
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
    fn v002_adds_pub_to_required_param() {
        let source = "param x: Dimensionless;";
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required param `x` must be declared `pub`",
            Range {
                start: Position {
                    line: 0,
                    character: 6,
                },
                end: Position {
                    line: 0,
                    character: 7,
                },
            },
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, source).unwrap();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(action.title, "Add `pub` to this declaration");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.is_preferred, Some(true));

        let edit = action.edit.as_ref().unwrap();
        let changes = edit.changes.as_ref().unwrap();
        let edits = &changes[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "pub ");
        assert_eq!(
            edits[0].range.start,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn v002_adds_pub_with_indentation() {
        let source = "    param x: Dimensionless;";
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required param `x` must be declared `pub`",
            Range {
                start: Position {
                    line: 0,
                    character: 10,
                },
                end: Position {
                    line: 0,
                    character: 11,
                },
            },
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, source).unwrap();
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
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
        let source = "dim Velocity = Length / Time;\npub param speed: Velocity = 10.0 m/s;";
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V003",
            "`pub param` `speed` references private dim `Velocity` in its type annotation\n\nhint: add `pub` to `Velocity` or remove `pub` from `speed`",
            Range {
                start: Position {
                    line: 1,
                    character: 17,
                },
                end: Position {
                    line: 1,
                    character: 25,
                },
            },
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, source).unwrap();
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
    fn extract_private_ref_name_works() {
        let msg = "`pub param` `speed` references private dim `Velocity` in its type annotation";
        assert_eq!(extract_private_ref_name(msg), Some("Velocity".to_string()));
    }

    #[test]
    fn no_actions_for_unrelated_diagnostic() {
        let source = "node x: Dimensionless = @nonexistent;";
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::N002",
            "unknown reference `nonexistent`",
            Range::default(),
        );
        let params = make_params(&uri, vec![diag]);
        assert!(code_actions(&params, source).is_none());
    }
}

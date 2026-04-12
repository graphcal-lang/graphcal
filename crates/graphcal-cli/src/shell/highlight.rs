//! Syntax highlighting for the interactive shell.
//!
//! Implements rustyline's `Highlighter` trait using the Graphcal lexer to
//! colorize input as the user types.

use std::borrow::Cow;

use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::{Completer, Helper, Hinter, Validator};

use graphcal_compiler::syntax::lexer::Lexer;
use graphcal_compiler::syntax::token::Token;

// ANSI color codes.
const BOLD_BLUE: &str = "\x1b[1;34m";
const BOLD_CYAN: &str = "\x1b[1;36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BOLD_MAGENTA: &str = "\x1b[1;35m";
const GRAY: &str = "\x1b[90m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// Rustyline helper that provides syntax highlighting for Graphcal input.
#[derive(Helper, Completer, Hinter, Validator)]
pub struct ShellHelper;

impl Highlighter for ShellHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Owned(highlight_line(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        // Always re-highlight after each keystroke.
        true
    }
}

/// Highlight a single line of input with ANSI color codes.
fn highlight_line(line: &str) -> String {
    let trimmed = line.trim_start();

    // Commands (lines starting with `:`) get a single color.
    if trimmed.starts_with(':') {
        return format!("{CYAN}{line}{RESET}");
    }

    // Find comment start (`//`) — everything after it is gray.
    // We need to be careful not to match `//` inside string literals.
    let comment_start = find_comment_start(line);

    let (code_part, comment_part) =
        comment_start.map_or((line, None), |pos| (&line[..pos], Some(&line[pos..])));

    let mut result = highlight_code(code_part);

    if let Some(comment) = comment_part {
        result.push_str(GRAY);
        result.push_str(comment);
        result.push_str(RESET);
    }

    result
}

/// Find the byte offset of a line comment (`//`), skipping those inside strings.
///
/// Handles backslash-escaped quotes (`\"`) inside strings so that a string
/// like `"foo\"bar // baz"` does not incorrectly detect `//` as a comment.
fn find_comment_start(line: &str) -> Option<usize> {
    let mut in_string = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if in_string && bytes[i] == b'\\' {
            // Skip the escaped character.
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            in_string = !in_string;
        } else if !in_string && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            return Some(i);
        } else {
            // Not a comment start — continue scanning.
        }
        i += 1;
    }
    None
}

/// Highlight the code portion (non-comment) of a line using the lexer.
fn highlight_code(code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(code.len() + 64);
    let mut lexer = Lexer::new(code);
    let mut last_end = 0;

    while let Some((token, span)) = lexer.next_token() {
        let start = span.offset();
        let end = start + span.len();

        // Append any gap (whitespace) between the last token and this one.
        if start > last_end {
            result.push_str(&code[last_end..start]);
        }

        let slice = &code[start..end];

        if let Some(color) = token_color(&token) {
            result.push_str(color);
            result.push_str(slice);
            result.push_str(RESET);
        } else {
            result.push_str(slice);
        }

        last_end = end;
    }

    // Append any trailing text after the last token (e.g., trailing whitespace
    // or text the lexer couldn't recognize).
    if last_end < code.len() {
        result.push_str(&code[last_end..]);
    }

    result
}

/// Map a token to its ANSI color code, or `None` for default color.
const fn token_color(token: &Token) -> Option<&'static str> {
    match token {
        // Keywords
        Token::Param
        | Token::Node
        | Token::Const
        | Token::If
        | Token::Else
        | Token::Dimension
        | Token::Unit
        | Token::Type
        | Token::Fn
        | Token::For
        | Token::Import
        | Token::Match
        | Token::As
        | Token::Assert
        | Token::Table
        | Token::Index
        | Token::Linspace
        | Token::Pub => Some(BOLD_BLUE),

        // Booleans
        Token::True | Token::False => Some(BOLD_CYAN),

        // Numbers
        Token::Number => Some(GREEN),

        // String literals
        Token::StringLiteral => Some(YELLOW),

        // Graph reference
        Token::At => Some(BOLD_MAGENTA),

        // Everything else: default
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn highlight_command() {
        let result = highlight_line(":help");
        assert!(result.starts_with(CYAN));
        assert!(result.ends_with(RESET));
        assert!(result.contains(":help"));
    }

    #[test]
    fn highlight_preserves_plain_text() {
        // An identifier with no special tokens should pass through.
        let result = highlight_line("x");
        assert_eq!(result, "x");
    }

    #[test]
    fn highlight_keyword() {
        let result = highlight_line("param x = 5.0;");
        assert!(result.contains(&format!("{BOLD_BLUE}param{RESET}")));
        assert!(result.contains(&format!("{GREEN}5.0{RESET}")));
    }

    #[test]
    fn highlight_graph_ref() {
        let result = highlight_line("node y = @x * 2.0;");
        assert!(result.contains(&format!("{BOLD_MAGENTA}@{RESET}")));
    }

    #[test]
    fn highlight_comment() {
        let result = highlight_line("param x = 5.0; // a comment");
        assert!(result.contains(&format!("{GRAY}// a comment{RESET}")));
    }

    #[test]
    fn highlight_comment_in_string_not_detected() {
        let result = highlight_line(r#"import "file://path";"#);
        // The `//` inside the string should NOT be treated as a comment.
        assert!(!result.contains(GRAY));
    }

    #[test]
    fn highlight_boolean() {
        let result = highlight_line("true");
        assert!(result.contains(&format!("{BOLD_CYAN}true{RESET}")));
    }

    #[test]
    fn highlight_string_literal() {
        let result = highlight_line(r#"import "./helper.gcl";"#);
        assert!(result.contains(&format!("{YELLOW}\"./helper.gcl\"{RESET}")));
    }
}

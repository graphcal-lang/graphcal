//! textDocument/rename and textDocument/prepareRename handlers.

use std::collections::HashMap;

use tower_lsp::lsp_types::{PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::convert::span_to_range;
use crate::server::AnalysisResult;
use crate::symbol_table::{SymbolCategory, SymbolKey};

/// Internal target info for the symbol being renamed.
struct RenameTarget {
    /// The symbol table key for this target.
    key: SymbolKey,
    /// Whether the definition is local (in this file) vs imported.
    is_local: bool,
}

/// Check whether a name is a valid Graphcal identifier.
fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Find the rename target at the given byte offset.
fn find_rename_target(analysis: &AnalysisResult, offset: usize) -> Option<RenameTarget> {
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        let key = reference.target.clone();
        // Reject builtins and fields.
        if let Some(def) = analysis.symbol_table.definitions.get(&key) {
            if matches!(
                def.category,
                SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst | SymbolCategory::Field
            ) {
                return None;
            }
        } else if analysis.imported_definitions.contains_key(&key) {
            return Some(RenameTarget {
                key,
                is_local: false,
            });
        } else {
            // Reference to something we can't find — might be a field reference.
            if key.is_field() {
                return None;
            }
        }
        let is_local = analysis.symbol_table.definitions.contains_key(&key);
        Some(RenameTarget { key, is_local })
    } else if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        if matches!(
            definition.category,
            SymbolCategory::BuiltinFn | SymbolCategory::BuiltinConst | SymbolCategory::Field
        ) {
            return None;
        }
        // Use pointer-based reverse lookup to get the correct scoped key.
        let actual_key = analysis
            .symbol_table
            .definitions
            .iter()
            .find(|(_, d)| std::ptr::eq(*d, definition))
            .map_or_else(
                || SymbolKey::TopLevel(definition.name.clone()),
                |(k, _)| k.clone(),
            );
        Some(RenameTarget {
            key: actual_key,
            is_local: true,
        })
    } else {
        None
    }
}

/// Validate a rename and return the current name's range and placeholder.
pub fn prepare_rename(analysis: &AnalysisResult, offset: usize) -> Option<PrepareRenameResponse> {
    let target = find_rename_target(analysis, offset)?;

    // Determine the span to highlight.
    let span = if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        reference.span
    } else if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        definition.name_span
    } else {
        return None;
    };

    // Get the current name text for the placeholder.
    let key_str = target.key.to_string();
    let placeholder = analysis
        .source
        .get(span.offset()..span.offset() + span.len())
        .unwrap_or(&key_str)
        .to_string();

    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: span_to_range(&analysis.source, span),
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

    let target = find_rename_target(analysis, offset)?;

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    // Collect all reference edits in the current file.
    let mut current_file_edits = Vec::new();
    for r in analysis.symbol_table.find_all_references(&target.key) {
        current_file_edits.push(TextEdit {
            range: span_to_range(&analysis.source, r.span),
            new_text: new_name.to_string(),
        });
    }

    // Include the definition's name span if it's local.
    if target.is_local
        && let Some(def) = analysis.symbol_table.definitions.get(&target.key)
        && !def.name_span.is_empty()
    {
        current_file_edits.push(TextEdit {
            range: span_to_range(&analysis.source, def.name_span),
            new_text: new_name.to_string(),
        });
    }

    if !current_file_edits.is_empty() {
        changes.insert(uri.clone(), current_file_edits);
    }

    // For imported symbols, also rename the definition in the source file.
    if !target.is_local
        && let Some(imported) = analysis.imported_definitions.get(&target.key)
        && !imported.definition.name_span.is_empty()
    {
        changes
            .entry(imported.uri.clone())
            .or_default()
            .push(TextEdit {
                range: span_to_range(&imported.source, imported.definition.name_span),
                new_text: new_name.to_string(),
            });
    }

    if changes.is_empty() {
        return None;
    }

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
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

    use super::*;
    use crate::symbol_table;

    /// Build a minimal `AnalysisResult` from source text.
    fn analysis_from_source(source: &str) -> AnalysisResult {
        let ast = graphcal_compiler::syntax::parser::Parser::with_name(source, "test.gcl")
            .parse_file()
            .unwrap();
        let symbol_table = symbol_table::build_from_ast(&ast);
        AnalysisResult {
            source: source.to_string(),
            symbol_table,
            imported_definitions: HashMap::new(),
            diagnostics: Vec::new(),
            eval_values: HashMap::new(),
            fn_signatures: HashMap::new(),
            import_decls: Vec::new(),
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
    fn is_valid_identifier_cases() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("velocity"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("my_var_2"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("123"));
        assert!(!is_valid_identifier("has space"));
        assert!(!is_valid_identifier("a-b"));
    }
}

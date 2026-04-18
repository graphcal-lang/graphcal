//! Shared symbol resolution for LSP features (hover, go-to-definition, etc.).

use graphcal_compiler::syntax::span::Span;

use crate::server::{AnalysisResult, ImportedDefinition};
use crate::symbol_table::{DefinitionInfo, SymbolKey};

/// Where a resolved symbol lives.
pub enum SymbolLocation<'a> {
    /// Symbol defined in the current file.
    Local(&'a DefinitionInfo),
    /// Symbol defined in an imported file.
    Imported(&'a ImportedDefinition),
}

/// A resolved symbol at a cursor position, with the key to look it up in the symbol table.
///
pub struct ResolvedSymbol<'a> {
    /// The symbol table key for this symbol.
    pub key: SymbolKey,
    /// Where the definition lives.
    pub location: SymbolLocation<'a>,
    /// Whether the cursor was on a reference (true) or a definition (false).
    pub is_reference: bool,
    /// The span of the token under the cursor.
    pub cursor_span: Span,
}

/// Resolve the symbol at the given byte offset.
///
/// Checks references first (cursor on a usage), then definitions (cursor on the name
/// in a declaration). Returns `None` if no symbol is found at the offset.
pub fn resolve_symbol_at(analysis: &AnalysisResult, offset: usize) -> Option<ResolvedSymbol<'_>> {
    // First check references.
    if let Some(reference) = analysis.symbol_table.find_reference_at(offset) {
        let key = reference.target.clone();
        let span = reference.span;
        if let Some(def) = analysis.symbol_table.definitions.get(&key) {
            return Some(ResolvedSymbol {
                key,
                location: SymbolLocation::Local(def),
                is_reference: true,
                cursor_span: span,
            });
        }
        if let Some(imported) = analysis.imported_definitions.get(&key) {
            return Some(ResolvedSymbol {
                key,
                location: SymbolLocation::Imported(imported),
                is_reference: true,
                cursor_span: span,
            });
        }
    }
    // Then check definitions.
    if let Some(definition) = analysis.symbol_table.find_definition_at(offset) {
        let span = definition.name_span;
        // Find the actual key in the definitions map (may differ from `definition.name`
        // for scoped symbols like `fn_name::param`).
        let key = analysis.symbol_table.find_definition_key(definition);
        return Some(ResolvedSymbol {
            key,
            location: SymbolLocation::Local(definition),
            is_reference: false,
            cursor_span: span,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test code"
    )]

    use std::sync::Arc;

    use tower_lsp::lsp_types::Url;

    use super::*;
    use crate::server::build_fn_signatures;
    use crate::symbol_table::{self, SymbolCategory};

    /// Build an `AnalysisResult` with imported definitions for two modules that
    /// both export a symbol named `x`.
    fn analysis_with_two_module_imports() -> AnalysisResult {
        // Source in "main.gcl":
        //   import "./mod_a.gcl";
        //   import "./mod_b.gcl";
        //   node a: Dimensionless = @mod_a::x;
        //   node b: Dimensionless = @mod_b::x;
        let source = concat!(
            "import \"./mod_a.gcl\";\n",            // 0..22
            "import \"./mod_b.gcl\";\n",            // 22..44
            "node a: Dimensionless = @mod_a::x;\n", // 44..79
            "node b: Dimensionless = @mod_b::x;\n", // 79..114
        );
        let ast = graphcal_compiler::syntax::parser::Parser::with_name(source, "main.gcl")
            .parse_file()
            .unwrap();
        let symbol_table = symbol_table::build_from_ast(&ast);

        // Simulate imported definitions from two different modules.
        let mut imported_definitions = std::collections::HashMap::new();
        imported_definitions.insert(
            SymbolKey::Qualified {
                module: "mod_a".to_string(),
                name: "x".to_string(),
            },
            ImportedDefinition {
                uri: Url::parse("file:///mod_a.gcl").unwrap(),
                source: Arc::new("param x: Dimensionless = 1.0;".to_string()),
                definition: DefinitionInfo {
                    name: "x".to_string(),
                    category: SymbolCategory::Param,
                    name_span: graphcal_compiler::syntax::span::Span::new(6, 1),
                    decl_span: graphcal_compiler::syntax::span::Span::new(0, 29),
                    type_description: Some("Dimensionless".to_string()),
                    detail: None,
                    visibility: None,
                },
            },
        );
        imported_definitions.insert(
            SymbolKey::Qualified {
                module: "mod_b".to_string(),
                name: "x".to_string(),
            },
            ImportedDefinition {
                uri: Url::parse("file:///mod_b.gcl").unwrap(),
                source: Arc::new("param x: Dimensionless = 2.0;".to_string()),
                definition: DefinitionInfo {
                    name: "x".to_string(),
                    category: SymbolCategory::Param,
                    name_span: graphcal_compiler::syntax::span::Span::new(6, 1),
                    decl_span: graphcal_compiler::syntax::span::Span::new(0, 29),
                    type_description: Some("Dimensionless".to_string()),
                    detail: None,
                    visibility: None,
                },
            },
        );

        AnalysisResult {
            source: source.to_string(),
            symbol_table,
            imported_definitions,
            diagnostics: Vec::new(),
            eval_values: std::collections::HashMap::new(),
            fn_signatures: build_fn_signatures(),
            import_links: Vec::new(),
        }
    }

    #[test]
    fn qualified_refs_resolve_to_correct_module() {
        let analysis = analysis_with_two_module_imports();

        // Find offset of "x" in "@mod_a::x" — search for the pattern in source.
        let mod_a_x_offset = analysis.source.find("@mod_a::x").unwrap() + "@mod_a::".len();
        let resolved_a =
            resolve_symbol_at(&analysis, mod_a_x_offset).expect("should resolve @mod_a::x");
        assert!(resolved_a.is_reference);
        assert_eq!(
            resolved_a.key,
            SymbolKey::Qualified {
                module: "mod_a".to_string(),
                name: "x".to_string(),
            }
        );
        match &resolved_a.location {
            SymbolLocation::Imported(imp) => {
                assert_eq!(imp.uri.as_str(), "file:///mod_a.gcl");
            }
            SymbolLocation::Local(_) => panic!("expected imported, got local"),
        }

        // Find offset of "x" in "@mod_b::x"
        let mod_b_x_offset = analysis.source.find("@mod_b::x").unwrap() + "@mod_b::".len();
        let resolved_b =
            resolve_symbol_at(&analysis, mod_b_x_offset).expect("should resolve @mod_b::x");
        assert!(resolved_b.is_reference);
        assert_eq!(
            resolved_b.key,
            SymbolKey::Qualified {
                module: "mod_b".to_string(),
                name: "x".to_string(),
            }
        );
        match &resolved_b.location {
            SymbolLocation::Imported(imp) => {
                assert_eq!(imp.uri.as_str(), "file:///mod_b.gcl");
            }
            SymbolLocation::Local(_) => panic!("expected imported, got local"),
        }
    }

    #[test]
    fn qualified_refs_are_distinct_keys() {
        let analysis = analysis_with_two_module_imports();

        // The symbol table should have distinct qualified references.
        let key_a = SymbolKey::Qualified {
            module: "mod_a".to_string(),
            name: "x".to_string(),
        };
        let key_b = SymbolKey::Qualified {
            module: "mod_b".to_string(),
            name: "x".to_string(),
        };
        assert_ne!(
            key_a, key_b,
            "qualified keys with different modules must be distinct"
        );

        let refs_a = analysis.symbol_table.find_all_references(&key_a);
        let refs_b = analysis.symbol_table.find_all_references(&key_b);
        assert_eq!(refs_a.len(), 1, "exactly one reference to mod_a::x");
        assert_eq!(refs_b.len(), 1, "exactly one reference to mod_b::x");
    }
}

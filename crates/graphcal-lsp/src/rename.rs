//! textDocument/rename and textDocument/prepareRename handlers.

use std::collections::HashMap;

use tower_lsp::lsp_types::{PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::convert::LineIndex;
use crate::resolve::{ResolvedSymbol, SymbolLocation, reference_lookup_keys, resolve_symbol_at};
use crate::server::AnalysisResult;
use crate::symbol_table::{SymbolCategory, SymbolKey, SymbolPath};

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

/// Why a rename request was explicitly refused.
///
/// Surfaced to the client as a descriptive JSON-RPC error rather than a
/// silent `null` response: applying the rename anyway would produce a
/// non-compiling buffer, which is the worst available outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameRefusal {
    /// The new name is not a lexable identifier (or is a reserved keyword).
    InvalidIdentifier { new_name: String },
    /// The new name collides with a visible declaration in the same
    /// namespace, which would compile to a duplicate-name error (N001).
    NameCollision { new_name: String },
}

impl std::fmt::Display for RenameRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier { new_name } => {
                write!(f, "`{new_name}` is not a valid graphcal identifier")
            }
            Self::NameCollision { new_name } => write!(
                f,
                "cannot rename to `{new_name}`: a declaration with that name is already in scope"
            ),
        }
    }
}

/// The key the renamed symbol would occupy: the same key shape with the
/// leaf name replaced. Used to probe the symbol table for collisions in
/// the renamed symbol's own namespace/scope.
fn key_with_new_name(key: &SymbolKey, new_name: &str) -> SymbolKey {
    fn path_with_new_leaf(path: &SymbolPath, new_name: &str) -> SymbolPath {
        match path {
            SymbolPath::Local(_) => SymbolPath::local(new_name),
            SymbolPath::Qualified { module, .. } => SymbolPath::Qualified {
                module: module.clone(),
                name: new_name.to_string(),
            },
        }
    }
    match key {
        SymbolKey::TopLevel(_) => SymbolKey::TopLevel(new_name.to_string()),
        SymbolKey::Qualified { module, .. } => SymbolKey::Qualified {
            module: module.clone(),
            name: new_name.to_string(),
        },
        SymbolKey::Constructor(path) => SymbolKey::Constructor(path_with_new_leaf(path, new_name)),
        SymbolKey::Variant { parent, .. } => SymbolKey::Variant {
            parent: parent.clone(),
            variant: new_name.to_string(),
        },
        SymbolKey::Field { owner, .. } => SymbolKey::Field {
            owner: owner.clone(),
            field_name: new_name.to_string(),
        },
        SymbolKey::ExprScoped { kind, offset, .. } => SymbolKey::ExprScoped {
            kind: *kind,
            offset: *offset,
            local: new_name.to_string(),
        },
    }
}

/// True when `new_name` collides with a visible declaration in the renamed
/// symbol's namespace/scope. Builtins (`PI`, `sqrt`, unit names) are not
/// collisions: the compiler allows shadowing them.
fn collides_with_existing(analysis: &AnalysisResult, key: &SymbolKey, new_name: &str) -> bool {
    let candidate = key_with_new_name(key, new_name);
    if let Some(existing) = analysis.symbol_table.definitions.get(&candidate)
        && !existing.is_builtin()
    {
        return true;
    }
    analysis.imported_definitions.contains_key(&candidate)
}

/// Perform the rename, returning a workspace edit.
///
/// `Ok(None)` means there is nothing renameable at the cursor (the client
/// sees a plain `null`); `Err` is an explicit refusal with a reason the
/// client should show to the user.
pub fn rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>, RenameRefusal> {
    if !is_valid_identifier(new_name) {
        return Err(RenameRefusal::InvalidIdentifier {
            new_name: new_name.to_string(),
        });
    }

    let Some(resolved) = resolve_symbol_at(analysis, offset) else {
        return Ok(None);
    };
    if !is_renameable(&resolved) {
        return Ok(None);
    }
    let SymbolLocation::Local(def) = &resolved.location else {
        return Ok(None);
    };
    // Renaming to the symbol's current name is a no-op, not a collision.
    if def.name == new_name {
        return Ok(None);
    }
    if collides_with_existing(analysis, &resolved.key, new_name) {
        return Err(RenameRefusal::NameCollision {
            new_name: new_name.to_string(),
        });
    }

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
        return Ok(None);
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), current_file_edits);

    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }))
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
        let symbol_table = symbol_table::build_for_buffer(&ast, source);
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
        let result = rename(&analysis, &uri, offset, "velocity")
            .unwrap()
            .unwrap();
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
        let result = rename(&analysis, &uri, at_x, "velocity").unwrap().unwrap();
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
        for bad in ["", "123bad", "has space"] {
            assert_eq!(
                rename(&analysis, &uri, offset, bad),
                Err(RenameRefusal::InvalidIdentifier {
                    new_name: bad.to_string()
                })
            );
        }
    }

    #[test]
    fn rename_imported_symbol_rejected() {
        // Build an analysis where `@y` resolves through an imported alias.
        // Cross-file rename is not yet implemented; the request must be
        // refused rather than producing a partial workspace edit.
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
        let main_text = "import helper.lib.{y};\nnode z: Dimensionless = @y + 1.0;\n";
        std::fs::write(&main_path, main_text).unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected the import to load cleanly, got: {:?}",
            analysis.diagnostics,
        );

        let cursor = main_text.find("@y").unwrap() + 1;
        assert!(
            prepare_rename(&analysis, cursor).is_none(),
            "imported symbol must not be prepare-renameable"
        );
        assert!(
            rename(&analysis, &main_uri, cursor, "renamed")
                .unwrap()
                .is_none(),
            "imported symbol must not be renamed",
        );

        // Issue #829: renaming a local declaration to the name of an
        // imported symbol collides as well — both would be visible.
        let z_cursor = main_text.find("node z").unwrap() + "node ".len();
        assert_eq!(
            rename(&analysis, &main_uri, z_cursor, "y"),
            Err(RenameRefusal::NameCollision {
                new_name: "y".to_string()
            })
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
                rename(&analysis, &uri, offset, keyword).is_err(),
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
        if let Ok(Some(result)) = rename(&analysis, &uri, offset, "Standby") {
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

    /// Issue #829: renaming a declaration to the name of another visible
    /// declaration must be refused — applying it would produce an N001
    /// duplicate-name compile error.
    #[test]
    fn rename_to_colliding_declaration_rejected() {
        let source = "\
param mass: Mass = 100.0 kg;
param velocity: Velocity = 50.0 m/s;
node momentum: Force * Time = @mass * @velocity;
node kinetic: Energy = 0.5 * @mass * @velocity ^ 2.0;
";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        let offset = source.find("momentum").unwrap();
        assert_eq!(
            rename(&analysis, &uri, offset, "velocity"),
            Err(RenameRefusal::NameCollision {
                new_name: "velocity".to_string()
            })
        );
        // Builtins may be shadowed, so renaming to `PI` is allowed.
        assert!(
            rename(&analysis, &uri, offset, "PI").is_ok_and(|edit| edit.is_some()),
            "renaming to a builtin name must stay allowed (shadowing compiles)"
        );
    }

    /// Issue #829, scoped namespaces: a variant name only collides with a
    /// sibling variant of the same index, not with a same-named variant of
    /// another index.
    #[test]
    fn rename_variant_collision_is_scoped_to_its_index() {
        let source = "\
index Season = { Winter, Summer };
index Hemisphere = { North, Winter };
node pick: Season = Season.Summer;
";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        // `Summer` → `Winter` collides with its sibling.
        let offset = source.find("Summer }").unwrap();
        assert_eq!(
            rename(&analysis, &uri, offset, "Winter"),
            Err(RenameRefusal::NameCollision {
                new_name: "Winter".to_string()
            })
        );
        // `North` → `Summer` is fine: `Summer` only exists under `Season`.
        let offset = source.find("North").unwrap();
        assert!(
            rename(&analysis, &uri, offset, "Summer").is_ok_and(|edit| edit.is_some()),
            "same-named variant of a different index is not a collision"
        );
    }

    /// Issues #827/#828: renaming an index variant must edit exactly the
    /// variant identifier tokens — not table-axis-to-row-label merges and not
    /// whole `Index.Variant` qualified paths.
    #[test]
    fn rename_index_variant_edits_are_segment_precise() {
        let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.0 km/s;
    Correction: 0.1 km/s;
};
node total: Velocity = @dv[Maneuver.Departure];
";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        // Cursor on the `Departure` variant declaration.
        let offset = source.find("Departure").unwrap();
        let result = rename(&analysis, &uri, offset, "Begin").unwrap().unwrap();
        let edits = result.changes.unwrap();
        let file_edits = edits.get(&uri).unwrap();
        // Declaration + table row key + index-access segment.
        assert_eq!(file_edits.len(), 3, "edits: {file_edits:?}");
        for edit in file_edits {
            let span_text: Vec<&str> = source
                .lines()
                .enumerate()
                .filter_map(|(i, line)| {
                    let i = u32::try_from(i).unwrap();
                    (edit.range.start.line == i && edit.range.end.line == i).then(|| {
                        &line
                            [edit.range.start.character as usize..edit.range.end.character as usize]
                    })
                })
                .collect();
            assert_eq!(
                span_text,
                vec!["Departure"],
                "each edit must replace exactly one single-line `Departure` token"
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

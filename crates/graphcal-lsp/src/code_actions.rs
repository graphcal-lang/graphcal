//! textDocument/codeAction handler.
//!
//! Provides quick-fix code actions for compiler diagnostics:
//! - D020: Replace a decimal power exponent with an exactly equivalent rational.
//! - V002: Add `pub(bind)` to a required `index` / `type` / `dim`.
//! - V003: Add `pub` to a private item referenced by a public declaration.
//! - V006: Add `pub` to a leaked symbol referenced by a re-exported declaration.
//! - Unknown names: add a category-marked selective import from a loaded module.
//!
//! Each repair is dispatched on the diagnostic's typed [`RepairData`].

use std::collections::HashMap;

use graphcal_compiler::syntax::lexer::Lexer;
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::token::Token;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Diagnostic, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::analysis::AnalysisResult;
use crate::convert::LineIndex;
use crate::repair_data::{AddedVisibility, AutoImportCategory, RepairData};

/// Produce code actions for the given diagnostics.
pub fn code_actions(
    params: &CodeActionParams,
    analysis: &AnalysisResult,
    current_source: &str,
) -> Option<CodeActionResponse> {
    if !quickfix_requested(params.context.only.as_deref()) {
        return None;
    }

    let source = current_source;
    let mut actions = Vec::new();

    let uri = &params.text_document.uri;
    for diag in &params.context.diagnostics {
        match RepairData::of(diag) {
            Some(RepairData::ExactExponent { replacement }) => {
                actions.push(make_exact_power_action(diag, uri, replacement));
            }
            Some(RepairData::AddVisibility { item, visibility }) => {
                actions.extend(make_add_visibility_action(
                    diag, analysis, source, uri, &item, visibility,
                ));
            }
            Some(RepairData::AutoImport { name, category }) => {
                actions.extend(make_auto_import_actions(
                    diag, analysis, uri, &name, category,
                ));
            }
            None => {}
        }
    }

    if actions.is_empty() {
        None
    } else {
        Some(actions)
    }
}

/// Whether a request's kind filter includes Graphcal's quick fixes.
///
/// Code action kinds are hierarchical: an empty/base request includes all
/// descendants, while `source` does not include `quickfix`.
fn quickfix_requested(only: Option<&[CodeActionKind]>) -> bool {
    only.is_none_or(|kinds| {
        kinds.iter().any(|requested| {
            let requested = requested.as_str();
            requested.is_empty()
                || CodeActionKind::QUICKFIX.as_str() == requested
                || CodeActionKind::QUICKFIX
                    .as_str()
                    .strip_prefix(requested)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    })
}

/// Build category-preserving auto-import actions for an unknown name.
fn make_auto_import_actions(
    diag: &Diagnostic,
    analysis: &AnalysisResult,
    uri: &Url,
    name: &str,
    category: AutoImportCategory,
) -> Vec<CodeActionOrCommand> {
    let namespace = category.namespace();

    let mut candidates = analysis
        .import_surfaces
        .iter()
        .flat_map(|(path, items)| {
            items
                .iter()
                .filter(|item| item.name.as_str() == name && item.kind.namespace() == namespace)
                .map(move |item| (path, item))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_path, left_item), (right_path, right_item)| {
        left_path
            .as_slice()
            .cmp(right_path.as_slice())
            .then_with(|| left_item.render().cmp(&right_item.render()))
    });

    candidates
        .into_iter()
        .map(|(path, item)| {
            let path = path.as_slice().join(".");
            let import_item = item.render();
            let edit = TextEdit {
                range: Range::default(),
                new_text: format!("import {path}::{{ {import_item} }};\n"),
            };
            let mut changes = HashMap::new();
            changes.insert(uri.clone(), vec![edit]);
            CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Import `{import_item}` from `{path}`"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                is_preferred: Some(false),
                ..Default::default()
            })
        })
        .collect()
}

/// Build the D020 exact-syntax replacement. The compiler supplies the
/// replacement only after proving decimal equivalence; the LSP never derives
/// a rational from rendered text or binary64.
fn make_exact_power_action(
    diag: &Diagnostic,
    uri: &Url,
    replacement: String,
) -> CodeActionOrCommand {
    let title = format!("Use exact exponent `{replacement}`");
    let edit = TextEdit {
        range: diag.range,
        new_text: replacement,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeActionOrCommand::CodeAction(CodeAction {
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

/// V002/V003/V006: find the private item's declaration via the symbol table
/// and give it the added visibility.
///
/// The edit is placed by the declaration's own span, after its attributes,
/// never by its line: a line can hold several declarations.
fn make_add_visibility_action(
    diag: &Diagnostic,
    analysis: &AnalysisResult,
    source: &str,
    uri: &Url,
    item: &str,
    visibility: AddedVisibility,
) -> Option<CodeActionOrCommand> {
    let definition = analysis.symbol_table.unique_definition_named(item)?;
    let (target, new_text) = match declaration_head(source, definition.decl_span)? {
        // `pub index I;` becomes `pub(bind) index I;`.
        DeclarationHead::Visibility(prefix) => (prefix, visibility.marker().to_string()),
        DeclarationHead::Keyword(offset) => (Span::new(offset, 0), visibility.prefix().to_string()),
    };
    let edit = TextEdit {
        range: LineIndex::new(source).span_to_range(target),
        new_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add `{}` to `{item}`", visibility.marker()),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: Some(true),
        ..Default::default()
    }))
}

/// What a declaration starts with once its attributes are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationHead {
    /// An existing `pub` or `pub(…)` prefix, spanning the whole prefix.
    Visibility(Span),
    /// The byte offset of the declaration keyword (`node`, `const`, `dim`, …).
    Keyword(usize),
}

/// Lex the declaration at `decl_span` (which starts at its first attribute)
/// past every `#[…]` attribute.
fn declaration_head(source: &str, decl_span: Span) -> Option<DeclarationHead> {
    let text = source.get(decl_span.offset()..decl_span.offset() + decl_span.len())?;
    let mut lexer = Lexer::new(text);
    let mut depth = 0usize;
    let absolute = |span: Span| Span::new(decl_span.offset() + span.offset(), span.len());
    while let Some((token, span)) = lexer.next_token() {
        match (depth, token) {
            (0, Token::Hash) => depth = 1,
            (1, Token::LBracket) => depth = 2,
            (2.., Token::LBracket) => depth += 1,
            (2.., Token::RBracket) => depth -= 1,
            (2.., _) => {}
            (0, Token::Pub) => {
                let start = absolute(span);
                return Some(DeclarationHead::Visibility(
                    match (lexer.next_token(), lexer.next_token(), lexer.next_token()) {
                        (Some((Token::LParen, _)), Some(_), Some((Token::RParen, close))) => {
                            start.merge(absolute(close))
                        }
                        _ => start,
                    },
                ));
            }
            (0, _) => return Some(DeclarationHead::Keyword(absolute(span).offset())),
            _ => return None,
        }
        if depth == 1 && token == Token::RBracket {
            depth = 0;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;

    use tower_lsp::lsp_types::{
        CodeActionContext, PartialResultParams, TextDocumentIdentifier, WorkDoneProgressParams,
    };

    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::fn_signatures::build_fn_signatures;
    use crate::symbol_table;

    /// Build a minimal `AnalysisResult` from source text.
    fn analysis_from_source(source: &str) -> AnalysisResult {
        let raw_ast = graphcal_compiler::syntax::parser::Parser::new(source)
            .parse_file()
            .unwrap();
        let desugared = graphcal_compiler::desugar::desugared_ast::File::from(raw_ast);
        let ast = desugared;
        let symbol_table = symbol_table::build_for_buffer(&ast, source);
        AnalysisResult {
            inputs: crate::workspace_revision::AnalysisInputs::untracked_for_test(
                crate::workspace_revision::DocumentIdentity::virtual_uri(
                    tower_lsp::lsp_types::Url::parse("file:///test.gcl").unwrap(),
                ),
            ),
            source: Arc::new(source.to_string()),
            symbol_table,
            project_symbols: crate::project_symbols::ProjectSymbols::Incomplete,
            imported_definitions: StdHashMap::new(),
            imported_bindings: Vec::new(),
            import_surfaces: StdHashMap::new(),
            diagnostics: Arc::new(StdHashMap::new()),
            eval_values: StdHashMap::new(),
            view: None,
            fn_signatures: build_fn_signatures(),
            extern_fn_signatures: StdHashMap::new(),
            import_links: Vec::new(),
            buffer_parsed: true,
        }
    }

    fn head(source: &str) -> Option<DeclarationHead> {
        declaration_head(source, Span::new(0, source.len()))
    }

    #[test]
    fn declaration_head_skips_attributes_and_finds_visibility() {
        assert_eq!(head("index Phase;"), Some(DeclarationHead::Keyword(0)));
        let source = "#[expected_fail(Mode#Boost)] #[doc] const node x: Int = 1;";
        assert_eq!(
            head(source),
            Some(DeclarationHead::Keyword(source.find("const").unwrap()))
        );
        assert_eq!(
            head("pub index Phase;"),
            Some(DeclarationHead::Visibility(Span::new(0, 3)))
        );
        assert_eq!(
            head("#[a] pub(bind) index Phase;"),
            Some(DeclarationHead::Visibility(Span::new(5, 9)))
        );
        assert_eq!(head("#[unterminated"), None);
    }

    /// The edits of the first quick fix offered for `source`'s diagnostics
    /// with `code`, applied to `source`.
    fn apply_first_quick_fix(source: &str, code: &str) -> String {
        let uri = Url::parse("untitled:fix.gcl").unwrap();
        let analysis = crate::analysis_pipeline::run_analysis_for_test(&uri, source);
        let diagnostics: Vec<_> = analysis.diagnostics[&uri]
            .iter()
            .filter(|diagnostic| {
                diagnostic.code
                    == Some(tower_lsp::lsp_types::NumberOrString::String(
                        code.to_string(),
                    ))
            })
            .cloned()
            .collect();
        assert!(!diagnostics.is_empty(), "{:?}", analysis.diagnostics);
        let actions =
            code_actions(&make_params(&uri, diagnostics), &analysis, source).expect("a quick fix");
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        crate::test_edits::apply_text_edits(source, edits)
    }

    /// LSP-7: the fix edits the named declaration, not the first declaration
    /// on its line, and the fixed program compiles.
    #[test]
    fn add_visibility_edits_the_named_declaration_by_span() {
        for (source, code, fixed) in [
            (
                "node a: Dimensionless = 1.0; dim Foo = Length;\npub node x: Foo = 1.0 m;\n",
                "graphcal::V003",
                "node a: Dimensionless = 1.0; pub dim Foo = Length;\npub node x: Foo = 1.0 m;\n",
            ),
            (
                "node a: Dimensionless = 1.0; index Phase;\n",
                "graphcal::V002",
                "node a: Dimensionless = 1.0; pub(bind) index Phase;\n",
            ),
            (
                "pub index Phase;\n",
                "graphcal::V002",
                "pub(bind) index Phase;\n",
            ),
        ] {
            let updated = apply_first_quick_fix(source, code);
            assert_eq!(updated, fixed);
            let uri = Url::parse("untitled:fix.gcl").unwrap();
            let checked = crate::analysis_pipeline::run_analysis_for_test(&uri, &updated);
            assert!(
                checked.has_no_diagnostics(),
                "{updated}: {:?}",
                checked.diagnostics
            );
        }
    }

    fn make_diag(code: &str, message: &str, range: Range, data: Option<&RepairData>) -> Diagnostic {
        Diagnostic {
            range,
            severity: Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR),
            code: Some(tower_lsp::lsp_types::NumberOrString::String(
                code.to_string(),
            )),
            source: Some("graphcal".to_string()),
            message: message.to_string(),
            data: data.and_then(RepairData::to_value),
            ..Default::default()
        }
    }

    fn public(item: &str) -> RepairData {
        RepairData::AddVisibility {
            item: item.to_string(),
            visibility: AddedVisibility::Public,
        }
    }

    fn bindable(item: &str) -> RepairData {
        RepairData::AddVisibility {
            item: item.to_string(),
            visibility: AddedVisibility::Bindable,
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
    fn code_action_kind_filter_excludes_quick_fixes() {
        let analysis = analysis_from_source("index Phase;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required index `Phase` must be declared `pub(bind)`",
            Range::default(),
            Some(&bindable("Phase")),
        );
        let mut params = make_params(&uri, vec![diag]);
        params.context.only = Some(vec![CodeActionKind::SOURCE]);

        assert!(code_actions(&params, &analysis, &analysis.source).is_none());
    }

    #[test]
    fn code_action_kind_filter_includes_quick_fixes() {
        let analysis = analysis_from_source("index Phase;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::V002",
            "required index `Phase` must be declared `pub(bind)`",
            Range::default(),
            Some(&bindable("Phase")),
        );
        let mut params = make_params(&uri, vec![diag]);
        params.context.only = Some(vec![CodeActionKind::QUICKFIX]);

        assert_eq!(
            code_actions(&params, &analysis, &analysis.source)
                .expect("quick fixes")
                .len(),
            1
        );
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
            Some(&bindable("Phase")),
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(action.title, "Add `pub(bind)` to `Phase`");
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
            Some(&bindable("Phase")),
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
            Some(&public("Velocity")),
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
            Some(&public("Velocity")),
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
            Some(&public("Inner")),
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
    fn unknown_name_auto_import_uses_canonical_category_marker() {
        let mut analysis = analysis_from_source("node x: Information = 1.0;");
        analysis.import_surfaces.insert(
            graphcal_compiler::syntax::non_empty::NonEmpty::new(
                "finance".to_string(),
                vec!["units".to_string()],
            ),
            vec![
                graphcal_compiler::resolve::exports::ExportedImportItem {
                    name: graphcal_compiler::syntax::names::NameAtom::parse("Information").unwrap(),
                    kind: graphcal_compiler::resolve::category::ExportedImportItemKind::Dimension,
                },
                graphcal_compiler::resolve::exports::ExportedImportItem {
                    name: graphcal_compiler::syntax::names::NameAtom::parse("Information").unwrap(),
                    kind: graphcal_compiler::resolve::category::ExportedImportItemKind::Unit(
                        graphcal_compiler::syntax::ast::UnitConstness::Const,
                    ),
                },
            ],
        );
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::D004",
            "unknown dimension `Information`",
            Range::default(),
            Some(&RepairData::AutoImport {
                name: "Information".to_string(),
                category: AutoImportCategory::Dimension,
            }),
        );

        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        assert_eq!(actions.len(), 1);
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        assert_eq!(
            action.title,
            "Import `dim Information` from `finance.units`"
        );
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(
            edits[0].new_text,
            "import finance.units::{ dim Information };\n"
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
    fn d020_replaces_only_with_compiler_supplied_exact_exponent() {
        let analysis =
            analysis_from_source("param x: Length = 1.0 m;\nnode bad: Length^(1/4) = @x ^ 0.25;");
        let uri = Url::parse("file:///test.gcl").unwrap();
        let range = Range {
            start: Position {
                line: 1,
                character: 37,
            },
            end: Position {
                line: 1,
                character: 41,
            },
        };
        let diag = make_diag(
            "graphcal::D020",
            "float syntax cannot be the exponent of a dimensioned base",
            range,
            Some(&RepairData::ExactExponent {
                replacement: "(1/4)".to_string(),
            }),
        );
        let params = make_params(&uri, vec![diag]);
        let actions = code_actions(&params, &analysis, &analysis.source).unwrap();
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits[0].range, range);
        assert_eq!(edits[0].new_text, "(1/4)");
    }

    #[test]
    fn d019_indexed_comparison_has_no_automatic_rewrite() {
        let analysis = analysis_from_source(
            "index Case = { A, B };\nnode values: Length[Case] = { Case#A: 1.0 m, Case#B: 2.0 m };\nnode bad: Bool[Case] = @values < 3.0 m;",
        );
        let uri = Url::parse("file:///test.gcl").unwrap();
        let diag = make_diag(
            "graphcal::D019",
            "comparison operators require unindexed operands, found Length[Case]",
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

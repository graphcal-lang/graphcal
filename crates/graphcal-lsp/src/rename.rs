//! textDocument/rename and textDocument/prepareRename handlers.

use tower_lsp::lsp_types::{PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::analysis::AnalysisResult;
use crate::convert::LineIndex;
use crate::project_symbols::ProjectOccurrence;
use crate::rename_check::{PlannedRename, RenameCheck, RenameCheckFailure};
use crate::resolve::{ResolvedSymbol, SymbolLocation, reference_lookup_keys, resolve_symbol_at};
use crate::symbol_identity::ExternFunctionId;
use crate::symbol_table::SymbolKey;
use graphcal_compiler::resolved_name::{ResolvedConstructorName, ResolvedStructTypeName};
use graphcal_compiler::syntax::span::Span;
use graphcal_compiler::syntax::token::SourceIdentifier;
use graphcal_compiler::syntax::type_name::record_constructor_name;

/// Check whether a name is a valid Graphcal identifier.
///
/// Delegates to [`SourceIdentifier::parse`], which asks the lexer, so hard
/// keywords (`node`, `param`, `true`, …) are rejected while contextual keyword
/// tokens remain valid identifiers.
fn is_valid_identifier(name: &str) -> bool {
    SourceIdentifier::parse(name).is_ok()
}

const fn resolved_definition<'a>(
    resolved: &'a ResolvedSymbol<'_>,
) -> &'a crate::symbol_table::DefinitionInfo {
    match &resolved.location {
        SymbolLocation::Local(definition) => definition,
        SymbolLocation::Imported(imported) => &imported.definition,
    }
}

/// Validate that the occurrence set is complete and that the cursor spells
/// the canonical leaf rather than a preserved import alias.
fn validate_rename_target(
    analysis: &AnalysisResult,
    uri: &Url,
    resolved: &ResolvedSymbol<'_>,
) -> Result<bool, RenameRefusal> {
    let definition = resolved_definition(resolved);
    if definition.is_builtin() {
        return Ok(false);
    }
    // The plugin ABI fixes an extern function's name: the declaration must
    // spell the plugin's export, so no rename of it can compile.
    if let SymbolKey::ExternFunction(function) = &resolved.key {
        return Err(RenameRefusal::ExternFunction {
            function: function.clone(),
        });
    }

    if let Some(project) = analysis.project_symbols.complete() {
        if project.is_externally_visible(&resolved.key) && !project.covers_reverse_dependencies() {
            return Err(RenameRefusal::IncompleteProjectIndex {
                name: definition.name.clone(),
            });
        }
        if project.definition(&resolved.key).is_none() {
            return Err(RenameRefusal::IncompleteProjectIndex {
                name: definition.name.clone(),
            });
        }
        if project.has_ambiguous_reference_to(&resolved.key) {
            return Err(RenameRefusal::AmbiguousOccurrence {
                name: definition.name.clone(),
            });
        }
        if resolved.is_reference
            && !project.occurrence_can_initiate_rename(uri, resolved.cursor_span, &resolved.key)
        {
            return Err(RenameRefusal::ImportAlias {
                alias: analysis
                    .source
                    .get(
                        resolved.cursor_span.offset()
                            ..resolved.cursor_span.offset() + resolved.cursor_span.len(),
                    )
                    .unwrap_or(&definition.name)
                    .to_string(),
            });
        }
        return Ok(true);
    }

    if !matches!(resolved.location, SymbolLocation::Local(_))
        || analysis.symbol_table.is_externally_visible(&resolved.key)
    {
        return Err(RenameRefusal::IncompleteProjectIndex {
            name: definition.name.clone(),
        });
    }
    if analysis
        .symbol_table
        .has_ambiguous_reference_to(&resolved.key)
    {
        return Err(RenameRefusal::AmbiguousOccurrence {
            name: definition.name.clone(),
        });
    }
    Ok(true)
}

/// Validate a rename and return the current name's range and placeholder.
///
/// `Ok(None)` means there is nothing renameable at the cursor; `Err` is an
/// explicit refusal whose reason the client shows to the user, instead of a
/// generic "this element can't be renamed".
pub fn prepare_rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
) -> Result<Option<PrepareRenameResponse>, RenameRefusal> {
    let Some(resolved) = resolve_symbol_at(analysis, offset) else {
        return Ok(None);
    };
    if !validate_rename_target(analysis, uri, &resolved)? {
        return Ok(None);
    }

    let def = resolved_definition(&resolved);
    let span = resolved.cursor_span;
    // Fall back to the definition's name if span slicing ever fails — never
    // a synthetic key rendering.
    let placeholder = analysis
        .source
        .get(span.offset()..span.offset() + span.len())
        .unwrap_or(&def.name)
        .to_string();

    Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: LineIndex::new(&analysis.source).span_to_range(span),
        placeholder,
    }))
}

/// Why a rename request was explicitly refused.
///
/// Surfaced to the client as a descriptive JSON-RPC error rather than a
/// silent `null` response: applying the rename anyway would produce a
/// non-compiling buffer, which is the worst available outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameRefusal {
    /// The new name is not a lexable identifier (or is a hard keyword).
    InvalidIdentifier { new_name: String },
    /// The renamed program raises a compile error the original did not
    /// (a reserved built-in name, a duplicate, a shadowed term, …); `reason`
    /// is the compiler's message.
    WouldNotCompile { new_name: String, reason: String },
    /// A reference would silently resolve to a different definition.
    WouldChangeResolution { new_name: String },
    /// Transitive project loading did not produce a complete occurrence set.
    IncompleteProjectIndex { name: String },
    /// A tolerant occurrence has the right namespace/spelling but no unique ID.
    AmbiguousOccurrence { name: String },
    /// The cursor is on a preserved local alias, not the canonical API leaf.
    ImportAlias { alias: String },
    /// An affected open document changed after the project index was built.
    StaleProjectSnapshot,
    /// Extern function names are fixed by the plugin they come from.
    ExternFunction { function: ExternFunctionId },
}

impl std::fmt::Display for RenameRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier { new_name } => {
                write!(f, "`{new_name}` is not a valid graphcal identifier")
            }
            Self::WouldNotCompile { new_name, reason } => {
                write!(f, "cannot rename to `{new_name}`: {reason}")
            }
            Self::WouldChangeResolution { new_name } => write!(
                f,
                "cannot rename to `{new_name}`: a reference would then resolve to a different declaration"
            ),
            Self::IncompleteProjectIndex { name } => write!(
                f,
                "cannot safely rename `{name}`: a complete project occurrence index is unavailable"
            ),
            Self::AmbiguousOccurrence { name } => write!(
                f,
                "cannot safely rename `{name}`: at least one occurrence has no unique semantic target"
            ),
            Self::ImportAlias { alias } => write!(
                f,
                "cannot rename from import alias `{alias}`; start the canonical API rename from its declaration or unaliased import name"
            ),
            Self::StaleProjectSnapshot => f.write_str(
                "cannot safely rename because an affected document changed after project analysis",
            ),
            Self::ExternFunction { function } => write!(
                f,
                "cannot rename extern function `{}::{}`: its name is fixed by the plugin, so the declaration must spell the plugin's export",
                function.plugin(),
                function.function()
            ),
        }
    }
}

/// Perform the rename, returning a workspace edit.
///
/// The planned edit is offered only after [`RenameCheck`] re-checked the
/// edited project, so the compiler's own naming rules decide what is
/// accepted. `Ok(None)` means there is nothing renameable at the cursor (the
/// client sees a plain `null`); `Err` is an explicit refusal with a reason
/// the client should show to the user.
pub fn rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    new_name: &str,
    check: &RenameCheck<'_>,
) -> Result<Option<WorkspaceEdit>, RenameRefusal> {
    let Some((plan, old_name)) = plan_rename(analysis, uri, offset, new_name)? else {
        return Ok(None);
    };
    match check.check(analysis, uri, &plan) {
        Ok(()) => {}
        Err(RenameCheckFailure::Cancelled) => return Ok(None),
        Err(RenameCheckFailure::NewCompileError(reason)) => {
            return Err(RenameRefusal::WouldNotCompile {
                new_name: new_name.to_string(),
                reason,
            });
        }
        Err(RenameCheckFailure::ChangedResolution) => {
            return Err(RenameRefusal::WouldChangeResolution {
                new_name: new_name.to_string(),
            });
        }
        Err(RenameCheckFailure::MissingSource) => {
            return Err(RenameRefusal::IncompleteProjectIndex { name: old_name });
        }
    }
    Ok(Some(workspace_edit(analysis, uri, &plan)))
}

/// Every occurrence the rename at `offset` respells, before any check of the
/// edited program, with the symbol's current name.
fn plan_rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
    new_name: &str,
) -> Result<Option<(PlannedRename, String)>, RenameRefusal> {
    if !is_valid_identifier(new_name) {
        return Err(RenameRefusal::InvalidIdentifier {
            new_name: new_name.to_string(),
        });
    }

    let Some(resolved) = resolve_symbol_at(analysis, offset) else {
        return Ok(None);
    };
    if !validate_rename_target(analysis, uri, &resolved)? {
        return Ok(None);
    }
    let definition = resolved_definition(&resolved);
    // Renaming to the symbol's current name is a no-op.
    if definition.name == new_name {
        return Ok(None);
    }

    let targets: Vec<SymbolKey> = rename_unit(analysis, &resolved.key)
        .iter()
        .flat_map(reference_lookup_keys)
        .collect();
    let table = &analysis.symbol_table;
    let occurrences: Vec<(Url, Span)> = analysis.project_symbols.complete().map_or_else(
        || {
            targets
                .iter()
                .flat_map(|key| {
                    table
                        .find_all_references(key)
                        .into_iter()
                        .map(|reference| reference.span)
                        .chain(
                            table
                                .definitions
                                .get(key)
                                .filter(|definition| !definition.name_span.is_empty())
                                .map(|definition| definition.name_span),
                        )
                })
                .map(|span| (uri.clone(), span))
                .collect()
        },
        |project| {
            targets
                .iter()
                .flat_map(|target| project.rename_occurrences(target))
                .map(|occurrence| (occurrence.uri, occurrence.span))
                .collect()
        },
    );
    let plan = PlannedRename::new(new_name, occurrences);
    Ok((!plan.is_empty()).then(|| (plan, definition.name.clone())))
}

/// The identities one rename respells together.
///
/// A record type `type T { T(..) }` and its sole constructor share one
/// spelling by the language's record rule: renaming either alone turns the
/// record into a union (S007). Both are declared together in the renamed
/// symbol's own file.
fn rename_unit(analysis: &AnalysisResult, key: &SymbolKey) -> Vec<SymbolKey> {
    let table = &analysis.symbol_table;
    let record_constructor = |nominal: &ResolvedStructTypeName| {
        nominal.constructor(record_constructor_name(nominal.leaf()))
    };
    let record = match key {
        SymbolKey::StructType(nominal) => Some((nominal.clone(), record_constructor(nominal))),
        SymbolKey::Constructor(constructor) => {
            table
                .definitions
                .keys()
                .find_map(|candidate| match candidate {
                    SymbolKey::StructType(nominal)
                        if record_constructor(nominal) == *constructor =>
                    {
                        Some((nominal.clone(), constructor.clone()))
                    }
                    _ => None,
                })
        }
        _ => None,
    };
    let counterpart = record
        .filter(|(nominal, constructor)| is_record(table, nominal, constructor))
        .map(|(nominal, constructor)| match key {
            SymbolKey::StructType(_) => SymbolKey::Constructor(constructor),
            _ => SymbolKey::StructType(nominal),
        });
    std::iter::once(key.clone()).chain(counterpart).collect()
}

/// Whether `constructor` is the only constructor `nominal` declares.
fn is_record(
    table: &crate::symbol_table::SymbolTable,
    nominal: &ResolvedStructTypeName,
    constructor: &ResolvedConstructorName,
) -> bool {
    let Some(body) = table
        .definitions
        .get(&SymbolKey::StructType(nominal.clone()))
        .map(|definition| definition.decl_span)
    else {
        return false;
    };
    let mut constructors = table.definitions.iter().filter(|(candidate, definition)| {
        matches!(candidate, SymbolKey::Constructor(other) if other.owner() == nominal.owner())
            && body.offset() <= definition.decl_span.offset()
            && definition.decl_span.offset() + definition.decl_span.len()
                <= body.offset() + body.len()
    });
    matches!(
        (constructors.next(), constructors.next()),
        (Some((SymbolKey::Constructor(only), _)), None) if only == constructor
    )
}

/// The client edit of a checked plan, in the analysed documents' coordinates.
fn workspace_edit(analysis: &AnalysisResult, uri: &Url, plan: &PlannedRename) -> WorkspaceEdit {
    let root_lines = LineIndex::new(&analysis.source);
    let mut project_lines = analysis
        .project_symbols
        .complete()
        .map(crate::project_symbols::ProjectSymbolIndex::line_indexes);
    let changes = plan
        .occurrences()
        .iter()
        .map(|(document, spans)| {
            let edits = spans
                .iter()
                .filter_map(|span| {
                    let range = match project_lines.as_mut() {
                        Some(lines) => lines.range(&ProjectOccurrence {
                            uri: document.clone(),
                            span: *span,
                        })?,
                        None if document == uri => root_lines.span_to_range(*span),
                        None => return None,
                    };
                    Some(TextEdit {
                        range,
                        new_text: plan.new_name().to_string(),
                    })
                })
                .collect();
            (document.clone(), edits)
        })
        .collect();
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real analysis of a standalone (untitled) buffer.
    fn analysis_from_source(source: &str) -> AnalysisResult {
        crate::analysis_pipeline::run_analysis_for_test(&test_uri(), source)
    }

    fn test_uri() -> Url {
        Url::parse("untitled:test.gcl").unwrap()
    }

    /// [`super::rename`] with the re-check analysing no other open buffers.
    fn rename(
        analysis: &AnalysisResult,
        uri: &Url,
        offset: usize,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, RenameRefusal> {
        let cancellation = graphcal_compiler::cancellation::CancellationToken::unbounded();
        let check = RenameCheck::new(
            Vec::new(),
            crate::analysis_pipeline::test_plugin_host(),
            &cancellation,
        );
        super::rename(analysis, uri, offset, new_name, &check)
    }

    /// The rename was refused because the compiler rejects the edited program
    /// with diagnostic `code`.
    #[track_caller]
    fn assert_refused_by_compiler(
        result: Result<Option<WorkspaceEdit>, RenameRefusal>,
        code: &str,
    ) {
        match result {
            Err(RenameRefusal::WouldNotCompile { reason, .. }) => {
                assert!(reason.starts_with(code), "{reason}");
            }
            other => panic!("expected a {code} refusal, got {other:?}"),
        }
    }

    fn apply_edits(source: &str, edits: &[TextEdit]) -> String {
        crate::test_edits::apply_text_edits(source, edits)
    }

    /// LSP-6: an extern function's name is fixed by its plugin, so renaming
    /// it from the declaration or a call site is refused with a dedicated
    /// reason instead of editing only the declaration (which then fails W003).
    #[test]
    fn rename_refuses_extern_functions() {
        let source = r#"
import plugin "graphcal:demo" as demo {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}
node y: Dimensionless = demo::lerp(0.0, 1.0, 0.5);
"#;
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        for cursor in [
            source.find("fn lerp").unwrap() + "fn ".len(),
            source.find("demo::lerp(").unwrap() + "demo::".len(),
        ] {
            let refusal = rename(&analysis, &uri, cursor, "blend").unwrap_err();
            assert!(
                matches!(
                    &refusal,
                    RenameRefusal::ExternFunction { function }
                        if function.plugin().as_str() == "demo"
                            && function.function().as_str() == "lerp"
                ),
                "{refusal:?}"
            );
            assert!(refusal.to_string().contains("`demo::lerp`"), "{refusal}");
            assert_eq!(prepare_rename(&analysis, &uri, cursor), Err(refusal));
        }
    }

    #[test]
    fn rename_expression_local_uses_definition_spelling() {
        let source = r"
index Step = range(0.0 s, 1.0 s, step: 1.0 s);
node y: Dimensionless[Step] = unfold(
    Step,
    0.0,
    |prev_y, prev_t, t| prev_y + (t - prev_t) / 1.0 s
);
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let cursor = source.find("prev_y").unwrap();
        let edit = rename(&analysis, &uri, cursor, "previous")
            .unwrap()
            .expect("expression local should be renameable");

        assert_eq!(edit.changes.unwrap()[&uri].len(), 2);
        assert_refused_by_compiler(rename(&analysis, &uri, cursor, "t"), "graphcal::N033");
    }

    #[test]
    fn rename_field_access_isolated_by_owning_constructor() {
        let source = r"
type Item { Item(mass: Dimensionless), }
type Other { Other(mass: Dimensionless), }
param item: Item = Item(mass: 1.0);
param other: Other = Other(mass: 2.0);
node item_mass: Dimensionless = @item.mass;
node other_mass: Dimensionless = @other.mass;
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let definition = source.find("Item(mass").unwrap() + "Item(".len();
        let edit = rename(&analysis, &uri, definition, "weight")
            .unwrap()
            .expect("record field should be renameable");
        let edits = &edit.changes.unwrap()[&uri];

        let lines = LineIndex::new(source);
        let mut actual: Vec<_> = edits
            .iter()
            .map(|edit| (edit.range.start.line, edit.range.start.character))
            .collect();
        let mut expected: Vec<_> = [
            definition,
            source.find("= Item(mass").unwrap() + "= Item(".len(),
            source.find("@item.mass").unwrap() + "@item.".len(),
        ]
        .into_iter()
        .map(|offset| {
            let position = lines.position(offset);
            (position.line, position.character)
        })
        .collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn rename_covers_nested_dag_include_and_composition_occurrences() {
        let source = r"
param outer: Dimensionless = 2.0;
dag d {
    param inner: Dimensionless;
    node doubled: Dimensionless = @inner * 2.0;
}
include d(inner: @outer)::{ doubled as result };
plot p = { mark: point, encode: { x: @outer } };
figure f = { plots: [p] };
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        for (needle, new_name, expected_edits) in [
            ("param outer", "outside", 3),
            ("param inner", "inside", 3),
            ("node doubled", "twice", 2),
            ("plot p", "chart", 2),
        ] {
            let offset = source.find(needle).unwrap() + needle.rfind(' ').unwrap() + 1;
            let edit = rename(&analysis, &uri, offset, new_name)
                .unwrap()
                .unwrap_or_else(|| panic!("`{needle}` should be renameable"));
            assert_eq!(
                edit.changes.unwrap()[&uri].len(),
                expected_edits,
                "partial rename for `{needle}`"
            );
        }
    }

    #[test]
    fn rename_respects_compiler_namespaces_for_same_spelling() {
        let source = "pub const node scale: Dimensionless = 2.0;\n\
                      pub const unit scale: Length = 1.0 m;\n\
                      node value: Length = @scale * 1.0 scale;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        let const_use = source.find("= @scale *").unwrap() + 3;
        let const_edit = rename(&analysis, &uri, const_use, "factor")
            .unwrap()
            .unwrap();
        let const_edits = &const_edit.changes.unwrap()[&uri];
        assert_eq!(const_edits.len(), 2);
        assert!(const_edits.iter().any(|edit| edit.range.start.line == 0));
        assert!(!const_edits.iter().any(|edit| edit.range.start.line == 1));

        let unit_use = source.rfind("scale").unwrap();
        let unit_edit = rename(&analysis, &uri, unit_use, "scaled_metre")
            .unwrap()
            .unwrap();
        let unit_edits = &unit_edit.changes.unwrap()[&uri];
        assert_eq!(unit_edits.len(), 2);
        assert!(unit_edits.iter().any(|edit| edit.range.start.line == 1));
        assert!(!unit_edits.iter().any(|edit| edit.range.start.line == 0));
    }

    #[test]
    fn local_rename_rejects_overlap_and_preserves_independent_scopes() {
        let uri = test_uri();
        let source = "node grid: Int[Fin(2), Fin(3)] = for row: Fin(2) { for col: Fin(3) { to_int(row) * 10 + to_int(col) } };";
        let analysis = analysis_from_source(source);
        for (old, new) in [("row", "col"), ("col", "row")] {
            assert_refused_by_compiler(
                rename(&analysis, &uri, source.find(old).unwrap(), new),
                "graphcal::N033",
            );
        }
        let source = "node first: Int[Fin(2)] = for row: Fin(2) { to_int(row) }; node other_values: Int[Fin(2)] = for col: Fin(2) { to_int(col) };";
        let analysis = analysis_from_source(source);
        let changes = rename(&analysis, &uri, source.find("row").unwrap(), "col")
            .unwrap()
            .unwrap()
            .changes
            .unwrap();
        let updated = apply_edits(source, &changes[&uri]);
        let checked = crate::analysis_pipeline::run_analysis_for_test(&uri, &updated);
        assert!(
            checked.has_no_diagnostics(),
            "{updated}: {:?}",
            checked.diagnostics
        );
    }

    #[test]
    fn local_rename_checks_match_payload_and_scan_lambda_scopes() {
        let uri = test_uri();
        for (source, old, new) in [
            (
                "node samples: Dimensionless[Fin(2)] = for i: Fin(2) { 1.0 }; node totals: Dimensionless[Fin(2)] = scan(@samples, 0.0, |acc, val| acc + val);",
                "acc",
                "val",
            ),
            (
                "type Pair { Pair(x: Int, y: Int), } node pair: Pair = Pair(x: 1, y: 2); node total: Int = match @pair { Pair(x: left, y: right) => left + right };",
                "left",
                "right",
            ),
        ] {
            let analysis = analysis_from_source(source);
            assert_refused_by_compiler(
                rename(&analysis, &uri, source.find(old).unwrap(), new),
                "graphcal::N033",
            );
        }
        let source = "type Choice { A(x: Int), B(x: Int), } node chosen: Choice = A(x: 1); node total: Int = match @chosen { A(x: left) => left, B(x: right) => right };";
        let analysis = analysis_from_source(source);
        let changes = rename(&analysis, &uri, source.find("left").unwrap(), "right")
            .unwrap()
            .unwrap()
            .changes
            .unwrap();
        let updated = apply_edits(source, &changes[&uri]);
        let checked = crate::analysis_pipeline::run_analysis_for_test(&uri, &updated);
        assert!(checked.has_no_diagnostics(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn rename_param_from_definition() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

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
        let updated = apply_edits(source, file_edits);
        assert!(
            crate::analysis_pipeline::run_analysis_for_test(&uri, &updated).has_no_diagnostics()
        );
    }

    #[test]
    fn rename_generic_parameter_edits_defaults_and_payload_types() {
        let source = "type Sized<N: Nat, M: Nat = N + 1> {\n\
                      Sized(values: Dimensionless[Fin(N)]),\n\
                      }";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let offset = source.find("N: Nat").unwrap();

        let result = rename(&analysis, &uri, offset, "Size").unwrap().unwrap();
        let edits = result.changes.unwrap();
        let file_edits = edits.get(&uri).unwrap();
        assert_eq!(file_edits.len(), 3);
        assert!(file_edits.iter().all(|edit| edit.new_text == "Size"));
    }

    #[test]
    fn rename_param_from_reference() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x + 1.0;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        // Cursor on "x" in "@x" — offset of the ident after @
        let at_x = source.find("@x").unwrap() + 1;
        let result = rename(&analysis, &uri, at_x, "velocity").unwrap().unwrap();
        let edits = result.changes.unwrap();
        let file_edits = edits.get(&uri).unwrap();
        assert_eq!(file_edits.len(), 2);
    }

    #[test]
    fn prepare_rename_builtin_rejected() {
        let source = "node lower: Dimensionless = least(1.0, 2.0);\n\
                      node reduced: Dimensionless = minimum(1.0);";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        for builtin in ["least", "minimum"] {
            let offset = source.find(builtin).unwrap();
            let result = prepare_rename(&analysis, &uri, offset);
            assert!(
                matches!(result, Ok(None)),
                "builtin `{builtin}` should not be renameable"
            );
        }
    }

    #[test]
    fn rename_invalid_name_rejected() {
        let source = "param x: Dimensionless = 1.0;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

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
    fn rename_imported_symbol_refuses_unindexed_sibling_importers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/helper")).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"helper\"\n",
        )
        .unwrap();
        let lib_path = dir.path().join("src/helper/lib.gcl");
        let lib_text = "pub const node y: Dimensionless = 2.0;";
        std::fs::write(&lib_path, lib_text).unwrap();
        let main_path = dir.path().join("src/helper/main.gcl");
        let main_text = "import helper.lib::{y};\nnode z: Dimensionless = @y + 1.0;\n";
        std::fs::write(&main_path, main_text).unwrap();
        // This sibling is closed and absent from main's dependency closure.
        std::fs::write(dir.path().join("src/helper/sibling.gcl"), main_text).unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let analysis = crate::analysis_pipeline::run_analysis_for_test(&main_uri, main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected the import to load cleanly, got: {:?}",
            analysis.diagnostics,
        );

        let defining_analysis = crate::analysis_pipeline::run_analysis_for_test(&lib_uri, lib_text);
        let definition_cursor = lib_text.find(" y:").unwrap() + 1;
        assert_eq!(
            rename(
                &defining_analysis,
                &lib_uri,
                definition_cursor,
                "unsafe_partial"
            ),
            Err(RenameRefusal::IncompleteProjectIndex {
                name: "y".to_string()
            }),
            "an exported definition must be refused when reverse importers are not indexed"
        );

        let cursor = main_text.find("@y").unwrap() + 1;
        for cursor in [cursor, main_text.find("{y}").unwrap() + 1] {
            assert!(matches!(
                prepare_rename(&analysis, &main_uri, cursor),
                Err(RenameRefusal::IncompleteProjectIndex { .. })
            ));
            assert!(matches!(
                rename(&analysis, &main_uri, cursor, "renamed"),
                Err(RenameRefusal::IncompleteProjectIndex { .. })
            ));
        }

        // Issue #829: renaming a local declaration to the name of an
        // imported symbol collides as well — both would be visible.
        let z_cursor = main_text.find("node z").unwrap() + "node ".len();
        assert_refused_by_compiler(
            rename(&analysis, &main_uri, z_cursor, "y"),
            "graphcal::N001",
        );
    }

    #[test]
    fn project_rename_covers_include_selectors_and_preserves_output_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src/helper");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"helper\"\n",
        )
        .unwrap();
        for (name, source) in [
            ("lib.gcl", "pub node doubled: Dimensionless = 2.0;\n"),
            (
                "consumer.gcl",
                "include helper.lib()::{ doubled as local };\npub node result: Dimensionless = @local + 1.0;\n",
            ),
            (
                "main.gcl",
                "include helper.lib()::{ doubled };\ninclude helper.consumer()::{ result };\nnode total: Dimensionless = @doubled + @result;\n",
            ),
        ] {
            std::fs::write(source_dir.join(name), source).unwrap();
        }
        let main_path = source_dir.join("main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let main_text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = crate::analysis_pipeline::run_analysis_for_test(&main_uri, &main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean include project: {:?}",
            analysis.diagnostics,
        );

        let cursor = main_text.find("@doubled").unwrap() + 1;
        assert!(matches!(
            rename(&analysis, &main_uri, cursor, "tripled"),
            Err(RenameRefusal::IncompleteProjectIndex { .. })
        ));
        assert!(matches!(
            prepare_rename(&analysis, &main_uri, cursor),
            Err(RenameRefusal::IncompleteProjectIndex { .. })
        ));
    }

    #[test]
    fn project_rename_covers_reexports_aliases_and_same_leaf_owners() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src/helper");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            dir.path().join("graphcal.toml"),
            "[package]\nname = \"helper\"\n",
        )
        .unwrap();
        let files = [
            ("lib.gcl", "pub const node y: Dimensionless = 2.0;\n"),
            (
                "a.gcl",
                "import helper.lib::{ pub y };\nconst node occupied: Dimensionless = 0.0;\npub const node a_value: Dimensionless = @y;\n",
            ),
            (
                "b.gcl",
                "import helper.lib::{ y as alias };\npub const node b_value: Dimensionless = @alias;\n",
            ),
            ("other.gcl", "pub const node y: Dimensionless = 40.0;\n"),
            (
                "main.gcl",
                "import helper.lib::{ y };\nimport helper.a::{ y as through_a, a_value };\nimport helper.b::{ b_value };\nimport helper.other as other;\nnode total: Dimensionless = @y + @through_a + @a_value + @b_value + @other::y;\n",
            ),
        ];
        for (name, source) in files {
            std::fs::write(source_dir.join(name), source).unwrap();
        }

        let main_path = source_dir.join("main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let main_text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = crate::analysis_pipeline::run_analysis_for_test(&main_uri, &main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean project: {:?}",
            analysis.diagnostics,
        );
        let cursor = main_text.rfind("@y +").unwrap() + 1;

        let references = crate::references::references(&analysis, &main_uri, cursor, true)
            .expect("project references");
        assert_eq!(references.len(), 11);
        let alias_cursor = main_text.rfind("through_a +").unwrap();
        assert!(matches!(
            prepare_rename(&analysis, &main_uri, alias_cursor),
            Err(RenameRefusal::IncompleteProjectIndex { .. })
        ));
        assert!(matches!(
            rename(&analysis, &main_uri, alias_cursor, "surprise"),
            Err(RenameRefusal::IncompleteProjectIndex { .. })
        ));
        for proposed in ["occupied", "renamed"] {
            assert!(matches!(
                rename(&analysis, &main_uri, cursor, proposed),
                Err(RenameRefusal::IncompleteProjectIndex { .. })
            ));
        }
    }

    #[test]
    fn rename_to_keyword_rejected() {
        // Regression: keywords passed the [A-Za-z_]\w* shape check, so
        // renaming a param to `node` produced unparsable code.
        let source = "param x: Dimensionless = 1.0;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        let offset = source.find("x:").unwrap();
        for keyword in ["node", "param", "index", "true"] {
            assert!(
                rename(&analysis, &uri, offset, keyword).is_err(),
                "renaming to keyword `{keyword}` must be rejected"
            );
        }
    }

    #[test]
    fn rename_to_contextual_keyword_is_allowed() {
        let source = "param x: Dimensionless = 1.0;\nnode y: Dimensionless = @x;";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let offset = source.find("x:").unwrap();

        for name in ["range", "linspace", "step", "points", "Fin"] {
            let edit = rename(&analysis, &uri, offset, name)
                .unwrap_or_else(|error| panic!("rename to `{name}` failed: {error}"))
                .expect("rename should produce a workspace edit");
            assert_eq!(edit.changes.unwrap()[&uri].len(), 2);
        }
        // These lex as identifiers but are reserved built-in Term names.
        for name in ["scan", "unfold"] {
            assert_refused_by_compiler(rename(&analysis, &uri, offset, name), "graphcal::N009");
        }
    }

    #[test]
    fn rename_constructor_edits_all_occurrences() {
        // Regression: rename collected references by the single resolved key
        // while `references` expands alias keys (TopLevel↔Constructor), so
        // some occurrences were silently left un-renamed.
        let source = "\
pub type Status { Idle, Active }
param s: Status = Idle;
node t: Dimensionless = match @s { Idle => 1.0, Active => 2.0 };
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let original = crate::analysis_pipeline::run_analysis_for_test(&uri, source);
        assert!(
            original.has_no_diagnostics(),
            "fixture must check cleanly: {:?}",
            original.diagnostics
        );

        let offset = source.find("Idle,").unwrap();
        let result = rename(&analysis, &uri, offset, "Standby")
            .expect("constructor rename should be accepted")
            .expect("constructor rename should produce a workspace edit");
        let edits = result.changes.unwrap();
        let file_edits = &edits[&uri];

        // Exactly the definition, the initializer, and the match arm.
        let mut actual: Vec<_> = file_edits
            .iter()
            .map(|edit| {
                assert_eq!(edit.new_text, "Standby");
                (
                    crate::convert::position_to_byte_offset(source, edit.range.start),
                    crate::convert::position_to_byte_offset(source, edit.range.end),
                )
            })
            .collect();
        actual.sort_unstable();
        let expected: Vec<_> = [
            source.find("Idle,").unwrap(),
            source.find("= Idle").unwrap() + "= ".len(),
            source.find("Idle =>").unwrap(),
        ]
        .into_iter()
        .map(|start| (start, start + "Idle".len()))
        .collect();
        assert_eq!(actual, expected, "edits: {file_edits:?}");

        let updated = apply_edits(source, file_edits);
        assert!(!updated.contains("Idle"), "{updated}");
        let checked = crate::analysis_pipeline::run_analysis_for_test(&uri, &updated);
        assert!(
            checked.has_no_diagnostics(),
            "{updated}: {:?}",
            checked.diagnostics
        );
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
node kinetic: Energy = 0.5 * @mass * @velocity ^ 2;
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        let offset = source.find("momentum").unwrap();
        assert_refused_by_compiler(
            rename(&analysis, &uri, offset, "velocity"),
            "graphcal::N001",
        );
        // LSP-5: a built-in spelling is reserved (N009), so renaming to `PI`
        // is refused with the compiler's reason instead of producing an edit
        // that does not compile.
        assert_refused_by_compiler(rename(&analysis, &uri, offset, "PI"), "graphcal::N009");
    }

    /// LSP-5: the compiler's naming rules decide which renames are offered.
    /// Each case compiles before the rename and would fail after it.
    #[test]
    fn rename_refuses_names_the_compiler_rejects() {
        let uri = test_uri();
        for (source, needle, new_name, code) in [
            // Dimensions, indexes and types share one Static universe.
            (
                "dim Foo = Length;\nindex Bar = { A };\nnode q: Foo = 1.0 m;\n",
                "Foo",
                "Bar",
                "graphcal::N001",
            ),
            (
                "index Foo = { A };\ntype Bar { B }\nnode q: Dimensionless[Foo] = for f: Foo { 1.0 };\n",
                "Foo",
                "Bar",
                "graphcal::N001",
            ),
            // Declarations and constructors share the Term universe.
            (
                "type Status { Idle, Active }\nnode Busy: Dimensionless = 1.0;\nnode s: Status = Idle;\n",
                "Idle",
                "Busy",
                "graphcal::N001",
            ),
            (
                "dag d { node x: Dimensionless = 1.0; }\nnode e: Dimensionless = 2.0;\n",
                "e:",
                "d",
                "graphcal::N001",
            ),
            // A local may not shadow a visible term.
            (
                "type Status { Idle, Active }\nnode t: Status[Fin(2)] = for i: Fin(2) { Idle };\n",
                "i:",
                "Idle",
                "graphcal::N034",
            ),
            (
                "node total: Dimensionless = 1.0;\nnode t: Int[Fin(2)] = for i: Fin(2) { to_int(i) };\n",
                "i:",
                "total",
                "graphcal::N034",
            ),
            // Built-in spellings are reserved.
            (
                "node area: Dimensionless = 1.0;\n",
                "area",
                "sqrt",
                "graphcal::N009",
            ),
        ] {
            let analysis = analysis_from_source(source);
            assert!(
                analysis.has_no_diagnostics(),
                "{source}: {:?}",
                analysis.diagnostics
            );
            assert_refused_by_compiler(
                rename(&analysis, &uri, source.find(needle).unwrap(), new_name),
                code,
            );
        }
    }

    /// LSP-8: an attribute's `Index#Label` is an occurrence of both the
    /// index and its label, so renaming either keeps the program compiling.
    #[test]
    fn rename_updates_expected_fail_index_labels() {
        let source = "\
index Mode = { Normal, Eco, Boost };
node power_use: Dimensionless[Mode] = for m: Mode { 1.0 };
node power_gen: Dimensionless[Mode] = table[Mode] { Normal: 2.0; Eco: 2.0; Boost: 0.5; };
#[expected_fail(Mode#Boost)]
assert power_ok = for m: Mode { @power_use[m] < @power_gen[m] };
";
        let analysis = analysis_from_source(source);
        assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
        let uri = test_uri();
        for (needle, old, new) in [("Boost }", "Boost", "Surge"), ("Mode =", "Mode", "Regime")] {
            let edit = rename(&analysis, &uri, source.find(needle).unwrap(), new)
                .unwrap()
                .expect("renameable");
            let updated = apply_edits(source, &edit.changes.unwrap()[&uri]);
            assert!(!updated.contains(old), "{updated}");
            let checked = analysis_from_source(&updated);
            assert!(
                checked.has_no_diagnostics(),
                "{updated}: {:?}",
                checked.diagnostics
            );
        }
        let attribute_label = source.find("Mode#Boost").unwrap() + "Mode#".len();
        let references = crate::references::references(&analysis, &uri, attribute_label, true)
            .expect("label references");
        assert_eq!(references.len(), 3, "{references:?}");
    }

    /// Issue #829, scoped namespaces: a variant name only collides with a
    /// sibling variant of the same index, not with a same-named variant of
    /// another index.
    #[test]
    fn rename_variant_collision_is_scoped_to_its_index() {
        let source = "\
index Season = { Winter, Summer };
index Hemisphere = { North, Winter };
node pick: Season = Season#Summer;
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

        // `Summer` → `Winter` collides with its sibling.
        let offset = source.find("Summer }").unwrap();
        assert_refused_by_compiler(rename(&analysis, &uri, offset, "Winter"), "graphcal::N001");
        // `North` → `Summer` is fine: `Summer` only exists under `Season`.
        let offset = source.find("North").unwrap();
        assert!(
            rename(&analysis, &uri, offset, "Summer").is_ok_and(|edit| edit.is_some()),
            "same-named variant of a different index is not a collision"
        );
    }

    /// Issues #827/#828: renaming an index variant must edit exactly the
    /// variant identifier tokens — not table-axis-to-row-label merges and not
    /// whole `Index#Variant` qualified paths.
    #[test]
    fn rename_index_variant_edits_are_segment_precise() {
        let source = "\
pub index Maneuver = { Departure, Correction };
param dv: Velocity[Maneuver] = table[Maneuver] {
    Departure: 2.0 km/s;
    Correction: 0.1 km/s;
};
node total: Velocity = @dv[Maneuver#Departure];
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();

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
    fn rename_index_updates_multi_decl_axis_and_contextual_headers() {
        let source = "\
index Component = { A };
index Mode = { Safe, Nominal };
param scalar: Dimensionless[Component],
param enabled: Bool[Component, Mode]
    = table[Component, (_, Mode)] {
        : _, Safe, Nominal;
        A: 1.0, true, false;
    };
";
        let analysis = analysis_from_source(source);
        let uri = test_uri();
        let offset = source.find("Mode =").unwrap();
        let result = rename(&analysis, &uri, offset, "State").unwrap().unwrap();
        let file_edits = &result.changes.unwrap()[&uri];

        assert_eq!(
            file_edits.len(),
            source.match_indices("Mode").count(),
            "every type and slot-axis occurrence must be renamed: {file_edits:?}"
        );
        for edit in file_edits {
            let line = source.lines().nth(edit.range.start.line as usize).unwrap();
            assert_eq!(
                &line[edit.range.start.character as usize..edit.range.end.character as usize],
                "Mode"
            );
        }

        let safe_offset = source.find("Safe,").unwrap();
        let variant_result = rename(&analysis, &uri, safe_offset, "Active")
            .unwrap()
            .unwrap();
        let variant_edits = &variant_result.changes.unwrap()[&uri];
        assert_eq!(
            variant_edits.len(),
            source.match_indices("Safe").count(),
            "the declaration and contextual header variant must both be renamed"
        );
        for edit in variant_edits {
            let line = source.lines().nth(edit.range.start.line as usize).unwrap();
            assert_eq!(
                &line[edit.range.start.character as usize..edit.range.end.character as usize],
                "Safe"
            );
        }
    }

    #[test]
    fn is_valid_identifier_cases() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("velocity"));
        assert!(is_valid_identifier("my_var_2"));
        for contextual_keyword in [
            "scan", "unfold", "range", "linspace", "step", "points", "Fin",
        ] {
            assert!(is_valid_identifier(contextual_keyword));
        }
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

    /// A record type and its sole constructor share one spelling, so renaming
    /// either renames both; a union's constructor is renamed alone.
    #[test]
    fn rename_keeps_record_type_and_constructor_together() {
        let source = "\
type Item { Item(mass: Mass) }
type Mode { Mode, Other }
node item: Item = Item(mass: 1.0 kg);
node mass: Mass = @item.mass;
node mode: Mode = Mode;
";
        let analysis = analysis_from_source(source);
        assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
        let uri = test_uri();
        for (cursor, expected) in [
            (source.find("Item {").unwrap(), 4),
            (source.find("Item(mass").unwrap(), 4),
            (source.find("Mode {").unwrap(), 2),
        ] {
            let edit = rename(&analysis, &uri, cursor, "Renamed")
                .unwrap()
                .expect("renameable");
            let edits = &edit.changes.unwrap()[&uri];
            assert_eq!(edits.len(), expected, "{edits:?}");
            let updated = apply_edits(source, edits);
            assert!(
                analysis_from_source(&updated).has_no_diagnostics(),
                "{updated}"
            );
        }
    }

    /// The input name and value of an include's static binding
    /// (`index T: Step`) are occurrences of the template's port and of the
    /// bound index.
    #[test]
    fn rename_updates_include_static_bindings() {
        let source = "\
index Step = { A, B };
dag scaled {
    pub(bind) index T;
    pub node ys: Dimensionless[T] = for t: T { 1.0 };
}
include scaled(index T: Step)::{ ys };
";
        let analysis = analysis_from_source(source);
        assert!(analysis.has_no_diagnostics(), "{:?}", analysis.diagnostics);
        let uri = test_uri();
        for (cursor, expected) in [
            (source.find("Step =").unwrap(), 2),
            (source.find("index T;").unwrap() + "index ".len(), 4),
        ] {
            let edit = rename(&analysis, &uri, cursor, "Renamed")
                .unwrap()
                .expect("renameable");
            let edits = &edit.changes.unwrap()[&uri];
            assert_eq!(edits.len(), expected, "{edits:?}");
            let updated = apply_edits(source, edits);
            assert!(
                analysis_from_source(&updated).has_no_diagnostics(),
                "{updated}"
            );
        }
    }

    /// LSP-20: renaming one declaration of each kind in every standalone
    /// valid fixture to a fresh name is accepted, and the edited program
    /// still checks cleanly. A missed occurrence would make the re-check
    /// refuse the rename, so this exercises occurrence completeness.
    #[test]
    fn fresh_renames_of_valid_fixtures_are_accepted_and_compile() {
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/valid");
        let mut files: Vec<_> = std::fs::read_dir(&fixtures)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "gcl"))
            .collect();
        files.sort();
        let uri = test_uri();
        let mut renamed = 0usize;
        let mut failures = Vec::new();
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let analysis = analysis_from_source(&source);
            if !analysis.has_no_diagnostics() {
                continue;
            }
            let mut seen = std::collections::HashSet::new();
            let mut definitions: Vec<_> = analysis
                .symbol_table
                .definitions
                .iter()
                .filter(|(key, definition)| {
                    definition.is_navigable() && !matches!(key, SymbolKey::ExternFunction(_))
                })
                .map(|(_, definition)| definition)
                .collect();
            definitions.sort_by_key(|definition| definition.name_span.offset());
            for definition in definitions {
                if !seen.insert(std::mem::discriminant(&definition.category)) {
                    continue;
                }
                let new_name = format!("{}_renamed", definition.name);
                match rename(&analysis, &uri, definition.name_span.offset(), &new_name) {
                    Ok(Some(edit)) => {
                        let updated = apply_edits(&source, &edit.changes.unwrap()[&uri]);
                        let checked = analysis_from_source(&updated);
                        if checked.has_no_diagnostics() {
                            renamed += 1;
                        } else {
                            failures.push(format!(
                                "{}: `{}` -> `{new_name}` broke: {:?}",
                                path.display(),
                                definition.name,
                                checked.diagnostics
                            ));
                        }
                    }
                    // A field of a record a plugin returns is part of the
                    // plugin's signature, which no rename can change.
                    Err(RenameRefusal::WouldNotCompile { reason, .. })
                        if reason.starts_with("graphcal::W005") => {}
                    other => failures.push(format!(
                        "{}: `{}` -> `{new_name}`: {other:?}",
                        path.display(),
                        definition.name
                    )),
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(renamed > 100, "only {renamed} renames were exercised");
    }
}

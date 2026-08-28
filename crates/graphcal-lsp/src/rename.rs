//! textDocument/rename and textDocument/prepareRename handlers.

use std::collections::HashMap;

use tower_lsp::lsp_types::{PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::convert::LineIndex;
use crate::resolve::{ResolvedSymbol, SymbolLocation, reference_lookup_keys, resolve_symbol_at};
use crate::server::AnalysisResult;
use crate::symbol_identity::{ExternFunctionId, FieldId, GenericParamId, IndexVariantId};
use crate::symbol_table::SymbolKey;
use graphcal_compiler::syntax::decl_name::{DeclName, ResolvedDeclName};
use graphcal_compiler::syntax::dimension::{DimName, ResolvedDimName, ResolvedUnitName, UnitName};
use graphcal_compiler::syntax::function_name::FnName;
use graphcal_compiler::syntax::index_name::{IndexName, IndexVariantName, ResolvedIndexName};
use graphcal_compiler::syntax::type_name::{
    ConstructorName, FieldName, GenericParamName, ResolvedConstructorName, ResolvedStructTypeName,
    StructTypeName,
};

/// Check whether a name is a valid Graphcal identifier.
///
/// Asks the lexer instead of a hand-kept rule so hard keywords (`node`,
/// `param`, `true`, …) are rejected while contextual keyword tokens remain
/// valid identifiers.
fn is_valid_identifier(name: &str) -> bool {
    use graphcal_compiler::syntax::lexer::Lexer;
    let mut lexer = Lexer::new(name);
    let is_single_ident = matches!(
        lexer.next_token(),
        Some((token, span)) if token.is_identifier() && span.len() == name.len()
    );
    is_single_ident && lexer.next_token().is_none()
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

    if let Some(project) = analysis.project_symbols.complete() {
        if matches!(resolved.location, SymbolLocation::Local(_))
            && analysis.symbol_table.is_externally_visible(&resolved.key)
            && !project.covers_reverse_dependencies()
        {
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
pub fn prepare_rename(
    analysis: &AnalysisResult,
    uri: &Url,
    offset: usize,
) -> Option<PrepareRenameResponse> {
    let resolved = resolve_symbol_at(analysis, offset)?;
    if !validate_rename_target(analysis, uri, &resolved).unwrap_or(false) {
        return None;
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
    /// The new name is not a lexable identifier (or is a hard keyword).
    InvalidIdentifier { new_name: String },
    /// The new name collides with a visible declaration in the same
    /// namespace, which would compile to a duplicate-name error (N001).
    NameCollision { new_name: String },
    /// Transitive project loading did not produce a complete occurrence set.
    IncompleteProjectIndex { name: String },
    /// A tolerant occurrence has the right namespace/spelling but no unique ID.
    AmbiguousOccurrence { name: String },
    /// The cursor is on a preserved local alias, not the canonical API leaf.
    ImportAlias { alias: String },
    /// An affected open document changed after the project index was built.
    StaleProjectSnapshot,
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
        }
    }
}

/// The key the renamed symbol would occupy: the same key shape with the
/// leaf name replaced. Used to probe the symbol table for collisions in
/// the renamed symbol's own namespace/scope.
fn key_with_new_name(key: &SymbolKey, new_name: &str) -> Option<SymbolKey> {
    Some(match key {
        SymbolKey::Declaration(name) => SymbolKey::Declaration(ResolvedDeclName::from_def(
            name.owner().clone(),
            DeclName::expect_valid(new_name),
        )),
        SymbolKey::Dimension(name) => SymbolKey::Dimension(ResolvedDimName::from_def(
            name.owner().clone(),
            DimName::expect_valid(new_name),
        )),
        SymbolKey::Unit(name) => SymbolKey::Unit(ResolvedUnitName::from_def(
            name.owner().clone(),
            UnitName::expect_valid(new_name),
        )),
        SymbolKey::StructType(name) => SymbolKey::StructType(ResolvedStructTypeName::from_def(
            name.owner().clone(),
            StructTypeName::expect_valid(new_name),
        )),
        SymbolKey::Constructor(name) => SymbolKey::Constructor(ResolvedConstructorName::from_def(
            name.owner().clone(),
            ConstructorName::expect_valid(new_name),
        )),
        SymbolKey::Index(name) => SymbolKey::Index(ResolvedIndexName::from_def(
            name.owner().clone(),
            IndexName::expect_valid(new_name),
        )),
        SymbolKey::IndexVariant(variant) => SymbolKey::IndexVariant(IndexVariantId::new(
            variant.index().clone(),
            IndexVariantName::expect_valid(new_name),
        )),
        SymbolKey::Field(field) => SymbolKey::Field(FieldId::new(
            field.owner().clone(),
            FieldName::expect_valid(new_name),
        )),
        SymbolKey::GenericParam(parameter) => SymbolKey::GenericParam(GenericParamId::new(
            parameter.owner().clone(),
            GenericParamName::expect_valid(new_name),
        )),
        SymbolKey::ExternFunction(function) => SymbolKey::ExternFunction(ExternFunctionId::new(
            function.owner().clone(),
            function.plugin().clone(),
            FnName::expect_valid(new_name),
        )),
        SymbolKey::Local(_)
        | SymbolKey::BuiltinFunction(_)
        | SymbolKey::BuiltinConstant(_)
        | SymbolKey::TimeScale(_) => return None,
    })
}

/// True when `new_name` collides with a visible declaration in the renamed
/// symbol's namespace/scope. Builtins (`PI`, `sqrt`, unit names) are not
/// collisions: the compiler allows shadowing them.
fn collides_with_existing(analysis: &AnalysisResult, key: &SymbolKey, new_name: &str) -> bool {
    let Some(candidate) = key_with_new_name(key, new_name) else {
        return false;
    };
    if let Some(existing) = analysis.symbol_table.definitions.get(&candidate)
        && !existing.is_builtin()
    {
        return true;
    }
    let top_level_namespace = matches!(
        candidate,
        SymbolKey::Declaration(_)
            | SymbolKey::Dimension(_)
            | SymbolKey::Unit(_)
            | SymbolKey::StructType(_)
            | SymbolKey::Constructor(_)
            | SymbolKey::Index(_)
    );
    top_level_namespace
        && analysis.imported_bindings.iter().any(|binding| {
            binding.spelling().is_local()
                && binding.spelling().leaf().as_str() == new_name
                && candidate.same_namespace(binding.target())
        })
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
    if !validate_rename_target(analysis, uri, &resolved)? {
        return Ok(None);
    }
    let definition = resolved_definition(&resolved);
    // Renaming to the symbol's current name is a no-op, not a collision.
    if definition.name == new_name {
        return Ok(None);
    }
    let renamed_target = key_with_new_name(&resolved.key, new_name);
    let collides = match (analysis.project_symbols.complete(), renamed_target.as_ref()) {
        (Some(project), Some(renamed_target)) => {
            project.collides_with(&resolved.key, renamed_target, new_name)
        }
        _ => collides_with_existing(analysis, &resolved.key, new_name),
    };
    if collides {
        return Err(RenameRefusal::NameCollision {
            new_name: new_name.to_string(),
        });
    }

    let changes: HashMap<Url, Vec<TextEdit>> =
        if let Some(project) = analysis.project_symbols.complete() {
            let mut changes = HashMap::new();
            for occurrence in reference_lookup_keys(&resolved.key)
                .iter()
                .flat_map(|target| project.rename_occurrences(target))
            {
                let Some(document) = project.document(&occurrence.uri) else {
                    return Err(RenameRefusal::IncompleteProjectIndex {
                        name: definition.name.clone(),
                    });
                };
                changes
                    .entry(occurrence.uri)
                    .or_insert_with(Vec::new)
                    .push(TextEdit {
                        range: LineIndex::new(&document.source).span_to_range(occurrence.span),
                        new_text: new_name.to_string(),
                    });
            }
            changes
        } else {
            let lines = LineIndex::new(&analysis.source);
            let mut spans: Vec<_> = reference_lookup_keys(&resolved.key)
                .iter()
                .flat_map(|key| analysis.symbol_table.find_all_references(key))
                .map(|reference| reference.span)
                .chain((!definition.name_span.is_empty()).then_some(definition.name_span))
                .collect();
            let mut seen = std::collections::HashSet::new();
            spans.retain(|span| seen.insert((span.offset(), span.len())));
            HashMap::from([(
                uri.clone(),
                spans
                    .into_iter()
                    .map(|span| TextEdit {
                        range: lines.span_to_range(span),
                        new_text: new_name.to_string(),
                    })
                    .collect(),
            )])
        };

    if changes.values().all(Vec::is_empty) {
        return Ok(None);
    }

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
        let project_table = symbol_table::build_for_buffer(&ast, source);
        let uri = Url::parse("file:///test.gcl").unwrap();
        let source = Arc::new(source.to_string());
        let project_symbols = crate::project_symbols::ProjectSymbols::Complete(
            crate::project_symbols::ProjectSymbolIndex::standalone(
                uri.clone(),
                [crate::project_symbols::ProjectDocumentSymbols::new(
                    uri,
                    Arc::clone(&source),
                    project_table,
                    Vec::new(),
                )],
            ),
        );
        AnalysisResult {
            inputs: crate::workspace_revision::AnalysisInputs::untracked_for_test(
                crate::workspace_revision::DocumentIdentity::virtual_uri(
                    Url::parse("file:///test.gcl").unwrap(),
                ),
            ),
            source,
            symbol_table,
            project_symbols,
            imported_definitions: HashMap::new(),
            imported_bindings: Vec::new(),
            import_surfaces: HashMap::new(),
            diagnostics: Arc::new(HashMap::new()),
            eval_values: HashMap::new(),
            fn_signatures: build_fn_signatures(),
            extern_fn_signatures: HashMap::new(),
            import_links: Vec::new(),
            buffer_parsed: true,
        }
    }

    fn apply_edits(source: &str, edits: &[TextEdit]) -> String {
        let mut edits: Vec<_> = edits
            .iter()
            .map(|edit| {
                (
                    crate::convert::position_to_byte_offset(source, edit.range.start),
                    crate::convert::position_to_byte_offset(source, edit.range.end),
                    edit.new_text.as_str(),
                )
            })
            .collect();
        edits.sort_unstable_by_key(|edit| std::cmp::Reverse(edit.0));
        edits.into_iter().fold(
            source.to_string(),
            |mut output, (start, end, replacement)| {
                output.replace_range(start..end, replacement);
                output
            },
        )
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
        let uri = Url::parse("file:///test.gcl").unwrap();
        let cursor = source.find("prev_y").unwrap();
        let edit = rename(&analysis, &uri, cursor, "previous")
            .unwrap()
            .expect("expression local should be renameable");

        assert_eq!(edit.changes.unwrap()[&uri].len(), 2);
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
        let uri = Url::parse("file:///test.gcl").unwrap();
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
        let uri = Url::parse("file:///test.gcl").unwrap();

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
        let uri = Url::parse("file:///test.gcl").unwrap();

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
    fn rename_generic_parameter_edits_defaults_and_payload_types() {
        let source = "type Sized<N: Nat, M: Nat = N + 1> {\n\
                      Sized(values: Dimensionless[Fin(N)]),\n\
                      }";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();
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
        let source = "node lower: Dimensionless = least(1.0, 2.0);\n\
                      node reduced: Dimensionless = minimum(1.0);";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();

        for builtin in ["least", "minimum"] {
            let offset = source.find(builtin).unwrap();
            let result = prepare_rename(&analysis, &uri, offset);
            assert!(
                result.is_none(),
                "builtin `{builtin}` should not be renameable"
            );
        }
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
    fn rename_imported_symbol_edits_definition_import_and_use() {
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
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(lib_path.canonicalize().unwrap()).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected the import to load cleanly, got: {:?}",
            analysis.diagnostics,
        );

        let defining_analysis = crate::server::run_analysis_for_test(&lib_uri, lib_text);
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
        assert!(prepare_rename(&analysis, &main_uri, cursor).is_some());
        let changes = rename(&analysis, &main_uri, cursor, "renamed")
            .unwrap()
            .unwrap()
            .changes
            .unwrap();
        assert_eq!(changes[&main_uri].len(), 2);
        assert_eq!(changes[&lib_uri].len(), 1);

        let updated_main = apply_edits(main_text, &changes[&main_uri]);
        let updated_lib = apply_edits(lib_text, &changes[&lib_uri]);
        std::fs::write(&lib_path, updated_lib).unwrap();
        std::fs::write(&main_path, &updated_main).unwrap();
        let updated = crate::server::run_analysis_for_test(&main_uri, &updated_main);
        assert!(
            updated.has_no_diagnostics(),
            "renamed project should compile: {:?}",
            updated.diagnostics,
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
        let analysis = crate::server::run_analysis_for_test(&main_uri, &main_text);
        assert!(
            analysis.has_no_diagnostics(),
            "expected clean include project: {:?}",
            analysis.diagnostics,
        );

        let cursor = main_text.find("@doubled").unwrap() + 1;
        let changes = rename(&analysis, &main_uri, cursor, "tripled")
            .unwrap()
            .unwrap()
            .changes
            .unwrap();
        assert_eq!(changes.values().map(Vec::len).sum::<usize>(), 4);
        for (uri, edits) in &changes {
            let path = uri.to_file_path().unwrap();
            let source = std::fs::read_to_string(&path).unwrap();
            std::fs::write(path, apply_edits(&source, edits)).unwrap();
        }
        let updated_main = std::fs::read_to_string(&main_path).unwrap();
        let updated_consumer = std::fs::read_to_string(source_dir.join("consumer.gcl")).unwrap();
        assert!(updated_consumer.contains("tripled as local"));
        assert!(updated_consumer.contains("@local"));
        let updated = crate::server::run_analysis_for_test(&main_uri, &updated_main);
        assert!(
            updated.has_no_diagnostics(),
            "renamed include project should compile: {:?}",
            updated.diagnostics,
        );
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
                "import helper.lib::{ y };\nimport helper.a::{ y as through_a, a_value };\nimport helper.b::{ b_value };\nimport helper.other as other;\nnode total: Dimensionless = @y + @through_a + @a_value + @b_value + @other.y;\n",
            ),
        ];
        for (name, source) in files {
            std::fs::write(source_dir.join(name), source).unwrap();
        }

        let main_path = source_dir.join("main.gcl");
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let main_text = std::fs::read_to_string(&main_path).unwrap();
        let analysis = crate::server::run_analysis_for_test(&main_uri, &main_text);
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
        assert!(prepare_rename(&analysis, &main_uri, alias_cursor).is_none());
        assert!(matches!(
            rename(&analysis, &main_uri, alias_cursor, "surprise"),
            Err(RenameRefusal::ImportAlias { .. })
        ));
        assert_eq!(
            rename(&analysis, &main_uri, cursor, "occupied"),
            Err(RenameRefusal::NameCollision {
                new_name: "occupied".to_string()
            }),
            "every direct-import scope must be collision checked"
        );

        let changes = rename(&analysis, &main_uri, cursor, "renamed")
            .unwrap()
            .unwrap()
            .changes
            .unwrap();
        assert_eq!(
            changes.values().map(Vec::len).sum::<usize>(),
            7,
            "{changes:#?}"
        );
        assert_eq!(
            changes.len(),
            4,
            "the same-leaf other module stays untouched"
        );

        for (uri, edits) in &changes {
            let path = uri.to_file_path().unwrap();
            let source = std::fs::read_to_string(&path).unwrap();
            std::fs::write(path, apply_edits(&source, edits)).unwrap();
        }
        let updated_main = std::fs::read_to_string(&main_path).unwrap();
        assert!(updated_main.contains("renamed as through_a"));
        assert!(updated_main.contains("+ @through_a"));
        assert!(updated_main.contains("@other.y"));
        let updated = crate::server::run_analysis_for_test(&main_uri, &updated_main);
        assert!(
            updated.has_no_diagnostics(),
            "renamed project should compile: {:?}",
            updated.diagnostics,
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
        let uri = Url::parse("file:///test.gcl").unwrap();
        let offset = source.find("x:").unwrap();

        for name in [
            "scan", "unfold", "range", "linspace", "step", "points", "Fin",
        ] {
            let edit = rename(&analysis, &uri, offset, name)
                .unwrap_or_else(|error| panic!("rename to `{name}` failed: {error}"))
                .expect("rename should produce a workspace edit");
            assert_eq!(edit.changes.unwrap()[&uri].len(), 2);
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
node kinetic: Energy = 0.5 * @mass * @velocity ^ 2;
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
node pick: Season = Season#Summer;
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
    fn rename_index_updates_multi_decl_axis_and_qualified_headers() {
        let source = "\
index Component = { A };
index Mode = { Safe, Nominal };
param scalar: Dimensionless[Component],
param enabled: Bool[Component, Mode]
    = table[Component, (_, Mode)] {
        : _, Mode#Safe, Mode#Nominal;
        A: 1.0, true, false;
    };
";
        let analysis = analysis_from_source(source);
        let uri = Url::parse("file:///test.gcl").unwrap();
        let offset = source.find("Mode =").unwrap();
        let result = rename(&analysis, &uri, offset, "State").unwrap().unwrap();
        let file_edits = &result.changes.unwrap()[&uri];

        assert_eq!(
            file_edits.len(),
            source.match_indices("Mode").count(),
            "every type, slot-axis, and header-axis occurrence must be renamed: {file_edits:?}"
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
            "the declaration and qualified header variant must both be renamed"
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
}

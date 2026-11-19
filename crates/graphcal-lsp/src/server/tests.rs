use super::analysis_worker::store_analysis;
use super::client_shape::{workspace_edit_for_client, workspace_edit_matches_open_snapshots};
use super::documents::{
    OpenDocumentSnapshot, StoredDocument, is_current_analysis, is_semantic_analysis,
    merged_diagnostics_for,
};

use crate::analysis_pipeline::{run_analysis, run_analysis_for_test, test_plugin_host};
use crate::fn_signatures::build_fn_signatures;
use crate::project_symbols::{ProjectDocumentSymbols, ProjectSymbols};
use crate::symbol_table::SymbolTable;
use crate::workspace_revision::AnalysisInputs;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tower_lsp::lsp_types::{Diagnostic, DocumentChanges, TextEdit, Url, WorkspaceEdit};

use crate::analysis::AnalysisResult;
use crate::client_capabilities::WorkspaceEditShape;
use crate::file_identity::document_identity;
use crate::project_symbols::ProjectSymbolIndex;
use crate::workspace_revision::{
    AnalysisInputSnapshot, CurrentRevisions, DocumentIdentity, RevisionClock,
};

use crate::analysis_pipeline::untitled_uri;

#[test]
fn analysis_access_separates_exact_coordinates_from_stale_root_candidates() {
    let uri = Url::parse("file:///tmp/stale-position.gcl").unwrap();
    let root = document_identity(&uri);
    let dependency = DocumentIdentity::file(std::path::PathBuf::from("/tmp/lib.gcl"));
    let clock = RevisionClock::default();
    let root_revision = clock.next().unwrap();
    let dependency_revision = clock.next().unwrap();
    let snapshot = AnalysisInputSnapshot::new(
        root.clone(),
        root_revision,
        HashMap::from([(dependency.clone(), dependency_revision)]),
    );
    let mut analysis = run_analysis_for_test(
        &uri,
        "const node first: Dimensionless = 1.0;\r\n\
             node second: Dimensionless = @first;\r\n",
    );
    analysis.inputs = snapshot.finish_loaded_project(HashSet::from([dependency.clone()]));

    let current = HashMap::from([
        (root.clone(), root_revision),
        (dependency.clone(), dependency_revision),
    ]);
    assert!(is_current_analysis(
        &analysis,
        &CurrentRevisions::new(&current, [])
    ));
    assert!(is_semantic_analysis(
        &analysis,
        &CurrentRevisions::new(&current, [])
    ));

    // This revision represents a current buffer with deleted CRLF text and
    // an inserted astral character (a two-code-unit UTF-16 shift).
    let shifted_root = HashMap::from([
        (root.clone(), clock.next().unwrap()),
        (dependency.clone(), dependency_revision),
    ]);
    assert!(!is_current_analysis(
        &analysis,
        &CurrentRevisions::new(&shifted_root, [])
    ));
    assert!(is_semantic_analysis(
        &analysis,
        &CurrentRevisions::new(&shifted_root, [])
    ));

    let changed_dependency =
        HashMap::from([(root, root_revision), (dependency, clock.next().unwrap())]);
    assert!(!is_current_analysis(
        &analysis,
        &CurrentRevisions::new(&changed_dependency, [])
    ));
    assert!(!is_semantic_analysis(
        &analysis,
        &CurrentRevisions::new(&changed_dependency, [])
    ));
}

#[test]
fn workspace_rename_refuses_stale_open_project_document() {
    let uri = Url::parse("file:///open.gcl").unwrap();
    let project = ProjectSymbolIndex::loaded_dependency_closure(
        uri.clone(),
        [ProjectDocumentSymbols::new(
            uri.clone(),
            Arc::new("old".to_string()),
            SymbolTable::default(),
            Vec::new(),
        )],
    );
    let edit = WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), Vec::new())])),
        document_changes: None,
        change_annotations: None,
    };
    let revision = RevisionClock::default().next().unwrap();
    let identity = document_identity(&uri);
    let snapshots = HashMap::from([(
        uri,
        OpenDocumentSnapshot {
            identity,
            revision,
            text: Arc::new("new".to_string()),
            version: 2,
        },
    )]);

    assert!(!workspace_edit_matches_open_snapshots(
        &edit, &project, &snapshots
    ));
}

#[test]
fn workspace_rename_edits_carry_open_document_versions() {
    let open_uri = Url::parse("file:///open.gcl").unwrap();
    let closed_uri = Url::parse("file:///closed.gcl").unwrap();
    let edit = WorkspaceEdit {
        changes: Some(HashMap::from([
            (
                open_uri.clone(),
                vec![TextEdit {
                    range: tower_lsp::lsp_types::Range::default(),
                    new_text: "renamed".to_string(),
                }],
            ),
            (
                closed_uri.clone(),
                vec![TextEdit {
                    range: tower_lsp::lsp_types::Range::default(),
                    new_text: "renamed".to_string(),
                }],
            ),
        ])),
        document_changes: None,
        change_annotations: None,
    };
    let revision = RevisionClock::default().next().unwrap();
    let snapshots = HashMap::from([(
        open_uri.clone(),
        OpenDocumentSnapshot {
            identity: document_identity(&open_uri),
            revision,
            text: Arc::new(String::new()),
            version: 7,
        },
    )]);

    let versioned =
        workspace_edit_for_client(edit, &snapshots, WorkspaceEditShape::DocumentChanges);
    assert!(versioned.changes.is_none());
    let Some(DocumentChanges::Edits(documents)) = versioned.document_changes else {
        panic!("expected versioned document edits");
    };
    assert_eq!(documents.len(), 2);
    let versions: HashMap<_, _> = documents
        .into_iter()
        .map(|document| (document.text_document.uri, document.text_document.version))
        .collect();
    assert_eq!(versions[&open_uri], Some(7));
    assert_eq!(versions[&closed_uri], None);
}

#[test]
fn legacy_workspace_edits_keep_changes_map() {
    let uri = Url::parse("file:///open.gcl").unwrap();
    let edit = WorkspaceEdit {
        changes: Some(HashMap::from([(uri, Vec::new())])),
        ..Default::default()
    };

    let shaped = workspace_edit_for_client(edit, &HashMap::new(), WorkspaceEditShape::Changes);
    assert!(shaped.changes.is_some());
    assert!(shaped.document_changes.is_none());
}

/// Build a bare `AnalysisResult` carrying only a diagnostics map, for
/// exercising the per-URI ownership merge.
fn analysis_with_diags(diags: HashMap<Url, Vec<Diagnostic>>) -> StoredDocument {
    StoredDocument::new(AnalysisResult {
        inputs: AnalysisInputs::untracked_for_test(DocumentIdentity::virtual_uri(
            Url::parse("file:///diagnostics.gcl").unwrap(),
        )),
        source: Arc::new(String::new()),
        symbol_table: SymbolTable::default(),
        project_symbols: ProjectSymbols::Incomplete,
        imported_definitions: HashMap::new(),
        imported_bindings: Vec::new(),
        import_surfaces: HashMap::new(),
        diagnostics: Arc::new(diags),
        eval_values: HashMap::new(),
        view: None,
        fn_signatures: build_fn_signatures(),
        extern_fn_signatures: HashMap::new(),
        import_links: Vec::new(),
        buffer_parsed: true,
        check: crate::analysis::ProjectCheck::Passed,
    })
}

fn diag(message: &str) -> Diagnostic {
    Diagnostic {
        message: message.to_string(),
        ..Default::default()
    }
}

/// Issue #832: an open document's own analysis is authoritative for its
/// own URI — an importer's (possibly stale) contribution must not
/// overwrite or clear it.
#[test]
fn open_document_owns_its_own_diagnostics() {
    let lib_uri = Url::parse("file:///p/lib.gcl").unwrap();
    let main_uri = Url::parse("file:///p/main.gcl").unwrap();

    let mut docs = HashMap::new();
    // lib's own analysis reports its own error.
    docs.insert(
        lib_uri.clone(),
        analysis_with_diags(HashMap::from([(lib_uri.clone(), vec![diag("lib broken")])])),
    );
    // main's analysis reports nothing for lib (e.g. computed before
    // lib's latest edit).
    docs.insert(
        main_uri.clone(),
        analysis_with_diags(HashMap::from([
            (main_uri, vec![]),
            (lib_uri.clone(), vec![]),
        ])),
    );

    let published = merged_diagnostics_for(&docs, std::slice::from_ref(&lib_uri));
    assert_eq!(published.len(), 1);
    assert_eq!(
        published[0].1,
        vec![diag("lib broken")],
        "lib's own analysis must win for lib's URI"
    );
}

/// Issue #832 repro: after closing the importer, a still-open imported
/// document keeps its own diagnostics instead of having them wiped.
#[test]
fn closing_importer_keeps_open_imported_documents_diagnostics() {
    let lib_uri = Url::parse("file:///p/lib.gcl").unwrap();
    let main_uri = Url::parse("file:///p/main.gcl").unwrap();

    let mut docs = HashMap::new();
    docs.insert(
        lib_uri.clone(),
        analysis_with_diags(HashMap::from([(lib_uri.clone(), vec![diag("lib broken")])])),
    );
    // The importer was just closed: only lib remains open. The closing
    // path re-publishes every URI from the remaining documents' view.
    let affected = vec![lib_uri.clone(), main_uri.clone()];
    let published = merged_diagnostics_for(&docs, &affected);
    let by_uri: HashMap<_, _> = published.into_iter().collect();
    assert_eq!(
        by_uri[&lib_uri],
        vec![diag("lib broken")],
        "still-open lib keeps its diagnostics"
    );
    assert!(
        by_uri[&main_uri].is_empty(),
        "the closed importer's URI clears"
    );
}

/// Diagnostics for a file that is not open merge across importers,
/// deduplicating identical reports.
#[test]
fn unopened_uri_merges_and_dedups_contributions() {
    let disk_uri = Url::parse("file:///p/disk.gcl").unwrap();
    let a_uri = Url::parse("file:///p/a.gcl").unwrap();
    let b_uri = Url::parse("file:///p/b.gcl").unwrap();

    let mut docs = HashMap::new();
    docs.insert(
        a_uri,
        analysis_with_diags(HashMap::from([(disk_uri.clone(), vec![diag("broken")])])),
    );
    docs.insert(
        b_uri,
        analysis_with_diags(HashMap::from([(
            disk_uri.clone(),
            vec![diag("broken"), diag("also broken")],
        )])),
    );

    let published = merged_diagnostics_for(&docs, std::slice::from_ref(&disk_uri));
    let mut messages: Vec<_> = published[0].1.iter().map(|d| d.message.clone()).collect();
    messages.sort();
    assert_eq!(messages, vec!["also broken", "broken"]);
}

/// Issue #834: a parse-failure analysis (the normal state mid-keystroke)
/// must not replace the cached symbol state with an empty table —
/// completion and signature help keep answering from the last good
/// analysis while only the diagnostics refresh.
#[test]
fn parse_failure_keeps_last_good_symbol_state() {
    let uri = untitled_uri();
    let good_text = "\
param mass: Mass = 100.0 kg;
param velocity: Velocity = 50.0 m/s;
node momentum: Force * Time = @mass * @velocity;
";
    let good = run_analysis(&uri, good_text, &[], test_plugin_host());
    assert!(good.buffer_parsed);
    assert!(good.has_no_diagnostics());

    // Mid-edit: the buffer no longer parses.
    let broken_text = format!("{good_text}node next: Mass = @");
    let broken = run_analysis(&uri, &broken_text, &[], test_plugin_host());
    assert!(!broken.buffer_parsed, "broken buffer must be flagged");
    assert!(
        !broken.has_no_diagnostics(),
        "the parse error must still be reported"
    );

    let mut docs = HashMap::new();
    store_analysis(&mut docs, &uri, good);
    store_analysis(&mut docs, &uri, broken);

    let stored = &docs[&uri];
    let cached = &stored.analysis;
    for name in ["mass", "velocity", "momentum"] {
        assert!(
            cached
                .symbol_table
                .definitions
                .values()
                .any(|definition| definition.name == name),
            "symbol `{name}` must survive the parse failure"
        );
    }
    assert!(
        stored
            .diagnostics
            .values()
            .any(|diagnostics| !diagnostics.is_empty()),
        "diagnostics must come from the failed analysis"
    );
    assert_eq!(
        cached.source.as_str(),
        good_text,
        "the kept source must stay consistent with the kept symbol table"
    );
}

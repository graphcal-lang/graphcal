//! LSP server backend: state management and `LanguageServer` trait implementation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic,
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, DocumentLink, DocumentLinkOptions, DocumentLinkParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InlayHint, InlayHintParams, Location, MessageType, OneOf,
    PrepareRenameResponse, ReferenceParams, RenameOptions, RenameParams, SaveOptions,
    ServerCapabilities, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgressOptions,
    WorkspaceEdit,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use graphcal_eval::builtins::{DimSignature, builtin_functions};
use graphcal_eval::eval::{
    EvalResult, Value, compile_and_eval_named, compile_and_eval_project, compile_to_tir,
    compile_to_tir_project,
};
use graphcal_syntax::ast::DeclKind;

use crate::convert::position_to_byte_offset;
use crate::diagnostics::{compile_error_to_diagnostics, eval_result_to_diagnostics};
use crate::symbol_table::{self, DefinitionInfo, SymbolTable};

/// A definition from an imported file, for cross-file go-to-definition and hover.
pub struct ImportedDefinition {
    /// URI of the file containing the definition.
    pub uri: Url,
    /// Source text of the imported file (needed for span-to-range conversion).
    pub source: String,
    /// The definition info (name, category, spans, type description).
    pub definition: DefinitionInfo,
}

/// Info about a `use` declaration for Document Links.
pub struct UseDeclInfo {
    /// The raw path string (e.g., `"./constants.gcl"`).
    pub path: String,
    /// Span of the path literal in the source.
    pub path_span: graphcal_syntax::span::Span,
}

/// Structured function signature for Signature Help.
pub struct FnSignatureInfo {
    /// Full signature label, e.g. `"fn sqrt(x: D^2) -> D"`.
    pub label: String,
    /// Individual parameter labels, e.g. `["x: D^2"]`.
    pub parameters: Vec<String>,
}

/// Cached analysis result for a document.
pub struct AnalysisResult {
    /// The raw source text.
    pub source: String,
    /// The symbol table (built from AST, enriched from TIR if available).
    pub symbol_table: SymbolTable,
    /// Definitions from imported files, keyed by symbol name.
    pub imported_definitions: HashMap<String, ImportedDefinition>,
    /// Diagnostics to publish.
    pub diagnostics: Vec<Diagnostic>,
    /// Computed values from evaluation, keyed by declaration name.
    /// Each value is a formatted display string (e.g., `"9.81 [m/s^2]"`).
    pub eval_values: HashMap<String, String>,
    /// Structured function signatures, keyed by function name.
    pub fn_signatures: HashMap<String, FnSignatureInfo>,
    /// Use declarations in this file (for Document Links).
    pub use_decls: Vec<UseDeclInfo>,
}

/// The LSP server backend.
#[derive(Debug)]
pub struct Backend {
    client: Client,
    /// Per-document analysis results, keyed by URI.
    documents: Arc<RwLock<HashMap<Url, AnalysisResult>>>,
}

impl std::fmt::Debug for AnalysisResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisResult")
            .field("source_len", &self.source.len())
            .field("symbol_table_defs", &self.symbol_table.definitions.len())
            .field("imported_defs", &self.imported_definitions.len())
            .field("diagnostics_count", &self.diagnostics.len())
            .field("eval_values_count", &self.eval_values.len())
            .field("fn_signatures_count", &self.fn_signatures.len())
            .field("use_decls_count", &self.use_decls.len())
            .finish()
    }
}

impl Backend {
    fn is_graphcal_file(uri: &Url) -> bool {
        std::path::Path::new(uri.path())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gcl"))
    }

    async fn analyze_and_publish(&self, uri: Url, text: String) {
        if !Self::is_graphcal_file(&uri) {
            return;
        }

        let analysis = run_analysis(&uri, &text);

        let diagnostics = analysis.diagnostics.clone();
        self.documents.write().await.insert(uri.clone(), analysis);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;

        // Ask the client to re-fetch inlay hints now that analysis is complete.
        // Inlay hints are pull-based (client requests them), so without this
        // refresh notification the client may show stale or missing hints.
        let _ = self.client.inlay_hint_refresh().await;
    }
}

/// Run the analysis pipeline, producing an `AnalysisResult`.
fn run_analysis(uri: &Url, text: &str) -> AnalysisResult {
    let name = uri.as_str();

    // Try to parse and compile to TIR.
    let tir_result = uri.to_file_path().map_or_else(
        |()| compile_to_tir(text, name).map(|tir| (tir, None)),
        |path| compile_to_tir_project(&path).map(|(tir, project)| (tir, Some(project))),
    );

    // Always parse the in-memory text first for the symbol table.
    // This may differ from the on-disk version when the user has unsaved edits.
    let parse_result = graphcal_syntax::parser::Parser::with_name(text, name).parse_file();

    match (tir_result, parse_result) {
        (Ok((tir, project)), Ok(ast)) => {
            // Both TIR and in-memory parse succeeded.
            let mut symbol_table = symbol_table::build_from_ast(&ast);
            symbol_table::enrich_from_tir(&mut symbol_table, &tir);

            let imported_definitions = project.map_or_else(HashMap::new, |project| {
                collect_imported_definitions(uri, &ast, &project, Some(&tir))
            });

            let fn_signatures = build_fn_signatures(Some(&tir));
            let use_decls = collect_use_decl_info(&ast);

            // Run full evaluation for diagnostics and computed values.
            let (diagnostics, eval_values) = run_eval(uri, text, name);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values,
                fn_signatures,
                use_decls,
            }
        }
        (Ok((tir, project)), Err(_)) => {
            // TIR succeeded (from disk) but in-memory text has parse errors.
            // This happens when the user has unsaved edits that break parsing.
            // Produce diagnostics from the in-memory text (not disk) so the
            // user sees the parse error.
            let diagnostics = match compile_and_eval_named(text, name) {
                Ok(result) => eval_result_to_diagnostics(&result, text),
                Err(e) => compile_error_to_diagnostics(&e, text),
            };

            let fn_signatures = build_fn_signatures(Some(&tir));

            // Use the on-disk AST for a partial symbol table and eval values.
            let (symbol_table, imported_definitions, eval_values, use_decls) = uri
                .to_file_path()
                .ok()
                .and_then(|path| std::fs::read_to_string(&path).ok())
                .and_then(|disk_text| {
                    graphcal_syntax::parser::Parser::with_name(&disk_text, name)
                        .parse_file()
                        .ok()
                        .map(|ast| (disk_text, ast))
                })
                .map_or_else(
                    || {
                        (
                            SymbolTable::default(),
                            HashMap::new(),
                            HashMap::new(),
                            Vec::new(),
                        )
                    },
                    |(_, disk_ast)| {
                        let mut st = symbol_table::build_from_ast(&disk_ast);
                        symbol_table::enrich_from_tir(&mut st, &tir);
                        let imports = project.map_or_else(HashMap::new, |p| {
                            collect_imported_definitions(uri, &disk_ast, &p, Some(&tir))
                        });
                        // Keep eval values from the last valid (disk) version.
                        let (_, vals) = run_eval(uri, text, name);
                        let udecls = collect_use_decl_info(&disk_ast);
                        (st, imports, vals, udecls)
                    },
                );

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values,
                fn_signatures,
                use_decls,
            }
        }
        (Err(e), Ok(ast)) => {
            // TIR failed but in-memory parse succeeded — use AST for symbol table.
            let symbol_table = symbol_table::build_from_ast(&ast);
            let imported_definitions = collect_imported_definitions_from_ast(uri, &ast);
            let diagnostics = compile_error_to_diagnostics(&e, text);
            let use_decls = collect_use_decl_info(&ast);

            AnalysisResult {
                source: text.to_string(),
                symbol_table,
                imported_definitions,
                diagnostics,
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(None),
                use_decls,
            }
        }
        (Err(e), Err(_)) => {
            // Both failed — minimal result with diagnostics.
            let diagnostics = compile_error_to_diagnostics(&e, text);

            AnalysisResult {
                source: text.to_string(),
                symbol_table: SymbolTable::default(),
                imported_definitions: HashMap::new(),
                diagnostics,
                eval_values: HashMap::new(),
                fn_signatures: build_fn_signatures(None),
                use_decls: Vec::new(),
            }
        }
    }
}

/// Run evaluation and extract both diagnostics and formatted values.
fn run_eval(uri: &Url, text: &str, name: &str) -> (Vec<Diagnostic>, HashMap<String, String>) {
    let eval_result = uri.to_file_path().map_or_else(
        |()| compile_and_eval_named(text, name),
        |path| compile_and_eval_project(&path, &HashMap::new()),
    );

    match eval_result {
        Ok(result) => {
            let diagnostics = eval_result_to_diagnostics(&result, text);
            let values = format_eval_values(&result);
            (diagnostics, values)
        }
        Err(e) => {
            let diagnostics = compile_error_to_diagnostics(&e, text);
            (diagnostics, HashMap::new())
        }
    }
}

/// Extract use-declaration info from an AST for Document Links.
fn collect_use_decl_info(ast: &graphcal_syntax::ast::File) -> Vec<UseDeclInfo> {
    ast.declarations
        .iter()
        .filter_map(|decl| {
            if let DeclKind::Use(u) = &decl.kind {
                Some(UseDeclInfo {
                    path: u.path.clone(),
                    path_span: u.path_span,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Build structured function signatures for Signature Help.
///
/// Combines builtin function signatures (always available) with user-defined
/// function signatures from the TIR (when available).
fn build_fn_signatures(tir: Option<&graphcal_eval::tir::TIR>) -> HashMap<String, FnSignatureInfo> {
    let mut sigs = HashMap::new();

    // Builtin functions — always available.
    for (name, f) in &builtin_functions() {
        let (params, ret) = builtin_signature_parts(f.arity, f.dim_sig);
        let params_str = params.join(", ");
        let label = format!("fn {name}({params_str}) -> {ret}");
        sigs.insert(
            (*name).to_string(),
            FnSignatureInfo {
                label,
                parameters: params,
            },
        );
    }

    // User-defined functions — from TIR resolved signatures.
    if let Some(tir) = tir {
        let registry = &tir.registry;
        for (fn_name, sig) in &tir.resolved_fn_sigs {
            let param_strs: Vec<String> = sig
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.resolved_type.format(registry)))
                .collect();

            let generics =
                if sig.generic_dim_params.is_empty() && sig.generic_index_params.is_empty() {
                    String::new()
                } else {
                    let all: Vec<String> = sig
                        .generic_dim_params
                        .iter()
                        .map(|p| format!("{p}: Dim"))
                        .chain(
                            sig.generic_index_params
                                .iter()
                                .map(|p| format!("{p}: Index")),
                        )
                        .collect();
                    format!("<{}>", all.join(", "))
                };

            let ret = sig.return_type.format(registry);
            let label = format!("fn {fn_name}{generics}({}) -> {ret}", param_strs.join(", "));
            sigs.insert(
                fn_name.as_str().to_string(),
                FnSignatureInfo {
                    label,
                    parameters: param_strs,
                },
            );
        }
    }

    sigs
}

/// Generate human-readable parameter and return type strings for a builtin function.
fn builtin_signature_parts(arity: usize, dim_sig: DimSignature) -> (Vec<String>, String) {
    match dim_sig {
        DimSignature::AllDimensionless => {
            let params: Vec<String> = if arity == 1 {
                vec!["x: Dimensionless".to_string()]
            } else {
                vec![
                    "a: Dimensionless".to_string(),
                    "b: Dimensionless".to_string(),
                ]
            };
            (params, "Dimensionless".to_string())
        }
        DimSignature::AngleToDimensionless => {
            (vec!["x: Angle".to_string()], "Dimensionless".to_string())
        }
        DimSignature::DimensionlessToAngle => {
            (vec!["x: Dimensionless".to_string()], "Angle".to_string())
        }
        DimSignature::Sqrt => (vec!["x: D^2".to_string()], "D".to_string()),
        DimSignature::Passthrough => (vec!["x: D".to_string()], "D".to_string()),
        DimSignature::SameDimension => (
            vec!["a: D".to_string(), "b: D".to_string()],
            "D".to_string(),
        ),
        DimSignature::SameDimensionToAngle => (
            vec!["y: D".to_string(), "x: D".to_string()],
            "Angle".to_string(),
        ),
    }
}

/// Format all successfully evaluated values into display strings.
fn format_eval_values(result: &EvalResult) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value_result, _decl_type) in &result.all {
        if let Ok(value) = value_result {
            map.insert(
                name.as_str().to_string(),
                format_value_inline(value, &result.base_dim_symbols),
            );
        }
    }
    map
}

/// Maximum character length for inlay hint display strings.
/// When the formatted value exceeds this, entries are truncated with `...`.
const INLAY_HINT_MAX_LEN: usize = 80;

/// Format a single `Value` as a compact inline string for inlay hints.
///
/// - Scalar: `"9.81 [m/s^2]"` or `"3.14159"` (dimensionless)
/// - Bool: `"true"` / `"false"`
/// - Int: `"42"`
/// - Struct: `"LowThrust { thrust: 0.5 [N], duration: 3600 [s] }"`
/// - Indexed: `"{ Departure: 4.92 [km/s], Correction: 0.24 [km/s], ... }"`
fn format_value_inline(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
) -> String {
    format_value_inline_with_budget(value, symbols, INLAY_HINT_MAX_LEN)
}

/// Format a `Value` with a character budget. When the formatted entries would
/// exceed `max_len`, remaining entries are replaced with `...`.
fn format_value_inline_with_budget(
    value: &Value,
    symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    match value {
        Value::Scalar { .. } => {
            let formatted = format_number(value.display_value().expect("matched as Scalar"));
            value.display_label(symbols).map_or_else(
                || formatted.clone(),
                |label| format!("{formatted} [{label}]"),
            )
        }
        Value::Bool(b) => format!("{b}"),
        Value::Int(i) => format!("{i}"),
        Value::Struct {
            variant, fields, ..
        } => {
            if fields.is_empty() {
                return format!("{variant} {{}}");
            }
            format_braced_entries(
                &format!("{variant} "),
                fields
                    .iter()
                    .map(|(k, v)| (k.as_str(), v))
                    .collect::<Vec<_>>(),
                symbols,
                max_len,
            )
        }
        Value::Indexed { entries, .. } => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            format_braced_entries(
                "",
                entries
                    .iter()
                    .map(|(k, v)| (k.as_str(), v))
                    .collect::<Vec<_>>(),
                symbols,
                max_len,
            )
        }
    }
}

/// Format a list of key-value pairs as `{prefix}{ k1: v1, k2: v2, ... }`,
/// truncating with `...` when the result would exceed `max_len`.
fn format_braced_entries(
    prefix: &str,
    entries: Vec<(&str, &Value)>,
    symbols: &std::collections::BTreeMap<graphcal_syntax::dimension::BaseDimId, String>,
    max_len: usize,
) -> String {
    let mut result = format!("{prefix}{{ ");
    let suffix = " }";
    let ellipsis = "... }";
    let total = entries.len();

    for (i, (key, val)) in entries.into_iter().enumerate() {
        let remaining_budget = max_len.saturating_sub(result.len() + suffix.len());
        let entry_str = format!(
            "{key}: {}",
            format_value_inline_with_budget(val, symbols, remaining_budget)
        );

        // Check if adding this entry (plus separator and closing) would exceed budget
        let separator = if i + 1 < total { ", " } else { "" };
        let needed = entry_str.len() + separator.len();

        if i > 0 && result.len() + needed + suffix.len() > max_len {
            // Truncate: replace with ellipsis
            result.push_str(ellipsis);
            return result;
        }

        result.push_str(&entry_str);
        if i + 1 < total {
            result.push_str(", ");
        }
    }

    result.push_str(suffix);
    result
}

/// Format a number for display: integers without decimal point, floats with
/// reasonable precision (up to 6 decimal places, trailing zeros stripped).
#[expect(
    clippy::cast_possible_truncation,
    reason = "guarded by abs() < 1e15 check"
)]
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let s = format!("{value:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}

/// Collect imported definitions from a loaded project.
///
/// For each `use` declaration in the root file, resolves the import path,
/// looks up the imported file in the project, and builds a symbol table
/// from the imported file's AST to extract the definition info.
fn collect_imported_definitions(
    root_uri: &Url,
    root_ast: &graphcal_syntax::ast::File,
    project: &graphcal_eval::loader::LoadedProject,
    tir: Option<&graphcal_eval::tir::TIR>,
) -> HashMap<String, ImportedDefinition> {
    let mut result = HashMap::new();

    let Ok(root_path) = root_uri.to_file_path() else {
        return result;
    };
    let root_dir = root_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    for decl in &root_ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = root_dir.join(&use_decl.path);
            let Ok(canonical) = import_path.canonicalize() else {
                continue;
            };
            let Some(loaded_file) = project.files.get(&canonical) else {
                continue;
            };

            let mut imported_table = symbol_table::build_from_ast(&loaded_file.ast);
            if let Some(tir) = tir {
                symbol_table::enrich_from_tir(&mut imported_table, tir);
            }

            let imported_uri = Url::from_file_path(&loaded_file.path).unwrap_or_else(|()| {
                Url::parse(&format!("file://{}", loaded_file.path.display()))
                    .unwrap_or_else(|_| root_uri.clone())
            });
            let source = loaded_file.source.to_string();

            for use_item in &use_decl.names {
                if let Some(def) = imported_table.definitions.remove(&use_item.name.name) {
                    result.insert(
                        use_item.name.name.clone(),
                        ImportedDefinition {
                            uri: imported_uri.clone(),
                            source: source.clone(),
                            definition: def,
                        },
                    );
                }
            }
        }
    }

    result
}

/// Fallback: collect imported definitions by reading and parsing imported files directly.
/// Used when `compile_to_tir_project` fails but the root file parses successfully.
fn collect_imported_definitions_from_ast(
    root_uri: &Url,
    root_ast: &graphcal_syntax::ast::File,
) -> HashMap<String, ImportedDefinition> {
    let mut result = HashMap::new();

    let Ok(root_path) = root_uri.to_file_path() else {
        return result;
    };
    let root_dir = root_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    for decl in &root_ast.declarations {
        if let DeclKind::Use(use_decl) = &decl.kind {
            let import_path = root_dir.join(&use_decl.path);
            let Ok(canonical) = import_path.canonicalize() else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&canonical) else {
                continue;
            };
            let file_name = canonical.display().to_string();
            let Ok(ast) =
                graphcal_syntax::parser::Parser::with_name(&source, &file_name).parse_file()
            else {
                continue;
            };

            let mut imported_table = symbol_table::build_from_ast(&ast);

            let imported_uri = Url::from_file_path(&canonical).unwrap_or_else(|()| {
                Url::parse(&format!("file://{}", canonical.display()))
                    .unwrap_or_else(|_| root_uri.clone())
            });

            for use_item in &use_decl.names {
                if let Some(def) = imported_table.definitions.remove(&use_item.name.name) {
                    result.insert(
                        use_item.name.name.clone(),
                        ImportedDefinition {
                            uri: imported_uri.clone(),
                            source: source.clone(),
                            definition: def,
                        },
                    );
                }
            }
        }
    }

    result
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                document_symbol_provider: Some(OneOf::Left(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["@".to_string(), ":".to_string()]),
                    resolve_provider: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "graphcal-lsp initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.analyze_and_publish(params.text_document.uri, params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().last() {
            self.analyze_and_publish(params.text_document.uri, change.text)
                .await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text)
                .await;
        }
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let result = crate::document_symbols::build_document_symbols(analysis);
        drop(docs);
        Ok(Some(DocumentSymbolResponse::Nested(result)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::goto_definition::goto_definition(analysis, &uri, offset);
        drop(docs);
        Ok(result)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::hover::hover(analysis, offset);
        drop(docs);
        Ok(result)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::references::references(analysis, &uri, offset, include_declaration);
        drop(docs);
        Ok(result)
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let result = crate::inlay_hints::inlay_hints(analysis, params.range);
        drop(docs);
        Ok(result)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::signature_help::signature_help(analysis, offset);
        drop(docs);
        Ok(result)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::completion::completion(analysis, offset);
        drop(docs);
        Ok(result.map(CompletionResponse::Array))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::rename::rename(analysis, &uri, offset, &new_name);
        drop(docs);
        Ok(result)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = position_to_byte_offset(&analysis.source, position);
        let result = crate::rename::prepare_rename(analysis, offset);
        drop(docs);
        Ok(result)
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;

        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let result = crate::document_links::document_links(analysis, &uri);
        drop(docs);
        Ok(result)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let docs = self.documents.read().await;
        let Some(analysis) = docs.get(&uri) else {
            return Ok(None);
        };
        let result = crate::formatting::format_document(&analysis.source);
        drop(docs);
        Ok(result)
    }
}

/// Start the LSP server, reading from stdin and writing to stdout.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use std::collections::BTreeMap;

    use graphcal_eval::eval::Value;
    use graphcal_syntax::dimension::Dimension;
    use graphcal_syntax::names::{FieldName, IndexName, StructTypeName, VariantName};
    use indexmap::IndexMap;

    use super::*;

    fn empty_symbols() -> BTreeMap<graphcal_syntax::dimension::BaseDimId, String> {
        BTreeMap::new()
    }

    fn scalar(si_value: f64) -> Value {
        Value::Scalar {
            si_value,
            dimension: Dimension::dimensionless(),
            display_unit: None,
        }
    }

    #[test]
    fn format_scalar_dimensionless() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&scalar(2.72), &symbols), "2.72");
        assert_eq!(format_value_inline(&scalar(42.0), &symbols), "42");
    }

    #[test]
    fn format_bool() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&Value::Bool(true), &symbols), "true");
        assert_eq!(format_value_inline(&Value::Bool(false), &symbols), "false");
    }

    #[test]
    fn format_int() {
        let symbols = empty_symbols();
        assert_eq!(format_value_inline(&Value::Int(7), &symbols), "7");
    }

    #[test]
    fn format_struct_with_fields() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("dv1"), scalar(100.0));
        fields.insert(FieldName::new("dv2"), scalar(200.0));
        let val = Value::Struct {
            type_name: StructTypeName::new("TransferResult"),
            variant: VariantName::new("TransferResult"),
            fields,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "TransferResult { dv1: 100, dv2: 200 }"
        );
    }

    #[test]
    fn format_struct_empty_fields() {
        let symbols = empty_symbols();
        let val = Value::Struct {
            type_name: StructTypeName::new("Nominal"),
            variant: VariantName::new("Nominal"),
            fields: IndexMap::new(),
        };
        assert_eq!(format_value_inline(&val, &symbols), "Nominal {}");
    }

    #[test]
    fn format_struct_multi_variant() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("thrust"), scalar(0.5));
        fields.insert(FieldName::new("duration"), scalar(3600.0));
        let val = Value::Struct {
            type_name: StructTypeName::new("ManeuverKind"),
            variant: VariantName::new("LowThrust"),
            fields,
        };
        assert_eq!(
            format_value_inline(&val, &symbols),
            "LowThrust { thrust: 0.5, duration: 3600 }"
        );
    }

    #[test]
    fn format_indexed() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        entries.insert(VariantName::new("A"), scalar(1.0));
        entries.insert(VariantName::new("B"), scalar(2.0));
        entries.insert(VariantName::new("C"), scalar(3.0));
        let val = Value::Indexed {
            index_name: IndexName::new("Phase"),
            entries,
        };
        assert_eq!(format_value_inline(&val, &symbols), "{ A: 1, B: 2, C: 3 }");
    }

    #[test]
    fn format_indexed_empty() {
        let symbols = empty_symbols();
        let val = Value::Indexed {
            index_name: IndexName::new("Phase"),
            entries: IndexMap::new(),
        };
        assert_eq!(format_value_inline(&val, &symbols), "{}");
    }

    #[test]
    fn format_indexed_truncation() {
        let symbols = empty_symbols();
        let mut entries = IndexMap::new();
        // Create entries with long names to trigger truncation at 80 chars
        entries.insert(VariantName::new("LongVariantAlpha"), scalar(1.23456));
        entries.insert(VariantName::new("LongVariantBeta"), scalar(2.34567));
        entries.insert(VariantName::new("LongVariantGamma"), scalar(3.45678));
        entries.insert(VariantName::new("LongVariantDelta"), scalar(4.56789));
        let val = Value::Indexed {
            index_name: IndexName::new("Idx"),
            entries,
        };
        let result = format_value_inline(&val, &symbols);
        assert!(
            result.len() <= INLAY_HINT_MAX_LEN + 10,
            "result too long: {result}"
        );
        assert!(result.ends_with("... }"), "expected truncation: {result}");
    }

    #[test]
    fn format_struct_inside_indexed() {
        let symbols = empty_symbols();
        let mut fields = IndexMap::new();
        fields.insert(FieldName::new("x"), scalar(1.0));
        let struct_val = Value::Struct {
            type_name: StructTypeName::new("Point"),
            variant: VariantName::new("Point"),
            fields,
        };
        let mut entries = IndexMap::new();
        entries.insert(VariantName::new("A"), struct_val);
        let val = Value::Indexed {
            index_name: IndexName::new("Idx"),
            entries,
        };
        assert_eq!(format_value_inline(&val, &symbols), "{ A: Point { x: 1 } }");
    }
}

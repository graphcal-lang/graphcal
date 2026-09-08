//! JSON-RPC boundary tests for advertised capabilities and negotiated shapes.

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{Duration, Instant, timeout};
use tower::{Service, ServiceExt};
use tower_lsp::ClientSocket;
use tower_lsp::jsonrpc::{Request, Response};
use tower_lsp::{LspService, lsp_types::Url};

use crate::server::{Backend, service};

const SOURCE: &str = "\
index Phase;
node thrust: Dimensionless = 1.0;
node doubled: Dimensionless = @thrust * 2.0;
node root: Dimensionless = sqrt(4.0);
";

async fn request(service: &mut LspService<Backend>, method: &'static str, params: Value) -> Value {
    let response = service
        .ready()
        .await
        .expect("service ready")
        .call(Request::build(method).params(params).id(1).finish())
        .await
        .expect("service call")
        .expect("request response");
    let (_, body) = response.into_parts();
    body.unwrap_or_else(|error| panic!("{method} failed: {error}"))
}

async fn notify(service: &mut LspService<Backend>, method: &'static str, params: Value) {
    let response = service
        .ready()
        .await
        .expect("service ready")
        .call(Request::build(method).params(params).finish())
        .await
        .expect("service call");
    assert!(response.is_none(), "notification returned a response");
}

async fn initialize(service: &mut LspService<Backend>, capabilities: Value) -> Value {
    request(
        service,
        "initialize",
        json!({
            "processId": null,
            "capabilities": capabilities,
            "workspaceFolders": null
        }),
    )
    .await
}

fn open_params(uri: &Url, version: i32, text: &str) -> Value {
    json!({
        "textDocument": {
            "uri": uri,
            "languageId": "graphcal",
            "version": version,
            "text": text
        }
    })
}

fn position_params(uri: &Url, line: u32, character: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character }
    })
}

async fn open_with_refresh_client(
    service: &mut LspService<Backend>,
    socket: ClientSocket,
    uri: &Url,
    version: i32,
    source: &str,
) {
    let (mut requests, mut responses) = socket.split();
    let open = notify(
        service,
        "textDocument/didOpen",
        open_params(uri, version, source),
    );
    let client = async {
        let mut published = false;
        let mut refreshed = false;
        while !published || !refreshed {
            let message = timeout(Duration::from_secs(5), requests.next())
                .await
                .expect("server-to-client message timeout")
                .expect("client socket closed");
            match message.method() {
                "textDocument/publishDiagnostics" => published = true,
                "workspace/inlayHint/refresh" => {
                    refreshed = true;
                    let id = message.id().cloned().expect("refresh request id");
                    responses
                        .send(Response::from_ok(id, json!(null)))
                        .await
                        .expect("refresh response");
                }
                method => panic!("unexpected server-to-client method: {method}"),
            }
        }
    };
    tokio::join!(open, client);
}

async fn dispatch_coordinate_and_document_requests(service: &mut LspService<Backend>, uri: &Url) {
    let request_cases = [
        (
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 38 }
            }),
        ),
        (
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 6 },
                "context": { "includeDeclaration": true }
            }),
        ),
        (
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 4, "character": 0 }
                }
            }),
        ),
        (
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 37 }
            }),
        ),
        (
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 40 }
            }),
        ),
        ("textDocument/prepareRename", position_params(uri, 1, 6)),
        (
            "textDocument/documentLink",
            json!({ "textDocument": { "uri": uri } }),
        ),
        (
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": uri },
                "options": { "tabSize": 4, "insertSpaces": true }
            }),
        ),
    ];
    for (method, params) in request_cases {
        let _ = request(service, method, params).await;
    }
}

#[tokio::test]
async fn minimal_client_uses_legacy_shapes_and_gets_no_optional_refresh() {
    let (mut service, mut socket) = service();
    let initialization = initialize(&mut service, json!({})).await;
    let capabilities = &initialization["capabilities"];

    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert!(capabilities.get("codeActionProvider").is_none());
    for capability in [
        "textDocumentSync",
        "documentSymbolProvider",
        "definitionProvider",
        "referencesProvider",
        "hoverProvider",
        "inlayHintProvider",
        "signatureHelpProvider",
        "completionProvider",
        "renameProvider",
        "documentLinkProvider",
        "documentFormattingProvider",
    ] {
        assert!(
            capabilities.get(capability).is_some(),
            "missing advertised capability {capability}"
        );
    }

    let uri = Url::parse("untitled:minimal.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 3, SOURCE),
    )
    .await;
    let diagnostics = timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("publish diagnostics timeout")
        .expect("publish diagnostics notification");
    assert_eq!(diagnostics.method(), "textDocument/publishDiagnostics");
    assert!(
        timeout(Duration::from_millis(50), socket.next())
            .await
            .is_err(),
        "minimal client must not receive an inlay-hint refresh request"
    );

    let symbols = request(
        &mut service,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    )
    .await;
    assert!(
        symbols
            .as_array()
            .is_some_and(|symbols| !symbols.is_empty())
    );
    assert!(
        symbols[0].get("location").is_some(),
        "expected flat symbols"
    );
    assert!(symbols[0].get("range").is_none());

    let hover = request(
        &mut service,
        "textDocument/hover",
        position_params(&uri, 1, 6),
    )
    .await;
    assert_eq!(hover["contents"]["kind"], "plaintext");
    assert!(
        !hover["contents"]["value"]
            .as_str()
            .expect("hover value")
            .contains("```")
    );

    let rename = request(
        &mut service,
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 6 },
            "newName": "force"
        }),
    )
    .await;
    assert!(rename.get("changes").is_some());
    assert!(rename.get("documentChanges").is_none());
}

fn code_action_params(uri: &Url, only: &str) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 5 }
        },
        "context": {
            "diagnostics": [{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 5 }
                },
                "message": "required index must be public",
                "code": "graphcal::V002"
            }],
            "only": [only]
        }
    })
}

async fn assert_versioned_edits(service: &mut LspService<Backend>, uri: &Url) {
    let filtered = request(
        service,
        "textDocument/codeAction",
        code_action_params(uri, "source"),
    )
    .await;
    assert!(filtered.is_null());

    let actions = request(
        service,
        "textDocument/codeAction",
        code_action_params(uri, "quickfix"),
    )
    .await;
    assert_eq!(actions[0]["kind"], "quickfix");
    assert_eq!(
        actions[0]["edit"]["documentChanges"][0]["textDocument"]["version"],
        7
    );
    assert!(actions[0]["edit"].get("changes").is_none());

    let rename = request(
        service,
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 6 },
            "newName": "force"
        }),
    )
    .await;
    assert_eq!(rename["documentChanges"][0]["textDocument"]["version"], 7);
}

#[tokio::test]
async fn rich_client_dispatches_every_advertised_request_shape() {
    let (mut service, socket) = service();
    let initialization = initialize(
        &mut service,
        json!({
            "workspace": {
                "workspaceEdit": { "documentChanges": true },
                "inlayHint": { "refreshSupport": true }
            },
            "textDocument": {
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": ["quickfix"] }
                    },
                    "isPreferredSupport": true
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "dataSupport": true
                }
            }
        }),
    )
    .await;
    assert_eq!(
        initialization["capabilities"]["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix"])
    );

    let uri = Url::parse("untitled:rich.gcl").unwrap();
    open_with_refresh_client(&mut service, socket, &uri, 7, SOURCE).await;

    let symbols = request(
        &mut service,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    )
    .await;
    assert!(symbols[0].get("range").is_some(), "expected nested symbols");
    assert!(symbols[0].get("location").is_none());

    let hover = request(
        &mut service,
        "textDocument/hover",
        position_params(&uri, 1, 6),
    )
    .await;
    assert_eq!(hover["contents"]["kind"], "markdown");

    dispatch_coordinate_and_document_requests(&mut service, &uri).await;

    assert_versioned_edits(&mut service, &uri).await;
}

#[tokio::test]
async fn initialized_registers_supported_filesystem_watchers() {
    let (mut service, socket) = service();
    initialize(
        &mut service,
        json!({
            "workspace": {
                "didChangeWatchedFiles": { "dynamicRegistration": true }
            }
        }),
    )
    .await;
    let (mut requests, mut responses) = socket.split();
    notify(&mut service, "initialized", json!({})).await;

    loop {
        let message = timeout(Duration::from_secs(5), requests.next())
            .await
            .expect("watcher registration timeout")
            .expect("client socket closed");
        if message.method() == "window/logMessage" {
            continue;
        }
        assert_eq!(message.method(), "client/registerCapability");
        let registration = &message.params().expect("registration params")["registrations"][0];
        assert_eq!(registration["method"], "workspace/didChangeWatchedFiles");
        let watchers = registration["registerOptions"]["watchers"]
            .as_array()
            .expect("watchers array");
        assert_eq!(watchers.len(), 4);
        assert!(watchers.iter().all(|watcher| watcher["kind"] == 7));
        let id = message.id().cloned().expect("registration request id");
        responses
            .send(Response::from_ok(id, json!(null)))
            .await
            .expect("registration response");
        break;
    }
}

#[tokio::test]
async fn coordinate_requests_reject_stale_snapshots() {
    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let uri = Url::parse("untitled:stale.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, SOURCE),
    )
    .await;
    let _ = timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("publish diagnostics timeout");

    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": "node broken: Dimensionless =" }]
        }),
    )
    .await;
    let hover = request(
        &mut service,
        "textDocument/hover",
        position_params(&uri, 0, 5),
    )
    .await;
    assert!(hover.is_null(), "stale coordinate response: {hover}");
}

#[tokio::test]
#[ignore = "manual latency/resource baseline; run with --release --ignored --nocapture"]
async fn protocol_latency_baseline() {
    const ITERATIONS: u32 = 100;

    let (mut service, mut socket) = service();
    let started = Instant::now();
    initialize(&mut service, json!({})).await;
    let initialize_elapsed = started.elapsed();
    let uri = Url::parse("untitled:baseline.gcl").unwrap();
    let started = Instant::now();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, SOURCE),
    )
    .await;
    let _ = timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("publish diagnostics timeout");
    let open_elapsed = started.elapsed();

    let mut hover_samples = Vec::with_capacity(ITERATIONS as usize);
    let mut completion_samples = Vec::with_capacity(ITERATIONS as usize);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let _ = request(
            &mut service,
            "textDocument/hover",
            position_params(&uri, 1, 6),
        )
        .await;
        hover_samples.push(started.elapsed());

        let started = Instant::now();
        let _ = request(
            &mut service,
            "textDocument/completion",
            position_params(&uri, 2, 40),
        )
        .await;
        completion_samples.push(started.elapsed());
    }
    hover_samples.sort_unstable();
    completion_samples.sort_unstable();
    let percentile =
        |samples: &[Duration], percentile: usize| samples[(samples.len() - 1) * percentile / 100];

    eprintln!(
        "LSP_BASELINE initialize_us={} open_to_diagnostics_us={} hover_p50_us={} hover_p95_us={} completion_p50_us={} completion_p95_us={} iterations={ITERATIONS}",
        initialize_elapsed.as_micros(),
        open_elapsed.as_micros(),
        percentile(&hover_samples, 50).as_micros(),
        percentile(&hover_samples, 95).as_micros(),
        percentile(&completion_samples, 50).as_micros(),
        percentile(&completion_samples, 95).as_micros(),
    );
}

#[tokio::test]
async fn formatting_uses_utf16_positions_at_the_json_rpc_boundary() {
    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let uri = Url::parse("untitled:unicode.gcl").unwrap();
    let source = "node   x: Dimensionless = 1.0; // 🚀";
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, source),
    )
    .await;
    let _ = timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("publish diagnostics timeout");

    let edits = request(
        &mut service,
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 4, "insertSpaces": true }
        }),
    )
    .await;
    let expected_utf16_length = source.encode_utf16().count();
    assert_eq!(edits[0]["range"]["end"]["character"], expected_utf16_length);
    assert_ne!(expected_utf16_length, source.len());
}

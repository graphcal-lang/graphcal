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

/// The JSON-RPC error `method` answers with; panics on a successful answer.
async fn request_error(
    service: &mut LspService<Backend>,
    method: &'static str,
    params: Value,
) -> tower_lsp::jsonrpc::Error {
    let response = service
        .ready()
        .await
        .expect("service ready")
        .call(Request::build(method).params(params).id(1).finish())
        .await
        .expect("service call")
        .expect("request response");
    let (_, body) = response.into_parts();
    body.expect_err("expected a JSON-RPC error")
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

async fn next_diagnostics(socket: &mut ClientSocket, uri: &Url) -> Value {
    loop {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("publish diagnostics timeout")
            .expect("client socket closed");
        if message.method() != "textDocument/publishDiagnostics" {
            continue;
        }
        let params = message.params().expect("diagnostic params");
        if params["uri"] == uri.as_str() {
            return params["diagnostics"].clone();
        }
    }
}

async fn next_nonempty_diagnostics(socket: &mut ClientSocket) -> Value {
    loop {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("non-empty diagnostics timeout")
            .expect("client socket closed");
        if message.method() != "textDocument/publishDiagnostics" {
            continue;
        }
        let diagnostics = &message.params().expect("diagnostic params")["diagnostics"];
        if !diagnostics.as_array().is_some_and(Vec::is_empty) {
            return diagnostics.clone();
        }
    }
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
                "code": "graphcal::V002",
                "data": { "kind": "add_visibility", "item": "Phase", "visibility": "bindable" }
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

fn watched_change_params(uri: &Url, change_type: u8) -> Value {
    json!({ "changes": [{ "uri": uri, "type": change_type }] })
}

async fn notify_watched_change(service: &mut LspService<Backend>, uri: &Url, change_type: u8) {
    notify(
        service,
        "workspace/didChangeWatchedFiles",
        watched_change_params(uri, change_type),
    )
    .await;
}

async fn calculated_hint_labels(service: &mut LspService<Backend>, uri: &Url) -> Vec<String> {
    request(
        service,
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 10, "character": 0 }
            }
        }),
    )
    .await
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|hint| hint["label"].as_str().map(str::to_string))
    .collect()
}

#[tokio::test]
async fn creating_previously_missing_dependency_repairs_open_importer() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/created");
    std::fs::create_dir_all(&source_dir).unwrap();
    let main_path = source_dir.join("main.gcl");
    let library_path = source_dir.join("lib.gcl");
    let source = "import created.lib::{value};\nnode result: Dimensionless = @value;\n";
    std::fs::write(
        project.path().join("graphcal.toml"),
        "[package]\nname = \"created\"\n",
    )
    .unwrap();
    std::fs::write(&main_path, source).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let library_uri = Url::from_file_path(&library_path).unwrap();

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let ((), diagnostics) = tokio::join!(
        notify(
            &mut service,
            "textDocument/didOpen",
            open_params(&main_uri, 1, source),
        ),
        next_nonempty_diagnostics(&mut socket),
    );
    assert!(diagnostics.is_array());
    let drain = tokio::spawn(async move { while socket.next().await.is_some() {} });

    std::fs::write(
        &library_path,
        "pub const node value: Dimensionless = 7.0;\n",
    )
    .unwrap();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        watched_change_params(&library_uri, 1),
    )
    .await;
    wait_for_hint(&mut service, &main_uri, " = 7").await;
    drain.abort();
}

/// What a refresh-capable test client observed, in arrival order.
#[derive(Debug)]
enum ClientEvent {
    InlayHintRefresh,
    Diagnostics(Url),
}

/// Answer every server request and report what arrived.
fn spawn_refresh_client(socket: ClientSocket) -> tokio::sync::mpsc::UnboundedReceiver<ClientEvent> {
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    let (mut requests, mut responses) = socket.split();
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let event = match message.method() {
                "workspace/inlayHint/refresh" => ClientEvent::InlayHintRefresh,
                "textDocument/publishDiagnostics" => {
                    let uri = message.params().expect("diagnostic params")["uri"]
                        .as_str()
                        .and_then(|uri| Url::parse(uri).ok())
                        .expect("diagnostic uri");
                    ClientEvent::Diagnostics(uri)
                }
                _ => continue,
            };
            if let Some(id) = message.id().cloned() {
                let _ = responses.send(Response::from_ok(id, json!(null))).await;
            }
            if events.send(event).is_err() {
                break;
            }
        }
    });
    received
}

async fn next_client_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<ClientEvent>,
) -> ClientEvent {
    timeout(Duration::from_secs(30), events.recv())
        .await
        .expect("client event timeout")
        .expect("client closed")
}

/// LSP-4 (#2253): one `didChangeWatchedFiles` batch touching many
/// dependencies of an open root schedules that root once, with one refresh.
#[tokio::test]
async fn watched_file_burst_schedules_each_root_once() {
    const LIBRARIES: usize = 5;

    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/burst");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        project.path().join("graphcal.toml"),
        "[package]\nname = \"burst\"\n",
    )
    .unwrap();
    let library_paths: Vec<_> = (0..LIBRARIES)
        .map(|index| source_dir.join(format!("lib{index}.gcl")))
        .collect();
    for (index, path) in library_paths.iter().enumerate() {
        std::fs::write(
            path,
            format!("pub const node v{index}: Dimensionless = 1.0;\n"),
        )
        .unwrap();
    }
    let imports = (0..LIBRARIES)
        .map(|index| format!("import burst.lib{index}::{{v{index}}};"))
        .collect::<Vec<_>>()
        .join("\n");
    let sum = (0..LIBRARIES)
        .map(|index| format!("@v{index}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let main_source = format!("{imports}\nnode total: Dimensionless = {sum};\n");
    let main_path = source_dir.join("main.gcl");
    std::fs::write(&main_path, &main_source).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();

    let (mut service, socket) = service();
    initialize(
        &mut service,
        json!({ "workspace": { "inlayHint": { "refreshSupport": true } } }),
    )
    .await;
    let mut events = spawn_refresh_client(socket);
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&main_uri, 1, &main_source),
    )
    .await;
    // The open's pass publishes, then asks for a refresh.
    while !matches!(next_client_event(&mut events).await, ClientEvent::Diagnostics(uri) if uri == main_uri)
    {
    }
    while !matches!(
        next_client_event(&mut events).await,
        ClientEvent::InlayHintRefresh
    ) {}
    let generation = service.inner().generation_for_test(&main_uri);

    for (index, path) in library_paths.iter().enumerate() {
        std::fs::write(
            path,
            format!("pub const node v{index}: Dimensionless = 2.0;\n"),
        )
        .unwrap();
    }
    let changes: Vec<_> = library_paths
        .iter()
        .map(|path| json!({ "uri": Url::from_file_path(path).unwrap(), "type": 2 }))
        .collect();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        json!({ "changes": changes }),
    )
    .await;
    assert_eq!(
        service.inner().generation_for_test(&main_uri),
        generation + 1,
        "the burst schedules the root exactly once"
    );
    let mut refreshes = 0;
    loop {
        match next_client_event(&mut events).await {
            ClientEvent::InlayHintRefresh => refreshes += 1,
            ClientEvent::Diagnostics(uri) if uri == main_uri => break,
            ClientEvent::Diagnostics(_) => {}
        }
    }
    assert_eq!(refreshes, 1, "one refresh for the whole burst");
}

/// LSP-9 (#2253): `didOpen` and `didSave` return before their analysis
/// finishes. Here the pass cannot finish until the client answers its
/// inlay-hint refresh request, which the client only reads after the
/// notifications returned.
#[tokio::test]
async fn open_and_save_do_not_wait_for_their_analysis() {
    let (mut service, socket) = service();
    initialize(
        &mut service,
        json!({ "workspace": { "inlayHint": { "refreshSupport": true } } }),
    )
    .await;
    let uri = Url::parse("untitled:unawaited.gcl").unwrap();
    timeout(
        Duration::from_secs(30),
        notify(
            &mut service,
            "textDocument/didOpen",
            open_params(&uri, 1, SOURCE),
        ),
    )
    .await
    .expect("didOpen returns before its analysis");
    timeout(
        Duration::from_secs(30),
        notify(
            &mut service,
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri }, "text": SOURCE }),
        ),
    )
    .await
    .expect("didSave returns before its analysis");

    let mut events = spawn_refresh_client(socket);
    while !matches!(
        next_client_event(&mut events).await,
        ClientEvent::Diagnostics(published) if published == uri
    ) {}
}

/// LSP-16 (#2253): disk revisions of documents no analysis depends on are
/// pruned, and closing the last document empties every per-document map.
#[tokio::test]
async fn per_document_state_shrinks_after_analysis_and_close() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/shrink");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        project.path().join("graphcal.toml"),
        "[package]\nname = \"shrink\"\n",
    )
    .unwrap();
    let library_path = source_dir.join("lib.gcl");
    let unrelated_path = source_dir.join("unrelated.gcl");
    let main_path = source_dir.join("main.gcl");
    let main_source = "import shrink.lib::{v};\nnode total: Dimensionless = @v;\n";
    std::fs::write(&library_path, "pub const node v: Dimensionless = 1.0;\n").unwrap();
    std::fs::write(&unrelated_path, "node u: Dimensionless = 1.0;\n").unwrap();
    std::fs::write(&main_path, main_source).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();

    let (mut service, socket) = service();
    initialize(&mut service, json!({})).await;
    let drain = tokio::spawn(async move {
        let mut socket = socket;
        while socket.next().await.is_some() {}
    });
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&main_uri, 1, main_source),
    )
    .await;
    service.inner().wait_until_settled_for_test(&main_uri).await;

    std::fs::write(&library_path, "pub const node v: Dimensionless = 2.0;\n").unwrap();
    std::fs::write(&unrelated_path, "node u: Dimensionless = 2.0;\n").unwrap();
    let changes: Vec<_> = [&library_path, &unrelated_path]
        .into_iter()
        .map(|path| json!({ "uri": Url::from_file_path(path).unwrap(), "type": 2 }))
        .collect();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        json!({ "changes": changes }),
    )
    .await;
    service.inner().wait_until_settled_for_test(&main_uri).await;
    let disk = service.inner().disk_revision_identities_for_test().await;
    assert_eq!(disk.len(), 1, "{disk:?}");
    assert!(
        disk.iter().all(|identity| identity
            .file_path()
            .is_some_and(|path| path.ends_with("lib.gcl"))),
        "only the dependency keeps its revision: {disk:?}"
    );

    notify(
        &mut service,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": main_uri } }),
    )
    .await;
    assert!(
        service
            .inner()
            .disk_revision_identities_for_test()
            .await
            .is_empty()
    );
    assert_eq!(
        service.inner().tracked_documents_for_test().await,
        [0, 0, 0]
    );
    drain.abort();
}

async fn assert_imported_factor_features(
    service: &mut LspService<Backend>,
    main_uri: &Url,
    library_uri: &Url,
    expected_hint: &str,
) {
    let hover = request(
        service,
        "textDocument/hover",
        position_params(main_uri, 1, 32),
    )
    .await;
    assert!(!hover.is_null());
    let definition = request(
        service,
        "textDocument/definition",
        position_params(main_uri, 1, 32),
    )
    .await;
    assert_eq!(definition["uri"], library_uri.as_str());
    wait_for_hint(service, main_uri, expected_hint).await;
}

/// The codes of a published diagnostic list.
#[cfg(unix)]
fn diagnostic_codes(diagnostics: &Value) -> Vec<String> {
    diagnostics
        .as_array()
        .expect("diagnostic array")
        .iter()
        .map(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .expect("diagnostic code")
                .to_owned()
        })
        .collect()
}

#[cfg(unix)]
#[tokio::test]
async fn compile_errors_reach_a_root_opened_through_a_symlink() {
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    let source_dir = real.join("src/linked");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(real.join("graphcal.toml"), "[package]\nname = \"linked\"\n").unwrap();
    let source = "node bad: Length = 1.0 kg;\n";
    std::fs::write(source_dir.join("main.gcl"), source).unwrap();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let uri = Url::from_file_path(alias.join("src/linked/main.gcl")).unwrap();

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let ((), diagnostics) = tokio::join!(
        notify(
            &mut service,
            "textDocument/didOpen",
            open_params(&uri, 1, source),
        ),
        next_diagnostics(&mut socket, &uri),
    );
    assert_eq!(diagnostic_codes(&diagnostics), ["graphcal::D002"]);
}

#[tokio::test]
async fn external_dependency_delete_and_recreate_refreshes_open_importer() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/watched");
    std::fs::create_dir_all(&source_dir).unwrap();
    let manifest_path = project.path().join("graphcal.toml");
    let main_path = source_dir.join("main.gcl");
    let library_path = source_dir.join("lib.gcl");
    let manifest = "[package]\nname = \"watched\"\n";
    let main_source =
        "import watched.lib::{factor};\nnode result: Dimensionless = @factor * 2.0;\n";
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::write(&main_path, main_source).unwrap();
    std::fs::write(
        &library_path,
        "pub const node factor: Dimensionless = 2.0;\n",
    )
    .unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let library_uri = Url::from_file_path(library_path.canonicalize().unwrap()).unwrap();

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&main_uri, 1, main_source),
    )
    .await;
    assert!(
        next_diagnostics(&mut socket, &main_uri)
            .await
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    assert_imported_factor_features(&mut service, &main_uri, &library_uri, " = 4").await;

    std::fs::write(
        &library_path,
        "pub const node other: Dimensionless = 3.0;\n",
    )
    .unwrap();
    notify_watched_change(&mut service, &library_uri, 2).await;
    let stale_hover = request(
        &mut service,
        "textDocument/hover",
        position_params(&main_uri, 1, 32),
    )
    .await;
    assert!(stale_hover.is_null());
    assert!(next_nonempty_diagnostics(&mut socket).await.is_array());
    assert!(
        request(
            &mut service,
            "textDocument/definition",
            position_params(&main_uri, 1, 32),
        )
        .await
        .is_null()
    );

    std::fs::remove_file(&library_path).unwrap();
    notify_watched_change(&mut service, &library_uri, 3).await;
    assert!(next_nonempty_diagnostics(&mut socket).await.is_array());

    std::fs::write(
        &library_path,
        "pub const node factor: Dimensionless = 3.0;\n",
    )
    .unwrap();
    notify_watched_change(&mut service, &library_uri, 1).await;
    let hover = timeout(Duration::from_secs(5), async {
        loop {
            let hover = request(
                &mut service,
                "textDocument/hover",
                position_params(&main_uri, 1, 32),
            )
            .await;
            if !hover.is_null() {
                break hover;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("hover did not recover after dependency recreation");
    assert!(!hover.is_null());
    assert!(
        next_diagnostics(&mut socket, &main_uri)
            .await
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    assert_imported_factor_features(&mut service, &main_uri, &library_uri, " = 6").await;
}

async fn wait_for_hint(service: &mut LspService<Backend>, uri: &Url, expected: &str) {
    timeout(Duration::from_secs(5), async {
        loop {
            if calculated_hint_labels(service, uri)
                .await
                .iter()
                .any(|label| label == expected)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("inlay hint `{expected}` did not appear"));
}

#[tokio::test]
async fn open_dependency_buffer_remains_authoritative_over_watcher_events() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/overlay");
    std::fs::create_dir_all(&source_dir).unwrap();
    let main_path = source_dir.join("main.gcl");
    let library_path = source_dir.join("lib.gcl");
    let main_source =
        "import overlay.lib::{factor};\nnode result: Dimensionless = @factor * 2.0;\n";
    std::fs::write(
        project.path().join("graphcal.toml"),
        "[package]\nname = \"overlay\"\n",
    )
    .unwrap();
    std::fs::write(&main_path, main_source).unwrap();
    std::fs::write(
        &library_path,
        "pub const node factor: Dimensionless = 2.0;\n",
    )
    .unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let library_uri = Url::from_file_path(&library_path).unwrap();

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&main_uri, 1, main_source),
    )
    .await;
    let _ = next_diagnostics(&mut socket, &main_uri).await;
    let drain = tokio::spawn(async move { while socket.next().await.is_some() {} });
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(
            &library_uri,
            1,
            "pub const node factor: Dimensionless = 5.0;\n",
        ),
    )
    .await;
    wait_for_hint(&mut service, &main_uri, " = 10").await;

    std::fs::write(
        &library_path,
        "pub const node factor: Dimensionless = 100.0;\n",
    )
    .unwrap();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        watched_change_params(&library_uri, 2),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let labels = calculated_hint_labels(&mut service, &main_uri).await;
    assert!(labels.iter().any(|label| label == " = 10"));
    assert!(!labels.iter().any(|label| label == " = 200"));

    notify(
        &mut service,
        "textDocument/didClose",
        json!({ "textDocument": { "uri": library_uri } }),
    )
    .await;
    wait_for_hint(&mut service, &main_uri, " = 200").await;
    drain.abort();
}

#[tokio::test]
async fn external_manifest_change_and_repair_refreshes_open_project() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("src/manifested");
    std::fs::create_dir_all(&source_dir).unwrap();
    let manifest_path = project.path().join("graphcal.toml");
    let main_path = source_dir.join("main.gcl");
    let manifest = "[package]\nname = \"manifested\"\n";
    let source = "node result: Dimensionless = 1.0;\n";
    std::fs::write(&manifest_path, manifest).unwrap();
    std::fs::write(&main_path, source).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let manifest_uri = Url::from_file_path(&manifest_path).unwrap();

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&main_uri, 1, source),
    )
    .await;
    assert!(
        next_diagnostics(&mut socket, &main_uri)
            .await
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    std::fs::write(&manifest_path, "not valid toml = [").unwrap();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        watched_change_params(&manifest_uri, 2),
    )
    .await;
    assert!(
        !next_diagnostics(&mut socket, &main_uri)
            .await
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    std::fs::write(&manifest_path, manifest).unwrap();
    notify(
        &mut service,
        "workspace/didChangeWatchedFiles",
        watched_change_params(&manifest_uri, 2),
    )
    .await;
    assert!(
        next_diagnostics(&mut socket, &main_uri)
            .await
            .as_array()
            .is_some_and(Vec::is_empty)
    );
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
async fn inlay_hints_requested_right_after_an_edit_answer_the_edited_text() {
    // A client without inlay-hint refresh (Helix) asks once after a change
    // and keeps the answer, so the answer must be the edited text's.
    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let uri = Url::parse("untitled:edited.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, SOURCE),
    )
    .await;
    let _ = timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("publish diagnostics timeout");
    // The analysis publishes while the request waits for it.
    let drain = tokio::spawn(async move { while socket.next().await.is_some() {} });

    notify(
        &mut service,
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": "node changed: Dimensionless = 3.0;\n" }]
        }),
    )
    .await;
    assert_eq!(calculated_hint_labels(&mut service, &uri).await, [" = 3"]);
    drain.abort();
}

/// LSP-3 (#2253): slow feature work runs off tower-lsp's serving task, so a
/// fast request is answered meanwhile, and `$/cancelRequest` answers the slow
/// request at once instead of after its work.
#[tokio::test]
async fn slow_feature_work_neither_blocks_requests_nor_ignores_cancellation() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use tower_lsp::jsonrpc::ErrorCode;

    const WAIT: Duration = Duration::from_secs(30);

    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let uri = Url::parse("untitled:slow.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, SOURCE),
    )
    .await;
    let _ = timeout(WAIT, socket.next())
        .await
        .expect("publish diagnostics timeout");
    let drain = tokio::spawn(async move { while socket.next().await.is_some() {} });
    let probe = Arc::clone(&service.inner().blocking_feature_probe);

    let slow = service.ready().await.expect("service ready").call(
        Request::build(crate::server::BlockingFeatureRequest::METHOD)
            .params(json!({ "uri": uri }))
            .id(2)
            .finish(),
    );
    let slow = tokio::spawn(slow);
    timeout(WAIT, probe.started.notified())
        .await
        .expect("the slow feature work starts");

    let hover = timeout(
        WAIT,
        request(
            &mut service,
            "textDocument/hover",
            position_params(&uri, 1, 6),
        ),
    )
    .await
    .expect("a fast request is answered while slow feature work runs");
    assert!(!hover.is_null(), "hover answers the analyzed document");

    notify(&mut service, "$/cancelRequest", json!({ "id": 2 })).await;
    let cancelled = timeout(WAIT, slow)
        .await
        .expect("the cancelled request is answered while its work still runs")
        .expect("request task")
        .expect("service call")
        .expect("request response");
    let (_, body) = cancelled.into_parts();
    assert_eq!(
        body.expect_err("cancelled request").code,
        ErrorCode::RequestCancelled
    );
    // Only now is the work released: the answer did not wait for it.
    probe.released.store(true, Ordering::SeqCst);
    timeout(WAIT, probe.finished.notified())
        .await
        .expect("the released work finishes");
    drain.abort();
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

/// A view client: declares the experimental capability that opts into
/// `graphcal/viewModelDidChange`.
const VIEW_SOURCE: &str = "\
node thrust: Dimensionless = 1.0;
node doubled: Dimensionless = @thrust * 2.0;
node root: Dimensionless = sqrt(4.0);
";

#[tokio::test]
async fn view_model_request_serves_the_analyzed_document() {
    let (mut service, mut socket) = service();
    initialize(
        &mut service,
        json!({ "experimental": { "graphcal": { "viewModelChanges": true } } }),
    )
    .await;
    let uri = Url::parse("untitled:view.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, VIEW_SOURCE),
    )
    .await;
    // Analysis publishes diagnostics, then announces the view model.
    let mut announced = false;
    while !announced {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("server-to-client message timeout")
            .expect("client socket closed");
        if message.method() == "graphcal/viewModelDidChange" {
            let params = message.params().expect("notification params");
            assert_eq!(params["uri"], json!(uri));
            announced = true;
        }
    }

    let view = request(
        &mut service,
        "graphcal/viewModel",
        json!({ "textDocument": { "uri": uri } }),
    )
    .await;
    let names: Vec<&str> = view["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| node["name"].as_str().expect("node name"))
        .collect();
    assert_eq!(names, ["thrust", "doubled", "root"]);
    // Dependencies name nodes by their opaque ids.
    assert_eq!(view["nodes"][1]["deps"], json!([view["nodes"][0]["id"]]));
    assert_eq!(view["nodes"][1]["label"], "doubled");
    assert_eq!(view["nodes"][1]["value"]["display"], "2");
    assert_eq!(view["nodes"][1]["source"]["line"], 2);
    let module = request(
        &mut service,
        "graphcal/viewModel",
        json!({ "textDocument": { "uri": uri }, "target": { "kind": "module" } }),
    )
    .await;
    assert_eq!(module, view);
    let no_instance = request(
        &mut service,
        "graphcal/viewModel",
        json!({
            "textDocument": { "uri": uri },
            "target": { "kind": "instance", "instance": "no-such-instance" },
        }),
    )
    .await;
    assert_eq!(no_instance, Value::Null);

    let unknown = request(
        &mut service,
        "graphcal/viewModel",
        json!({ "textDocument": { "uri": "untitled:never-opened.gcl" } }),
    )
    .await;
    assert_eq!(unknown, Value::Null);
}

const EXTERN_SOURCE: &str = "\
import plugin \"graphcal:demo\" as demo {
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}
node y: Dimensionless = demo::lerp(0.0, 1.0, 0.5);
";

/// LSP-12: an explicit refusal reaches the user as an error with its reason,
/// both from `prepareRename` and from `rename`, rather than a bare `null`.
#[tokio::test]
async fn rename_refusals_reach_the_client_with_their_reason() {
    let (mut service, mut socket) = service();
    initialize(&mut service, json!({})).await;
    let uri = Url::parse("untitled:extern.gcl").unwrap();
    notify(
        &mut service,
        "textDocument/didOpen",
        open_params(&uri, 1, EXTERN_SOURCE),
    )
    .await;
    let diagnostics = next_diagnostics(&mut socket, &uri).await;
    assert_eq!(diagnostics, json!([]));

    let cursor = position_params(&uri, 1, 8);
    let error = request_error(&mut service, "textDocument/prepareRename", cursor.clone()).await;
    assert_eq!(error.code, tower_lsp::jsonrpc::ErrorCode::InvalidParams);
    assert!(
        error
            .message
            .contains("cannot rename extern function `demo::lerp`"),
        "{error:?}"
    );

    let mut rename = cursor;
    rename["newName"] = json!("blend");
    let error = request_error(&mut service, "textDocument/rename", rename).await;
    assert!(error.message.contains("`demo::lerp`"), "{error:?}");

    // Nothing renameable is still a plain `null`.
    let blank = request(
        &mut service,
        "textDocument/prepareRename",
        position_params(&uri, 3, 22),
    )
    .await;
    assert!(blank.is_null(), "{blank}");
}

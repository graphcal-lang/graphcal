import { validateDocument, sameDocument, type SourceDocument } from "./document";
import { element, required } from "./dom";
import { SourceEditor } from "./editor";
import { examples, loadExample } from "./examples";
import { setupLayout } from "./layout";
import { documentLocation, type DocumentLocation } from "./location";
import { Output } from "./output";
import { decodeFragment, shareUrl, WARN_URL_LENGTH } from "./share-codec";
import { WorkerClient } from "./worker-client";
import "./styles.css";

const filename = required<HTMLInputElement>("#filename");
const status = required("#status");
const stop = required<HTMLButtonElement>("#stop");
const auto = required<HTMLInputElement>("#auto");
const chooser = required<HTMLSelectElement>("#examples");
const shareStatus = required("#share-status");
const shareLink = required<HTMLInputElement>("#share-link");
let current: SourceDocument = { filename: "main.gcl", source: "" };
let original = current;
let shared: SourceDocument | undefined;
let revision = 0;
let lastLocation = window.location.href;
let observedLocation = lastLocation;
let pendingLoad: AbortController | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;
const layout = setupLayout();
const output = new Output(required("#output"), (range) => {
  layout.showEditor();
  editor.focus(range);
});
const client = new WorkerClient((event) => {
  stop.disabled = event.kind === "result" || event.kind === "error";
  switch (event.kind) {
    case "loading":
      status.textContent = "Loading browser engine…";
      break;
    case "running":
      status.textContent = "Running…";
      break;
    case "error":
      status.textContent = event.message;
      output.clear(event.message);
      break;
    case "result":
      editor.diagnostics(event.outcome.status === "compile_error" ? event.outcome.diagnostics : []);
      status.textContent = output.render(event.outcome);
      break;
  }
});
const editor = new SourceEditor(
  required("#editor"),
  current.source,
  (source) => {
    current = { ...current, source };
    edited();
  },
  run,
);
filename.value = current.filename;
auto.checked = false;
for (const example of examples) {
  const option = element("option", example.title);
  option.value = example.id;
  chooser.append(option);
}
function message(error: unknown) {
  return error instanceof Error ? error.message : "Unexpected browser error";
}
function updateShareStatus() {
  shareStatus.textContent =
    shared && sameDocument(shared, current)
      ? "URL includes the current source."
      : "Edits are not saved in the URL. Use Share to create a snapshot.";
  shareLink.hidden = true;
}
function edited() {
  revision++;
  pendingLoad?.abort();
  clearTimeout(timer);
  client.invalidate();
  stop.disabled = true;
  // Decorations cannot be dispatched from inside a CodeMirror update listener.
  queueMicrotask(() => editor.diagnostics([]));
  status.textContent = "Edited — results are stale";
  output.clear("Source changed. Run to see current results.");
  updateShareStatus();
  if (auto.checked) timer = setTimeout(run, 400);
}
function cancelLoad() {
  if (!pendingLoad) return;
  pendingLoad.abort();
  pendingLoad = undefined;
  revision++;
}
function run() {
  cancelLoad();
  clearTimeout(timer);
  try {
    client.run(validateDocument(current));
  } catch (error) {
    status.textContent = message(error);
  }
}
function mayReplace() {
  return (
    sameDocument(current, original) || confirm("Discard edits and replace the current document?")
  );
}
function load(document: SourceDocument) {
  clearTimeout(timer);
  client.stop();
  stop.disabled = true;
  original = current = validateDocument(document);
  filename.value = current.filename;
  editor.load(current.source);
  editor.diagnostics([]);
  output.clear();
  status.textContent = "Ready";
  updateShareStatus();
}
async function open(location: DocumentLocation, url: string, push: boolean) {
  pendingLoad?.abort();
  const controller = new AbortController();
  pendingLoad = controller;
  const token = ++revision;
  clearTimeout(timer);
  client.stop();
  stop.disabled = true;
  auto.checked = false;
  status.textContent = "Loading snippet…";
  try {
    const document =
      location.kind === "shared"
        ? await decodeFragment(location.fragment)
        : await loadExample(
            location.id,
            AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]),
          );
    if (token !== revision || controller.signal.aborted) return;
    pendingLoad = undefined;
    shared = location.kind === "shared" ? document : undefined;
    load(document);
    if (push) history.pushState(null, "", url);
    lastLocation = observedLocation = window.location.href;
    chooser.value = location.kind === "example" ? location.id : "";
    required("#example-description").textContent =
      location.kind === "example"
        ? examples.find((example) => example.id === location.id)!.description
        : "Shared source — review it, then press Run. Graphcal versions may change results.";
    auto.checked = location.kind === "example";
    if (auto.checked) run();
    else status.textContent = "Shared snippet loaded — press Run to evaluate";
  } catch (error) {
    if (token !== revision || controller.signal.aborted) return;
    status.textContent = message(error);
    // Keep both the document and last accepted URL when navigation fails.
    if (window.location.href !== lastLocation) history.replaceState(null, "", lastLocation);
    observedLocation = window.location.href;
  } finally {
    if (pendingLoad === controller) pendingLoad = undefined;
  }
}
filename.addEventListener("input", () => {
  current = { ...current, filename: filename.value };
  edited();
});
required("#run").addEventListener("click", run);
stop.addEventListener("click", () => {
  clearTimeout(timer);
  client.stop();
  stop.disabled = true;
  auto.checked = false;
  status.textContent = "Stopped";
});
auto.addEventListener("change", () => {
  clearTimeout(timer);
  if (auto.checked) run();
});
required("#reset").addEventListener("click", () => {
  if (!mayReplace()) return;
  revision++;
  pendingLoad?.abort();
  load(original);
  if (auto.checked) run();
});
required("#load-example").addEventListener("click", () => {
  if (!mayReplace()) return;
  const url = new URL("/playground/", window.location.origin);
  url.searchParams.set("example", chooser.value);
  void open({ kind: "example", id: chooser.value }, url.href, true);
});
required("#share").addEventListener("click", () => {
  if (pendingLoad) {
    cancelLoad();
    status.textContent = "Loading cancelled — sharing the current document";
  }
  const snapshot = current;
  const token = revision;
  void (async () => {
    try {
      const url = await shareUrl(snapshot, window.location.origin);
      if (revision !== token) {
        shareStatus.textContent = "Source changed while sharing. Press Share again.";
        return;
      }
      history.replaceState(null, "", url);
      lastLocation = observedLocation = url;
      shared = snapshot;
      shareLink.value = url;
      shareLink.hidden = false;
      shareStatus.textContent =
        url.length > WARN_URL_LENGTH
          ? "Long link: some messaging apps may truncate it. Copy the full link below."
          : "URL includes the current source. Copy the link below.";
      try {
        await navigator.clipboard.writeText(url);
        if (revision === token) shareStatus.textContent += " Link copied.";
      } catch {
        if (revision === token) {
          shareLink.focus();
          shareLink.select();
        }
      }
    } catch (error) {
      if (revision === token) shareStatus.textContent = message(error);
    }
  })();
});
function navigated() {
  if (window.location.href === observedLocation) return;
  observedLocation = window.location.href;
  if (!mayReplace()) {
    history.replaceState(null, "", lastLocation);
    observedLocation = lastLocation;
    return;
  }
  try {
    void open(documentLocation(new URL(window.location.href)), window.location.href, false);
  } catch (error) {
    status.textContent = message(error);
    history.replaceState(null, "", lastLocation);
    observedLocation = lastLocation;
  }
}
window.addEventListener("popstate", navigated);
window.addEventListener("hashchange", navigated);
window.addEventListener("beforeunload", (event) => {
  if (!sameDocument(current, original) && !(shared && sameDocument(current, shared))) {
    event.preventDefault();
    event.returnValue = "";
  }
});
window.addEventListener("pagehide", () => {
  revision++;
  pendingLoad?.abort();
  clearTimeout(timer);
  client.stop();
  output.clear();
});
window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    stop.disabled = true;
    auto.checked = false;
    editor.diagnostics([]);
    status.textContent = "Restored — press Run to evaluate";
  }
});
if (import.meta.hot)
  import.meta.hot.dispose(() => {
    revision++;
    pendingLoad?.abort();
    clearTimeout(timer);
    client.stop();
    editor.destroy();
    output.clear();
  });
try {
  void open(documentLocation(new URL(window.location.href)), window.location.href, false);
} catch (error) {
  status.textContent = message(error);
}

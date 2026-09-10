import { validateDocument, sameDocument, type SourceDocument } from "./document";
import { element, required } from "./dom";
import { SourceEditor } from "./editor";
import { examples, loadExample } from "./examples";
import { setupLayout } from "./layout";
import {
  documentLocation,
  viewLocation,
  sameLocationDocument,
  type DocumentLocation,
  type PlaygroundView,
} from "./location";
import { Output } from "./output";
import { Report } from "./report";
import { sameBindings, type Binding } from "./bindings";
import { decodeFragment, shareUrl, WARN_URL_LENGTH, type SharedCalculation } from "./share-codec";
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
let shared: SharedCalculation | undefined;
let bindings: Binding[] = [];
let restoredBindings: Binding[] | undefined;
let parametersPending = false;
let parametersEdited = false;
let view: PlaygroundView = "workspace";
let revision = 0;
let lastLocation = window.location.href;
let observedLocation = lastLocation;
let pendingLoad: AbortController | undefined;
let timer: ReturnType<typeof setTimeout> | undefined;
const layout = setupLayout();
const report = new Report(
  required("#report"),
  (nextBindings) => {
    client.run(validateDocument(current), nextBindings);
  },
  () => {
    revision++;
    parametersPending = parametersEdited = true;
    stop.disabled = false;
    status.textContent = "Parameters changed — showing last successful evaluation";
    updateShareStatus();
  },
);
const output = new Output(required("#output"), (range) => {
  selectView("workspace");
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
      report.clear(event.message + " Press Run to retry with the last applied parameters.");
      break;
    case "result": {
      const delivery = report.deliver(event.outcome);
      if (delivery === "stale") {
        stop.disabled = false;
        break;
      }
      if (event.outcome.status === "binding_errors" || event.outcome.status === "eval_error") {
        parametersPending = true;
        const error =
          event.outcome.status === "binding_errors"
            ? event.outcome.errors.map((error) => `${error.name}: ${error.message}`).join("; ")
            : event.outcome.message;
        status.textContent = "Input rejected — " + error;
        if (delivery === "initial")
          report.clear(error + " Use Reset parameters to evaluate source defaults.");
        updateShareStatus();
        break;
      }
      editor.diagnostics(event.outcome.status === "compile_error" ? event.outcome.diagnostics : []);
      status.textContent = output.render(event.outcome);
      if (event.outcome.status === "evaluated") {
        bindings = event.bindings;
        restoredBindings = undefined;
        parametersPending = false;
        revision++;
        if (delivery === "initial") report.render(event.outcome, event.ports, bindings);
        updateShareStatus();
      } else report.clear("Report unavailable. See the workspace diagnostics.");
      break;
    }
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
  shareStatus.textContent = parametersPending
    ? "Pending or rejected parameter edits are not shareable. Share uses the last successfully applied parameters."
    : shared && sameDocument(shared.document, current) && sameBindings(shared.bindings, bindings)
      ? "URL includes the current source and applied parameters."
      : "Edits are not saved in the URL. Use Share to create a snapshot.";
  shareLink.hidden = true;
}
function edited() {
  revision++;
  pendingLoad?.abort();
  clearTimeout(timer);
  client.invalidate();
  bindings = [];
  restoredBindings = undefined;
  parametersPending = parametersEdited = false;
  report.clear("Source changed. Parameter overrides cleared. Run to generate a report.");
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
    report.clear("Generating report…");
    client.run(validateDocument(current), restoredBindings ?? bindings);
  } catch (error) {
    status.textContent = message(error);
  }
}
function mayReplace() {
  return (
    (sameDocument(current, original) && !parametersEdited) ||
    confirm("Discard edits and replace the current document?")
  );
}
function load(document: SourceDocument, overrides: Binding[] = []) {
  clearTimeout(timer);
  client.stop();
  stop.disabled = true;
  original = current = validateDocument(document);
  filename.value = current.filename;
  editor.load(current.source);
  editor.diagnostics([]);
  output.clear();
  report.clear();
  bindings = [];
  restoredBindings = overrides;
  parametersPending = overrides.length > 0;
  parametersEdited = false;
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
  report.clear("Loading snippet…");
  stop.disabled = true;
  auto.checked = false;
  status.textContent = "Loading snippet…";
  try {
    const selectedView = viewLocation(new URL(url));
    const calculation =
      location.kind === "shared"
        ? await decodeFragment(location.fragment)
        : {
            document: await loadExample(
              location.id,
              AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]),
            ),
            bindings: [],
          };
    if (token !== revision || controller.signal.aborted) return;
    pendingLoad = undefined;
    shared = location.kind === "shared" ? calculation : undefined;
    load(calculation.document, calculation.bindings);
    selectView(selectedView, false);
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
required("#workspace-view").addEventListener("click", () => selectView("workspace"));
required("#report-view").addEventListener("click", () => selectView("report"));
required("#reset-parameters").addEventListener("click", () => {
  revision++;
  bindings = [];
  restoredBindings = undefined;
  parametersEdited = true;
  parametersPending = false;
  updateShareStatus();
  run();
});
function selectView(next: PlaygroundView, push = true) {
  if (push && pendingLoad) {
    cancelLoad();
    status.textContent = "Loading cancelled — view changed";
  }
  view = next;
  required("#workspace").hidden = view === "report";
  required("#report").hidden = view !== "report";
  required("#mobile-tabs").hidden = view === "report";
  required("#workspace-view").setAttribute("aria-pressed", String(view === "workspace"));
  required("#report-view").setAttribute("aria-pressed", String(view === "report"));
  if (push) {
    const url = new URL(window.location.href);
    if (view === "report") url.searchParams.set("view", view);
    else url.searchParams.delete("view");
    if (url.href !== window.location.href) history.pushState(null, "", url);
    lastLocation = observedLocation = url.href;
  }
  shareLink.hidden = true;
}
stop.addEventListener("click", () => {
  clearTimeout(timer);
  client.stop();
  stop.disabled = true;
  auto.checked = false;
  report.clear("Stopped. Run to regenerate with the last applied parameters.");
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
  if (view === "report") url.searchParams.set("view", view);
  void open({ kind: "example", id: chooser.value }, url.href, true);
});
required("#share").addEventListener("click", () => {
  if (pendingLoad) {
    cancelLoad();
    status.textContent = "Loading cancelled — sharing the current document";
  }
  const snapshot = { document: current, bindings };
  const snapshotView = view;
  const token = revision;
  void (async () => {
    try {
      const url = await shareUrl(
        snapshot.document,
        window.location.origin,
        snapshot.bindings,
        snapshotView,
      );
      if (revision !== token || view !== snapshotView) {
        shareStatus.textContent =
          "Source, parameters, or view changed while sharing. Press Share again.";
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
          : "URL includes source, applied parameters, and selected view. Copy the link below.";
      if (parametersPending)
        shareStatus.textContent += " Pending or rejected parameter edits were excluded.";
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
  try {
    const url = new URL(window.location.href);
    const nextView = viewLocation(url);
    if (sameLocationDocument(url, new URL(lastLocation))) {
      selectView(nextView, false);
      lastLocation = observedLocation = url.href;
      return;
    }
    if (!mayReplace()) {
      history.replaceState(null, "", lastLocation);
      observedLocation = lastLocation;
      return;
    }
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
  if (
    (!sameDocument(current, original) || parametersEdited) &&
    !(
      shared &&
      sameDocument(current, shared.document) &&
      sameBindings(bindings, shared.bindings) &&
      !parametersPending
    )
  ) {
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
  report.clear();
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
    report.clear();
  });
try {
  void open(documentLocation(new URL(window.location.href)), window.location.href, false);
} catch (error) {
  status.textContent = message(error);
}

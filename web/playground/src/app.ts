import rocket from "../../../tests/fixtures/valid/rocket.gcl?raw";
import { validateDocument, sameDocument, type SourceDocument } from "./document";
import { required } from "./dom";
import { SourceEditor } from "./editor";
import { setupLayout } from "./layout";
import { Output } from "./output";
import { WorkerClient } from "./worker-client";
import "./styles.css";

const filename = required<HTMLInputElement>("#filename");
const status = required("#status");
const stop = required<HTMLButtonElement>("#stop");
const auto = required<HTMLInputElement>("#auto");
let current: SourceDocument = { filename: "rocket.gcl", source: rocket };
let original = current;
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
function edited() {
  clearTimeout(timer);
  client.invalidate();
  stop.disabled = true;
  // Decorations cannot be dispatched from inside a CodeMirror update listener.
  queueMicrotask(() => editor.diagnostics([]));
  status.textContent = "Edited — results are stale";
  output.clear("Source changed. Run to see current results.");
  if (auto.checked) timer = setTimeout(run, 400);
}
function run() {
  clearTimeout(timer);
  try {
    client.run(validateDocument(current));
  } catch (error) {
    status.textContent = error instanceof Error ? error.message : "Invalid document";
  }
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
  if (!sameDocument(current, original) && !confirm("Discard edits and restore the loaded snippet?"))
    return;
  load(original);
  if (auto.checked) run();
});
window.addEventListener("beforeunload", (event) => {
  if (!sameDocument(current, original)) {
    event.preventDefault();
    event.returnValue = "";
  }
});
window.addEventListener("pagehide", () => {
  clearTimeout(timer);
  client.stop();
  output.clear();
});
if (import.meta.hot)
  import.meta.hot.dispose(() => {
    clearTimeout(timer);
    client.stop();
    editor.destroy();
    output.clear();
  });
run();

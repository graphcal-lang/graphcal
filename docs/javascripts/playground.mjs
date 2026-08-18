const EXAMPLE_BASE = new URL("../assets/playground/examples/", import.meta.url);
const WORKER_URL = new URL("playground-worker.mjs", import.meta.url);
const EVALUATION_TIMEOUT_MS = 10_000;
const AUTO_RUN_DELAY_MS = 400;

// Vendored Vega bundles (copied from crates/graphcal-report/assets by the
// docs build). Classic UMD scripts, loaded lazily the first time a figure
// must render so text-only sessions never pay for them.
const VEGA_SCRIPT_URLS = ["vega.min.js", "vega-lite.min.js", "vega-embed.min.js"].map(
  (name) => new URL(`../assets/playground/vega/${name}`, import.meta.url),
);
let vegaRuntime;

function loadClassicScript(url) {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = url;
    script.addEventListener("load", resolve);
    script.addEventListener("error", () => reject(new Error(`failed to load ${url}`)));
    document.head.append(script);
  });
}

function ensureVega() {
  vegaRuntime ??= (async () => {
    for (const url of VEGA_SCRIPT_URLS) {
      // Sequential: vega-lite needs vega, vega-embed needs both.
      await loadClassicScript(url);
    }
    if (typeof globalThis.vegaEmbed !== "function") {
      throw new Error("vega-embed did not initialize");
    }
    return globalThis.vegaEmbed;
  })();
  return vegaRuntime;
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

class GraphcalPlayground extends HTMLElement {
  constructor() {
    super();
    this.initialized = false;
    this.files = new Map();
    this.originalFiles = new Map();
    this.entry = "";
    this.currentPath = "";
    this.worker = undefined;
    this.requestId = 0;
    this.activeRequest = undefined;
    this.autoRunTimer = undefined;
    this.timeoutTimer = undefined;
    this.loadId = 0;
  }

  connectedCallback() {
    this.ensureInitialized();
  }

  disconnectedCallback() {
    this.loadId += 1;
    this.stopWorker();
    clearTimeout(this.autoRunTimer);
    clearTimeout(this.timeoutTimer);
  }

  ensureInitialized() {
    if (this.initialized) {
      if (this.files.size > 0) {
        this.run();
      } else {
        this.loadExample();
      }
      return;
    }
    this.initialized = true;
    this.buildShell();
    this.loadExample();
  }

  buildShell() {
    this.classList.add("gc-playground");
    this.setAttribute("aria-label", "Graphcal browser playground");

    const header = element("div", "gc-playground__header");
    const title = element("strong", "gc-playground__title", "Run Graphcal in your browser");
    this.status = element("span", "gc-playground__status", "Loading example…");
    this.status.setAttribute("role", "status");
    header.append(title, this.status);

    this.tabs = element("div", "gc-playground__tabs");
    this.tabs.setAttribute("role", "tablist");
    this.tabs.setAttribute("aria-label", "Project files");

    this.editor = element("textarea", "gc-playground__editor");
    this.editor.setAttribute("aria-label", "Graphcal source editor");
    this.editor.setAttribute("spellcheck", "false");
    this.editor.setAttribute("autocapitalize", "off");
    this.editor.setAttribute("autocomplete", "off");
    this.editor.addEventListener("input", () => this.handleEdit());
    this.editor.addEventListener("keydown", (event) => this.handleEditorKey(event));

    const controls = element("div", "gc-playground__controls");
    this.runButton = element("button", "md-button md-button--primary", "Run");
    this.runButton.type = "button";
    this.runButton.disabled = true;
    this.runButton.addEventListener("click", () => this.run());
    this.resetButton = element("button", "md-button", "Reset");
    this.resetButton.type = "button";
    this.resetButton.disabled = true;
    this.resetButton.addEventListener("click", () => this.reset());
    const shortcut = element("span", "gc-playground__shortcut", "Ctrl/⌘ + Enter");
    controls.append(this.runButton, this.resetButton, shortcut);

    const editorPane = element("div", "gc-playground__editor-pane");
    editorPane.append(this.tabs, this.editor, controls);

    this.output = element("div", "gc-playground__output");
    this.output.setAttribute("aria-live", "polite");
    this.output.append(element("p", "gc-playground__placeholder", "Evaluation output appears here."));

    const layout = element("div", "gc-playground__layout");
    layout.append(editorPane, this.output);

    const scope = element(
      "p",
      "gc-playground__scope",
      "Experimental browser v1 for alpha-stage Graphcal. Local single- and multi-file projects are supported; package dependencies and plugins are not.",
    );

    this.replaceChildren(header, layout, scope);
  }

  async loadExample() {
    const loadId = ++this.loadId;
    const example = this.getAttribute("example");
    if (!example) {
      this.showFatal("This playground does not specify an example.");
      return;
    }

    try {
      const descriptorUrl = new URL(`${example}/project.json`, EXAMPLE_BASE);
      const descriptorResponse = await fetch(descriptorUrl);
      if (!descriptorResponse.ok) {
        throw new Error(`example descriptor returned HTTP ${descriptorResponse.status}`);
      }
      const descriptor = await descriptorResponse.json();
      if (typeof descriptor.entry !== "string" || !Array.isArray(descriptor.files)) {
        throw new Error("example descriptor has an invalid shape");
      }

      const loadedFiles = await Promise.all(
        descriptor.files.map(async (path) => {
          if (typeof path !== "string") throw new Error("example file path is not text");
          const response = await fetch(new URL(path, descriptorUrl));
          if (!response.ok) throw new Error(`${path} returned HTTP ${response.status}`);
          return [path, await response.text()];
        }),
      );

      if (!this.isConnected || loadId !== this.loadId) return;

      this.entry = descriptor.entry;
      this.files = new Map(loadedFiles);
      this.originalFiles = new Map(loadedFiles);
      this.currentPath = this.files.has(this.entry) ? this.entry : loadedFiles[0]?.[0] ?? "";
      this.renderTabs();
      this.selectFile(this.currentPath);
      this.runButton.disabled = false;
      this.resetButton.disabled = false;
      this.setStatus("Ready", "ready");
      this.run();
    } catch (error) {
      if (!this.isConnected || loadId !== this.loadId) return;
      this.showFatal(`Could not load playground example: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  renderTabs() {
    this.tabs.replaceChildren();
    for (const path of this.files.keys()) {
      const label = path === this.entry ? `${path} (entry)` : path;
      const button = element("button", "gc-playground__tab", label);
      button.type = "button";
      button.setAttribute("role", "tab");
      button.dataset.path = path;
      button.setAttribute("aria-selected", String(path === this.currentPath));
      button.addEventListener("click", () => this.selectFile(path));
      this.tabs.append(button);
    }
    this.tabs.hidden = this.files.size <= 1;
  }

  selectFile(path) {
    if (!this.files.has(path)) return;
    this.currentPath = path;
    this.editor.value = this.files.get(path);
    this.editor.setAttribute("aria-label", `Source editor for ${path}`);
    for (const tab of this.tabs.querySelectorAll("[role=tab]")) {
      tab.setAttribute("aria-selected", String(tab.dataset.path === path));
    }
  }

  handleEdit() {
    this.files.set(this.currentPath, this.editor.value);
    clearTimeout(this.autoRunTimer);
    this.autoRunTimer = setTimeout(() => this.run(), AUTO_RUN_DELAY_MS);
  }

  handleEditorKey(event) {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      this.run();
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      const start = this.editor.selectionStart;
      const end = this.editor.selectionEnd;
      this.editor.setRangeText("    ", start, end, "end");
      this.handleEdit();
    }
  }

  reset() {
    this.files = new Map(this.originalFiles);
    this.currentPath = this.files.has(this.entry) ? this.entry : this.files.keys().next().value;
    this.renderTabs();
    this.selectFile(this.currentPath);
    this.run();
  }

  run() {
    if (this.files.size === 0) return;
    clearTimeout(this.autoRunTimer);
    if (this.activeRequest !== undefined) this.stopWorker();
    this.ensureWorker();

    const id = ++this.requestId;
    this.activeRequest = id;
    this.setStatus("Running…", "running");
    this.runButton.textContent = "Running…";

    this.worker.postMessage({
      id,
      request: {
        entry: this.entry,
        files: Array.from(this.files, ([path, content]) => ({ path, content })),
      },
    });

    clearTimeout(this.timeoutTimer);
    this.timeoutTimer = setTimeout(() => {
      if (this.activeRequest !== id) return;
      this.stopWorker();
      this.showFatal("Evaluation exceeded the browser time limit. The worker was stopped; press Run to retry in a fresh worker.");
    }, EVALUATION_TIMEOUT_MS);
  }

  ensureWorker() {
    if (this.worker) return;
    const worker = new Worker(WORKER_URL, { type: "module" });
    this.worker = worker;
    worker.addEventListener("message", (event) => this.handleWorkerMessage(event));
    worker.addEventListener("error", (event) => {
      if (this.worker !== worker) return;
      this.stopWorker();
      this.showFatal(`The Graphcal worker failed: ${event.message || "unknown worker error"}`);
    });
  }

  stopWorker() {
    this.worker?.terminate();
    this.worker = undefined;
    this.activeRequest = undefined;
    clearTimeout(this.timeoutTimer);
    if (this.runButton) this.runButton.textContent = "Run";
  }

  handleWorkerMessage(event) {
    const { id, result, workerError } = event.data;
    if (id !== this.activeRequest) return;
    this.activeRequest = undefined;
    clearTimeout(this.timeoutTimer);
    this.runButton.textContent = "Run";

    if (workerError) {
      this.stopWorker();
      this.showFatal(`Evaluation failed inside WebAssembly: ${workerError}`);
      return;
    }
    this.renderOutcome(result);
  }

  renderOutcome(outcome) {
    this.output.replaceChildren();
    switch (outcome?.status) {
      case "rejected":
        this.setStatus("Project rejected", "error");
        this.output.append(this.renderMessage("error", outcome.error.message));
        break;
      case "compile_error":
        this.setStatus("Compile error", "error");
        outcome.diagnostics.forEach((diagnostic) => this.output.append(this.renderDiagnostic(diagnostic)));
        break;
      case "evaluated":
        this.renderEvaluation(outcome.evaluation);
        break;
      default:
        this.showFatal("The WebAssembly module returned an unknown result.");
    }
  }

  renderEvaluation(evaluation) {
    this.setStatus(evaluation.has_errors ? "Completed with errors" : "Up to date", evaluation.has_errors ? "error" : "success");

    for (const notice of evaluation.notices) {
      const message = notice.kind === "plot_error"
        ? `Plot ${notice.name}: ${notice.message}`
        : notice.message;
      this.output.append(this.renderMessage("notice", message));
    }

    const heading = element("h3", "gc-playground__output-title", "Values");
    this.output.append(heading);
    const values = element("dl", "gc-playground__values");
    for (const declaration of evaluation.values) {
      const name = element("dt", "gc-playground__value-name", declaration.name);
      name.title = declaration.declaration_kind;
      const value = element("dd", "gc-playground__value");
      if (declaration.outcome.status === "value") {
        value.append(this.renderValue(declaration.outcome.value));
      } else {
        value.append(this.renderNodeError(declaration.outcome.error));
      }
      values.append(name, value);
    }
    if (evaluation.values.length === 0) {
      this.output.append(element("p", "gc-playground__placeholder", "No values were produced."));
    } else {
      this.output.append(values);
    }

    if (evaluation.assertions.length > 0) {
      this.output.append(element("h3", "gc-playground__output-title", "Assertions"));
      const assertions = element("ul", "gc-playground__assertions");
      for (const assertion of evaluation.assertions) {
        const item = element("li", `gc-playground__assertion gc-playground__assertion--${assertion.outcome.status}`);
        const message = assertion.outcome.message ? `: ${assertion.outcome.message}` : "";
        const affected = assertion.affected_declarations.length > 0
          ? ` (affected: ${assertion.affected_declarations.join(", ")})`
          : "";
        item.textContent = `${assertion.name}: ${assertion.outcome.status.toUpperCase()}${message}${affected}`;
        assertions.append(item);
      }
      this.output.append(assertions);
    }

    const figures = evaluation.figures ?? [];
    if (figures.length > 0) {
      this.output.append(element("h3", "gc-playground__output-title", "Plots"));
      for (const figure of figures) {
        const container = element("figure", "gc-playground__figure");
        container.append(element("figcaption", "gc-playground__figure-name", figure.name));
        const target = element("div", "gc-playground__figure-view");
        container.append(target);
        this.output.append(container);
        this.renderFigure(target, figure);
      }
    }

    this.output.append(element("p", "gc-playground__version", `Graphcal ${evaluation.compiler_version} · running locally in WebAssembly`));
  }

  renderFigure(target, figure) {
    ensureVega()
      .then((vegaEmbed) => vegaEmbed(target, figure.spec, { actions: false }))
      .catch((error) => {
        target.replaceChildren(
          this.renderMessage("error", `Plot ${figure.name} failed to render: ${error.message ?? error}`),
        );
      });
  }

  renderValue(value) {
    if (value.kind === "struct") {
      const details = element("details", "gc-playground__composite");
      details.open = true;
      details.append(element("summary", "", value.display));
      const fields = element("dl", "gc-playground__nested-values");
      for (const field of value.fields) {
        const fieldName = element("dt", "", field.name);
        const fieldValue = element("dd", "");
        fieldValue.append(this.renderValue(field.value));
        fields.append(fieldName, fieldValue);
      }
      details.append(fields);
      return details;
    }

    if (value.kind === "indexed") {
      const details = element("details", "gc-playground__composite");
      details.open = true;
      details.append(element("summary", "", value.display));
      const table = element("table", "gc-playground__indexed-table");
      const body = document.createElement("tbody");
      for (const entry of value.entries) {
        const row = document.createElement("tr");
        const key = element("th", "", entry.display_key);
        key.scope = "row";
        const cell = document.createElement("td");
        cell.append(this.renderValue(entry.value));
        row.append(key, cell);
        body.append(row);
      }
      table.append(body);
      details.append(table);
      return details;
    }

    return element("span", "gc-playground__scalar", value.display);
  }

  renderNodeError(error) {
    if (error.kind === "dependency_failed") {
      return element("span", "gc-playground__runtime-error", `ERROR: dependency failed (${error.failed_dependencies.join(", ")})`);
    }
    return element("span", "gc-playground__runtime-error", `ERROR: ${error.message}`);
  }

  renderDiagnostic(diagnostic) {
    const card = element("article", "gc-playground__diagnostic");
    const header = element("div", "gc-playground__diagnostic-header");
    header.append(
      element("strong", "", diagnostic.code ?? "Graphcal error"),
      element("span", "", diagnostic.file),
    );
    card.append(header, element("p", "", diagnostic.message));

    for (const label of diagnostic.labels) {
      const line = label.range.start.line + 1;
      const character = label.range.start.character + 1;
      const button = element(
        "button",
        "gc-playground__diagnostic-location",
        `${diagnostic.file}:${line}:${character}${label.message ? ` — ${label.message}` : ""}`,
      );
      button.type = "button";
      button.addEventListener("click", () => this.focusRange(diagnostic.file, label.range));
      card.append(button);
    }
    if (diagnostic.help) card.append(element("p", "gc-playground__diagnostic-help", `Hint: ${diagnostic.help}`));
    return card;
  }

  focusRange(path, range) {
    if (!this.files.has(path)) return;
    this.selectFile(path);
    const start = positionToOffset(this.editor.value, range.start);
    const end = positionToOffset(this.editor.value, range.end);
    this.editor.focus();
    this.editor.setSelectionRange(start, Math.max(start, end));
  }

  renderMessage(kind, message) {
    return element("p", `gc-playground__message gc-playground__message--${kind}`, message);
  }

  setStatus(message, state) {
    this.status.textContent = message;
    this.status.dataset.state = state;
  }

  showFatal(message) {
    this.setStatus("Unavailable", "error");
    this.runButton.textContent = "Run";
    this.output.replaceChildren(this.renderMessage("error", message));
  }
}

function positionToOffset(source, position) {
  let line = 0;
  let lineStart = 0;
  while (line < position.line) {
    const newline = source.indexOf("\n", lineStart);
    if (newline < 0) return source.length;
    lineStart = newline + 1;
    line += 1;
  }
  const newline = source.indexOf("\n", lineStart);
  const lineEnd = newline < 0 ? source.length : newline;
  return Math.min(lineStart + position.character, lineEnd);
}

if (!customElements.get("graphcal-playground")) {
  customElements.define("graphcal-playground", GraphcalPlayground);
}

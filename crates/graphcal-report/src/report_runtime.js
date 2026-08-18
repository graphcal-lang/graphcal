// Graphcal report hydration runtime.
//
// Runs the same compiler/evaluator the CLI uses, compiled to WebAssembly and
// embedded in this page, inside a Web Worker built from a blob URL so the
// single-file artifact works from file:// paths. Readers bind closed typed
// values to entry params (the CLI --set discipline); results patch the static
// baseline in place. If anything here fails, the page loudly says so and the
// static baseline remains valid.
(function () {
  "use strict";

  var DEBOUNCE_MS = 200;
  var EVALUATION_TIMEOUT_MS = 10000;

  function payloadText(id) {
    var el = document.getElementById(id);
    return el ? el.textContent : null;
  }

  var projectText = payloadText("graphcal-project");
  var glueB64 = payloadText("graphcal-engine-glue");
  var wasmB64 = payloadText("graphcal-engine-wasm");
  if (!projectText || !glueB64 || !wasmB64) return;

  var project;
  var baselineBindings;
  try {
    project = JSON.parse(projectText);
    baselineBindings = JSON.parse(payloadText("graphcal-baseline") || "[]");
  } catch (error) {
    return;
  }

  // --- The worker body. Serialized with toString() and appended to the
  // wasm-bindgen no-modules glue, so `wasm_bindgen` is a global here.
  function workerMain() {
    var prepared = null;
    function describeError(error) {
      if (error && error.message) return String(error.message);
      try {
        var text = JSON.stringify(error);
        if (text && text !== "{}") return text;
      } catch (ignored) {
        // fall through to String()
      }
      return String(error);
    }
    self.onmessage = function (event) {
      var msg = event.data;
      try {
        if (msg.type === "init") {
          var binary = atob(msg.wasm);
          var bytes = new Uint8Array(binary.length);
          for (var i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
          Promise.resolve(wasm_bindgen({ module_or_path: bytes }))
            .then(function () {
              prepared = wasm_bindgen.prepareProject(msg.project);
              self.postMessage({ type: "ready", ports: prepared.parameterPorts() });
            })
            .catch(function (error) {
              self.postMessage({ type: "fatal", message: describeError(error) });
            });
        } else if (msg.type === "evaluate") {
          if (!prepared) throw new Error("engine is not prepared");
          self.postMessage({
            type: "result",
            id: msg.id,
            outcome: prepared.evaluateBindings(msg.bindings),
          });
        }
      } catch (error) {
        self.postMessage({ type: "fatal", id: msg.id, message: describeError(error) });
      }
    };
  }

  // --- UI chrome -----------------------------------------------------------
  function element(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  var statusChip = element("div", "hydration-status", "loading engine…");
  document.body.appendChild(statusChip);
  function setStatus(text, state) {
    statusChip.textContent = text;
    statusChip.className = "hydration-status" + (state ? " hydration-status--" + state : "");
  }

  var banner = element("div", "modified-banner");
  banner.hidden = true;
  var bannerText = element("span", "", "Values differ from the as-published baseline.");
  var resetButton = element("button", "modified-banner__reset", "Reset to baseline");
  banner.appendChild(bannerText);
  banner.appendChild(resetButton);
  document.body.insertBefore(banner, document.body.firstChild);

  function fatal(message) {
    setStatus("interactive mode unavailable", "error");
    var main = document.querySelector("main");
    if (!main) return;
    var notice = element(
      "p",
      "error-chip",
      "Interactive mode unavailable: " + message + " — the page shows the as-published baseline.",
    );
    main.insertBefore(notice, main.firstChild);
  }

  // --- Controls ------------------------------------------------------------
  var controls = new Map();

  function selectorEscape(name) {
    if (window.CSS && CSS.escape) return CSS.escape(name);
    return name.replace(/"/g, '\\"');
  }

  function cardFor(name) {
    return document.querySelector('#inputs [data-decl="' + selectorEscape(name) + '"]');
  }

  function baselineExprFor(port) {
    for (var i = 0; i < baselineBindings.length; i += 1) {
      if (baselineBindings[i].name === port.name) return baselineBindings[i].expr;
    }
    // The static card body shows the baseline value in full literal syntax
    // (value plus canonical unit label), which is a valid closed expression.
    var card = cardFor(port.name);
    var value = card ? card.querySelector('[data-role="value"]') : null;
    return value ? value.textContent.trim() : "";
  }

  function makeControl(port) {
    var card = cardFor(port.name);
    if (!card) return null;
    var holder = element("div", "control");
    var errorLine = element("p", "control-error");
    errorLine.hidden = true;

    var initialExpr = baselineExprFor(port);
    var control = {
      name: port.name,
      initialExpr: initialExpr,
      currentExpr: initialExpr,
      setSi: function () {},
      setError: function (message) {
        errorLine.textContent = message;
        errorLine.hidden = !message;
      },
      restore: null,
    };

    function commit(expr) {
      control.currentExpr = expr;
      scheduleEvaluate();
    }

    var kind = port.control.kind;
    if (kind === "boolean") {
      var checkbox = element("input");
      checkbox.type = "checkbox";
      checkbox.checked = initialExpr.trim() === "true";
      checkbox.addEventListener("change", function () {
        commit(checkbox.checked ? "true" : "false");
      });
      control.restore = function () {
        checkbox.checked = control.initialExpr.trim() === "true";
      };
      var toggleLabel = element("label", "control-toggle");
      toggleLabel.appendChild(checkbox);
      toggleLabel.appendChild(document.createTextNode(" enabled"));
      holder.appendChild(toggleLabel);
    } else if (kind === "select") {
      var select = element("select", "control-select");
      for (var i = 0; i < port.control.variants.length; i += 1) {
        var option = element("option", "", port.control.variants[i]);
        option.value = port.control.index + "." + port.control.variants[i];
        select.appendChild(option);
      }
      select.value = initialExpr.trim();
      select.addEventListener("change", function () {
        commit(select.value);
      });
      control.restore = function () {
        select.value = control.initialExpr.trim();
      };
      holder.appendChild(select);
    } else {
      var field = element("input", "control-field");
      field.type = "text";
      field.value = initialExpr;
      field.spellcheck = false;
      field.addEventListener("input", function () {
        commit(field.value);
      });
      control.restore = function () {
        field.value = control.initialExpr;
      };
      holder.appendChild(field);

      var isBoundedQuantity =
        kind === "quantity" &&
        typeof port.control.lower_si === "number" &&
        typeof port.control.upper_si === "number" &&
        port.control.upper_si > port.control.lower_si;
      var isBoundedInteger =
        kind === "integer" &&
        typeof port.control.lower === "number" &&
        typeof port.control.upper === "number" &&
        port.control.upper > port.control.lower;
      if (isBoundedQuantity || isBoundedInteger) {
        var slider = element("input", "control-slider");
        slider.type = "range";
        var lower = isBoundedQuantity ? port.control.lower_si : port.control.lower;
        var upper = isBoundedQuantity ? port.control.upper_si : port.control.upper;
        slider.min = String(lower);
        slider.max = String(upper);
        slider.step = isBoundedQuantity ? String((upper - lower) / 200) : "1";
        var unitSuffix =
          isBoundedQuantity && port.control.unit ? " " + port.control.unit : "";
        slider.addEventListener("input", function () {
          var expr = slider.value + unitSuffix;
          field.value = expr;
          commit(expr);
        });
        control.setSi = function (si) {
          if (Number.isFinite(si)) slider.value = String(si);
        };
        holder.appendChild(slider);
      }
    }

    holder.appendChild(errorLine);
    card.appendChild(holder);
    return control;
  }

  function buildControls(ports) {
    for (var i = 0; i < ports.length; i += 1) {
      var control = makeControl(ports[i]);
      if (control) controls.set(control.name, control);
    }
    resetButton.addEventListener("click", function () {
      controls.forEach(function (control) {
        control.currentExpr = control.initialExpr;
        control.setError("");
        if (control.restore) control.restore();
      });
      scheduleEvaluate();
    });
  }

  function currentBindings() {
    var bindings = [];
    controls.forEach(function (control) {
      var expr = control.currentExpr.trim();
      if (expr) bindings.push({ name: control.name, expr: expr });
    });
    return bindings;
  }

  function anyModified() {
    var modified = false;
    controls.forEach(function (control) {
      if (control.currentExpr.trim() !== control.initialExpr.trim()) modified = true;
    });
    return modified;
  }

  // --- Result rendering ----------------------------------------------------
  function viewDepth(view) {
    if (view.kind !== "indexed") return 0;
    var first = view.entries.length > 0 ? view.entries[0].value : null;
    return 1 + (first ? viewDepth(first) : 0);
  }

  function flattenView(prefix, view, out) {
    if (view.kind === "struct" && view.fields.length > 0) {
      for (var i = 0; i < view.fields.length; i += 1) {
        flattenView(prefix + "." + view.fields[i].name, view.fields[i].value, out);
      }
    } else if (view.kind === "indexed") {
      for (var j = 0; j < view.entries.length; j += 1) {
        var entry = view.entries[j];
        flattenView(prefix + "[" + entry.display_key + "]", entry.value, out);
      }
    } else {
      out.push([prefix, view.display]);
    }
  }

  function buildEntriesTable(entries) {
    var table = element("table", "entries");
    table.setAttribute("data-role", "value");
    var body = document.createElement("tbody");
    for (var i = 0; i < entries.length; i += 1) {
      var row = document.createElement("tr");
      var th = document.createElement("th");
      th.scope = "row";
      th.appendChild(element("code", "", entries[i][0]));
      var td = document.createElement("td");
      td.textContent = entries[i][1];
      row.appendChild(th);
      row.appendChild(td);
      body.appendChild(row);
    }
    table.appendChild(body);
    return table;
  }

  function buildGrid(view) {
    var columns = [];
    var rows = [];
    for (var i = 0; i < view.entries.length; i += 1) {
      var outer = view.entries[i];
      var cells = {};
      var inner = outer.value;
      if (inner.kind === "indexed") {
        for (var j = 0; j < inner.entries.length; j += 1) {
          var cell = inner.entries[j];
          if (columns.indexOf(cell.display_key) < 0) columns.push(cell.display_key);
          cells[cell.display_key] = cell.value.display;
        }
      }
      rows.push([outer.display_key, cells]);
    }
    var table = element("table", "grid");
    var thead = document.createElement("thead");
    var headRow = document.createElement("tr");
    headRow.appendChild(document.createElement("th"));
    for (var c = 0; c < columns.length; c += 1) {
      var th = document.createElement("th");
      th.scope = "col";
      th.textContent = columns[c];
      headRow.appendChild(th);
    }
    thead.appendChild(headRow);
    table.appendChild(thead);
    var tbody = document.createElement("tbody");
    for (var r = 0; r < rows.length; r += 1) {
      var tr = document.createElement("tr");
      var rowTh = document.createElement("th");
      rowTh.scope = "row";
      rowTh.textContent = rows[r][0];
      tr.appendChild(rowTh);
      for (var c2 = 0; c2 < columns.length; c2 += 1) {
        var td = document.createElement("td");
        td.textContent = rows[r][1][columns[c2]] || "";
        tr.appendChild(td);
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    return table;
  }

  function renderView(view) {
    var depth = viewDepth(view);
    if (depth === 2) {
      var grid = buildGrid(view);
      grid.setAttribute("data-role", "value");
      return grid;
    }
    if (depth === 1 || (view.kind === "struct" && view.fields.length > 0)) {
      var entries = [];
      flattenView("", view, entries);
      return buildEntriesTable(entries);
    }
    if (depth >= 3) {
      var slices = element("div", "slices");
      slices.setAttribute("data-role", "value");
      for (var i = 0; i < view.entries.length; i += 1) {
        slices.appendChild(element("h4", "slice-label", "[" + view.entries[i].display_key + "]"));
        slices.appendChild(buildGrid(view.entries[i].value));
      }
      return slices;
    }
    var scalar = element("p", "card-value", view.display);
    scalar.setAttribute("data-role", "value");
    return scalar;
  }

  function patchValues(evaluation) {
    for (var i = 0; i < evaluation.values.length; i += 1) {
      var declaration = evaluation.values[i];
      var card = document.querySelector('[data-decl="' + selectorEscape(declaration.name) + '"]');
      if (!card) continue;
      var slot = card.querySelector('[data-role="value"]');
      if (!slot) continue;
      var replacement;
      if (declaration.outcome.status === "value") {
        replacement = renderView(declaration.outcome.value);
      } else {
        var error = declaration.outcome.error;
        var message =
          error.kind === "dependency_failed"
            ? "dependency failed: " + error.failed_dependencies.join(", ")
            : error.message;
        replacement = element("p", "error-chip", "ERROR: " + message);
        replacement.setAttribute("data-role", "value");
      }
      slot.replaceWith(replacement);
    }
  }

  function patchParamControls(evaluation) {
    for (var i = 0; i < evaluation.values.length; i += 1) {
      var declaration = evaluation.values[i];
      if (declaration.declaration_kind !== "param") continue;
      var control = controls.get(declaration.name);
      if (!control) continue;
      if (declaration.outcome.status === "value" && declaration.outcome.value.kind === "quantity") {
        control.setSi(declaration.outcome.value.si_value);
      }
    }
  }

  function patchChecks(evaluation) {
    for (var i = 0; i < evaluation.assertions.length; i += 1) {
      var assertion = evaluation.assertions[i];
      var item = document.querySelector('[data-check="' + selectorEscape(assertion.name) + '"]');
      if (!item) continue;
      var status = assertion.outcome.status;
      item.className = "check check--" + status;
      var badge = item.querySelector(".badge");
      if (badge) badge.textContent = status.toUpperCase();
      var messageSpan = item.querySelector(".check-message");
      if (assertion.outcome.message) {
        if (!messageSpan) {
          messageSpan = element("span", "check-message");
          item.appendChild(document.createTextNode(" "));
          item.appendChild(messageSpan);
        }
        messageSpan.textContent = assertion.outcome.message;
      } else if (messageSpan) {
        messageSpan.textContent = "";
      }
    }
  }

  function patchFigures(evaluation) {
    if (typeof window.vegaEmbed !== "function") return;
    for (var i = 0; i < evaluation.figures.length; i += 1) {
      var figure = evaluation.figures[i];
      var holder = document.querySelector(
        'figure[data-figure="' + selectorEscape(figure.name) + '"] div[id]',
      );
      if (holder) {
        window.vegaEmbed(holder, figure.spec, { actions: false }).catch(function () {});
      }
    }
  }

  function clearBindingErrors() {
    controls.forEach(function (control) {
      control.setError("");
    });
  }

  function applyOutcome(outcome) {
    if (outcome.status === "evaluated") {
      clearBindingErrors();
      patchValues(outcome.evaluation);
      patchParamControls(outcome.evaluation);
      patchChecks(outcome.evaluation);
      patchFigures(outcome.evaluation);
      setStatus(
        outcome.evaluation.has_errors ? "live · evaluation has errors" : "live",
        outcome.evaluation.has_errors ? "warn" : "ok",
      );
    } else if (outcome.status === "binding_errors") {
      clearBindingErrors();
      for (var i = 0; i < outcome.errors.length; i += 1) {
        var bindingError = outcome.errors[i];
        var control = controls.get(bindingError.name);
        if (control) control.setError(bindingError.message);
      }
      setStatus("input rejected", "warn");
    } else {
      setStatus("evaluation failed: " + outcome.message, "error");
    }
    banner.hidden = !anyModified();
  }

  // --- Worker loop ---------------------------------------------------------
  var worker = null;
  var workerUrl = null;
  var requestId = 0;
  var activeRequest = null;
  var timeoutTimer = null;
  var debounceTimer = null;
  var evaluateQueued = false;
  var ready = false;

  function scheduleEvaluate() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(runEvaluate, DEBOUNCE_MS);
  }

  function runEvaluate() {
    if (!ready) {
      evaluateQueued = true;
      return;
    }
    if (activeRequest !== null) {
      evaluateQueued = true;
      return;
    }
    requestId += 1;
    activeRequest = requestId;
    setStatus("computing…", "busy");
    timeoutTimer = setTimeout(function () {
      // Cancellation is worker teardown: a blocked evaluation cannot be
      // interrupted, so replace the whole engine and re-prepare.
      worker.terminate();
      activeRequest = null;
      ready = false;
      evaluateQueued = true;
      setStatus("evaluation timed out · restarting engine", "warn");
      startWorker();
    }, EVALUATION_TIMEOUT_MS);
    worker.postMessage({ type: "evaluate", id: requestId, bindings: currentBindings() });
  }

  function handleMessage(event) {
    var msg = event.data;
    if (msg.type === "ready") {
      ready = true;
      if (controls.size === 0) buildControls(msg.ports);
      setStatus("live", "ok");
      // Verify the baseline (and pick up slider positions) with one initial
      // evaluation over the embedded baseline bindings.
      evaluateQueued = false;
      runEvaluate();
      return;
    }
    if (msg.type === "result") {
      if (msg.id !== activeRequest) return;
      clearTimeout(timeoutTimer);
      activeRequest = null;
      applyOutcome(msg.outcome);
      if (evaluateQueued) {
        evaluateQueued = false;
        runEvaluate();
      }
      return;
    }
    if (msg.type === "fatal") {
      clearTimeout(timeoutTimer);
      activeRequest = null;
      fatal(msg.message);
    }
  }

  function startWorker() {
    try {
      worker = new Worker(workerUrl);
    } catch (error) {
      fatal("this browser refused to start the report engine (" + error + ")");
      return;
    }
    worker.onmessage = handleMessage;
    worker.onerror = function (event) {
      fatal(event.message || "the report engine crashed");
    };
    worker.postMessage({ type: "init", project: project, wasm: wasmB64 });
    setStatus("preparing model…", "busy");
  }

  try {
    var glueSource = atob(glueB64);
    var driverSource = "\n(" + workerMain.toString() + ")();\n";
    workerUrl = URL.createObjectURL(
      new Blob([glueSource, driverSource], { type: "text/javascript" }),
    );
  } catch (error) {
    fatal("could not assemble the report engine (" + error + ")");
    return;
  }
  startWorker();
})();

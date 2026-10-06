// Graphcal report UI runtime.
//
// Owns report controls, result patching, debounce, and transport restart. The
// evaluator is supplied by the caller through a worker-like transport factory,
// so this runtime does not depend on embedded WebAssembly.
(function (global) {
  "use strict";

  var DEBOUNCE_MS = 200;
  var AUTO_RUN_DELAY_MS = 300;
  var EVALUATION_TIMEOUT_MS = 10000;

  function mount(options) {
    if (!options || typeof options.createTransport !== "function") {
      throw new Error("GraphcalReport.mount requires a createTransport factory");
    }
    var baselineBindings = options.baselineBindings || [];
    var formState = global.GraphcalReportFormState;
    if (!formState) throw new Error("Graphcal report form state is unavailable");
    var axisLabels = formState.axisLabels;
    var emptyDraft = formState.emptyDraft;
    var draftFromView = formState.draftFromView;
    var cloneStructured = formState.clone;
    var incompleteDraft = formState.incomplete;

  // --- UI chrome -----------------------------------------------------------
  function element(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  // Header title block: the report's verification state at a glance, the way a
  // drawing's title block carries its revision and sign-off.
  function titleCell(tag, modifier, label) {
    var cell = element(tag, "title-cell title-cell--" + modifier);
    if (tag === "button") cell.type = "button";
    var value = element("span", "title-cell__value");
    cell.append(element("span", "title-cell__label", label), value);
    return { cell: cell, value: value };
  }
  var titleBlock = element("div", "title-block");
  var checksCell = titleCell("button", "checks", "Checks");
  checksCell.cell.hidden = true;
  var checksText = element("span");
  checksCell.value.append(element("span", "status-dot"), checksText);
  var inputsCell = titleCell("div", "inputs", "Inputs");
  var publishedText = element("span", "", "As published");
  var engineCell = titleCell("div", "engine", "Engine");
  titleBlock.append(checksCell.cell, inputsCell.cell, engineCell.cell);
  var reportHeader = document.querySelector("main > header");
  if (reportHeader) {
    reportHeader.classList.add("report-header");
    reportHeader.appendChild(titleBlock);
  }

  var statusChip = element("span", "hydration-status", "Loading engine…");
  statusChip.setAttribute("role", "status");
  statusChip.setAttribute("aria-live", "polite");
  if (reportHeader) engineCell.value.append(element("span", "status-dot"), statusChip);
  else document.body.appendChild(statusChip);
  // `stale` means the visible results no longer match the inputs on screen.
  function setStatus(text, state, stale) {
    statusChip.textContent = text;
    statusChip.title = text;
    statusChip.className = "hydration-status" + (state ? " hydration-status--" + state : "");
    engineCell.cell.className = "title-cell title-cell--engine" + (state ? " is-" + state : "");
    document.body.classList.toggle("results-stale", Boolean(stale));
  }

  var banner = element("div", "modified-banner");
  banner.hidden = true;
  var bannerText = element("span", "", "Values differ from the as-published baseline.");
  var resetButton = element("button", "modified-banner__reset", "Reset");
  resetButton.type = "button";
  resetButton.title = "Return every input to its published value";
  banner.appendChild(bannerText);
  banner.appendChild(resetButton);
  if (reportHeader) inputsCell.value.append(publishedText, banner);
  else document.body.insertBefore(banner, document.body.firstChild);

  // Summarize from the rendered checks so the header and the Checks tab agree.
  function updateChecksSummary() {
    var checks = document.querySelectorAll(".check[data-check]");
    var failing = document.querySelectorAll(".check--fail, .check--error, .check--blocked").length;
    checksCell.cell.hidden = checks.length === 0;
    checksText.textContent = failing
      ? failing + " of " + checks.length + " failing"
      : checks.length === 1 ? "1 passes" : "All " + checks.length + " pass";
    checksCell.cell.classList.toggle("title-cell--fail", failing > 0);
    checksCell.cell.classList.toggle("title-cell--pass", failing === 0);
    checksCell.cell.title = "Show checks";
  }
  checksCell.cell.addEventListener("click", function () {
    if (!workspace || !workspace.showResults("checks")) {
      var section = document.getElementById("checks");
      if (section) section.scrollIntoView({ block: "start" });
      return;
    }
    var results = document.getElementById("workspace-results");
    if (results && results.getBoundingClientRect().top > window.innerHeight * 0.5) results.scrollIntoView({ block: "start" });
  });
  updateChecksSummary();

  var autoRun = true;
  var autoRunToggle = element("input");
  autoRunToggle.type = "checkbox";
  autoRunToggle.checked = true;
  autoRunToggle.setAttribute("aria-describedby", "auto-run-description");
  var autoRunLabel = element("label", "auto-run-toggle");
  autoRunLabel.appendChild(autoRunToggle);
  autoRunLabel.appendChild(document.createTextNode("Auto run"));
  autoRunLabel.title = "Recalculate after a short pause once an edit is complete";
  var autoRunDescription = element(
    "span",
    "auto-run-description",
    "Runs complete edits after a short pause.",
  );
  autoRunDescription.id = "auto-run-description";
  // With auto run off, edits wait here until applied together.
  var applyAll = element("button", "apply-all", "Apply edits");
  applyAll.type = "button";
  applyAll.hidden = true;
  applyAll.disabled = true;
  applyAll.addEventListener("click", function () {
    controls.forEach(function (control) { control.submitIfDirty(); });
  });
  function updateApplyAll() {
    var waiting = 0;
    controls.forEach(function (control) { if (control.isDirty()) waiting += 1; });
    applyAll.hidden = autoRun;
    applyAll.disabled = waiting === 0;
    applyAll.textContent = waiting > 1 ? "Apply " + waiting + " edits" : "Apply edits";
  }
  var inputToolbar = element("div", "input-toolbar");
  inputToolbar.appendChild(autoRunLabel);
  inputToolbar.appendChild(applyAll);
  inputToolbar.appendChild(autoRunDescription);
  var inputsSection = document.getElementById("inputs");
  if (inputsSection) {
    var inputCards = inputsSection.querySelector ? inputsSection.querySelector(".cards") : null;
    inputsSection.insertBefore(inputToolbar, inputCards);
  }

  // Keep the static document intact until the engine has verified the baseline.
  var workspace = null;
  var parameterDescriptions = new Map();

  function registerField(port, widget, kind, labels, path, identity, constructorControl, extras) {
    if (workspace) workspace.field({
      parameter: port.name,
      widget: widget,
      kind: kind,
      labels: labels,
      path: path,
      identity: identity,
      constructorControl: Boolean(constructorControl),
      description: parameterDescriptions.get(port.name) || "",
      hint: extras && extras.hint ? extras.hint : "",
      slider: extras && extras.slider ? extras.slider : null,
      unit: extras && extras.unit ? extras.unit : "",
      unitTitle: extras && extras.unitTitle ? extras.unitTitle : "",
    });
  }

  // Describe a root parameter's declared domain. Quantity bounds and `unit` are
  // both canonical SI from the engine, so they are shown together unconverted.
  function controlHint(control) {
    if (!control) return "";
    function between(lower, upper, unit) {
      var suffix = unit ? " " + unit : "";
      if (lower !== null && upper !== null) return lower + " to " + upper + suffix;
      if (lower !== null) return "at least " + lower + suffix;
      if (upper !== null) return "at most " + upper + suffix;
      return "";
    }
    if (control.kind === "quantity") {
      return between(
        typeof control.lower_si === "number" ? String(control.lower_si) : null,
        typeof control.upper_si === "number" ? String(control.upper_si) : null,
        control.unit || "",
      );
    }
    if (control.kind === "integer") {
      var range = between(
        typeof control.lower === "string" ? control.lower : null,
        typeof control.upper === "string" ? control.upper : null,
        "",
      );
      return range ? "integer, " + range : "integer";
    }
    if (control.kind === "datetime") return "datetime, " + control.time_scale;
    return "";
  }

  function fatal(message) {
    setStatus("Interactive mode unavailable", "error");
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

  // Format a number as a valid Graphcal real literal: integral values need
  // an explicit decimal point (`1200 kg` is not a quantity literal,
  // `1200.0 kg` is).
  function numberLiteral(value) {
    var text = String(value);
    return /^-?[0-9]+$/.test(text) ? text + ".0" : text;
  }

  // Project evaluated leaves into editable Graphcal literals. Container syntax
  // is never assembled in JavaScript: structured values cross the boundary as
  // tagged data and Rust constructs the checked AST.
  function namedKeyLiteral(index, variant) {
    return index + "#" + variant;
  }

  function baselineBindingFor(name) {
    for (var i = 0; i < baselineBindings.length; i += 1) {
      if (baselineBindings[i].name === name) return baselineBindings[i];
    }
    return null;
  }

  function definitionFor(port, id) {
    for (var i = 0; i < port.definitions.length; i += 1) {
      if (port.definitions[i].id === id) return port.definitions[i];
    }
    return null;
  }

  function appendLeafEditor(holder, port, schema, draft, commit, label, root, labels, path, identity) {
    if (schema.kind === "boolean") {
      var checkbox = element("input");
      checkbox.type = "checkbox";
      checkbox.setAttribute("aria-label", label);
      checkbox.checked = draft.expr.trim() === "true";
      checkbox.indeterminate = !draft.expr.trim();
      checkbox.addEventListener("change", function () {
        checkbox.indeterminate = false;
        draft.expr = checkbox.checked ? "true" : "false";
        commit();
      });
      var toggleLabel = element("label", "control-toggle");
      toggleLabel.appendChild(checkbox);
      toggleLabel.appendChild(document.createTextNode(" enabled"));
      holder.appendChild(toggleLabel);
      registerField(port, checkbox, "boolean", labels, path, identity);
      return;
    }
    if (schema.kind === "key" && schema.axis.kind === "named") {
      var select = element("select", "control-select");
      select.setAttribute("aria-label", label);
      schema.axis.variants.forEach(function (variant) {
        var option = element("option", "", variant);
        option.value = namedKeyLiteral(schema.axis.name, variant);
        select.appendChild(option);
      });
      select.value = draft.expr.trim();
      select.addEventListener("change", function () { draft.expr = select.value; commit(); });
      holder.appendChild(select);
      registerField(port, select, "select", labels, path, identity);
      return;
    }
    var field = element("input", "control-field");
    field.type = "text";
    field.setAttribute("aria-label", label);
    field.value = draft.expr;
    field.placeholder = "closed value literal";
    field.spellcheck = false;
    var slider = null;
    field.addEventListener("input", function () {
      draft.expr = field.value;
      if (slider) syncSlider();
      commit();
    });
    holder.appendChild(field);

    var control = root ? port.control : null;
    var integerLower = control && typeof control.lower === "string" ? Number(control.lower) : NaN;
    var integerUpper = control && typeof control.upper === "string" ? Number(control.upper) : NaN;
    var boundedQuantity = control && control.kind === "quantity" &&
      typeof control.lower_si === "number" && typeof control.upper_si === "number" &&
      control.upper_si > control.lower_si;
    var boundedInteger = control && control.kind === "integer" &&
      Number.isSafeInteger(integerLower) && Number.isSafeInteger(integerUpper) &&
      Number.isSafeInteger(integerUpper - integerLower) && integerUpper > integerLower;
    // Move the slider only when the typed literal is a plain number in the
    // slider's own (canonical SI) unit; any other spelling leaves it untouched
    // rather than guessing a conversion.
    // An unplaceable value hides the thumb: a range input would otherwise show
    // its midpoint (or a snapped number) as if it were the current value.
    function syncSlider() {
      var match = /^\s*(-?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s*(.*?)\s*$/.exec(field.value);
      var expectedUnit = boundedQuantity && control.unit ? control.unit : "";
      var placed = Boolean(match) && match[2] === expectedUnit;
      // Unplaced sliders rest at their minimum so the track shows no filled portion.
      slider.value = placed ? match[1] : slider.min;
      slider.classList.toggle("is-unplaced", !placed);
      slider.title = placed
        ? ""
        : "The slider can only show values written in " + (expectedUnit || "plain numbers") + ". Dragging replaces this value.";
    }
    if (boundedQuantity || boundedInteger) {
      slider = element("input", "control-slider");
      slider.type = "range";
      slider.setAttribute("aria-label", label + " slider");
      slider.min = String(boundedQuantity ? control.lower_si : integerLower);
      slider.max = String(boundedQuantity ? control.upper_si : integerUpper);
      slider.step = boundedQuantity ? String((control.upper_si - control.lower_si) / 200) : "1";
      syncSlider();
      slider.addEventListener("input", function () {
        draft.expr = boundedQuantity
          ? numberLiteral(Number(slider.value)) + (control.unit ? " " + control.unit : "")
          : slider.value;
        field.value = draft.expr;
        slider.classList.remove("is-unplaced");
        slider.title = "";
        commit();
      });
      holder.appendChild(slider);
    }
    // The canonical SI unit (or [-] for dimensionless) is the field's dimensional
    // cue. It is not a display unit: any unit of the same dimension is accepted.
    var quantity = schema.kind === "quantity" || schema.kind === "complex";
    registerField(port, field, "literal", labels, path, identity, false, {
      hint: root ? controlHint(port.control) : "",
      slider: slider,
      unit: quantity ? "[" + (schema.unit || "-") + "]" : "",
      unitTitle: quantity
        ? schema.unit
          ? "Dimension of " + schema.unit + " (canonical SI unit). Any unit of this dimension is accepted."
          : "Dimensionless"
        : "",
    });
  }

  function appendStructuredEditor(holder, port, schema, draft, commit, label, root, path, labels, identity) {
    labels = labels || [port.name];
    identity = identity || [];
    if (schema.kind === "algebraic") {
      var definition = definitionFor(port, schema.definition);
      if (!definition) throw new Error("missing algebraic editor definition");
      var constructorDrafts = {};
      if (draft.kind === "algebraic") constructorDrafts[draft.constructor] = draft.fields;
      if (draft.kind === "missing" && definition.constructors.length === 1) {
        draft.kind = "algebraic";
        draft.definition = schema.definition;
        draft.constructor = definition.constructors[0].id;
        draft.fields = definition.constructors[0].fields.map(function (field) {
          return emptyDraft(port, field.schema);
        });
        constructorDrafts[draft.constructor] = draft.fields;
      }
      var fieldsHolder = element("div", "control-structure");
      function renderFields() {
        fieldsHolder.replaceChildren();
        if (draft.kind !== "algebraic") return;
        var constructor = definition.constructors.find(function (candidate) {
          return candidate.id === draft.constructor;
        });
        if (!constructor) return;
        constructor.fields.forEach(function (fieldSchema, index) {
          var fieldset = element("fieldset", "control-fieldset");
          fieldset.appendChild(element("legend", "control-legend", fieldSchema.name));
          var fieldPath = path.concat([{ kind: "field", index: index }]);
          fieldset.setAttribute("data-binding-path", JSON.stringify(fieldPath));
          appendStructuredEditor(
            fieldset, port, fieldSchema.schema, draft.fields[index], commit,
            label + " " + fieldSchema.name, false, fieldPath,
            labels.concat([fieldSchema.name]),
            identity.concat([{ kind: "constructor", definition: schema.definition, index: constructor.id }, { kind: "field", index: index }])
          );
          fieldsHolder.appendChild(fieldset);
        });
      }
      if (definition.constructors.length > 1 || definition.constructors[0].fields.length === 0) {
        var select = element("select", "control-select control-constructor");
        select.setAttribute("aria-label", label + " constructor");
        var placeholder = element("option", "", "Select constructor…");
        placeholder.value = "";
        placeholder.disabled = true;
        select.appendChild(placeholder);
        definition.constructors.forEach(function (constructor) {
          var option = element("option", "", constructor.name);
          option.value = String(constructor.id);
          select.appendChild(option);
        });
        select.value = draft.kind === "algebraic" ? String(draft.constructor) : "";
        select.addEventListener("change", function () {
          var constructor = definition.constructors.find(function (candidate) {
            return candidate.id === Number(select.value);
          });
          if (!constructor) return;
          if (draft.kind === "algebraic") constructorDrafts[draft.constructor] = draft.fields;
          var fields = constructorDrafts[constructor.id] || constructor.fields.map(function (field) {
            return emptyDraft(port, field.schema);
          });
          draft.kind = "algebraic";
          draft.definition = schema.definition;
          draft.constructor = constructor.id;
          draft.fields = fields;
          constructorDrafts[constructor.id] = fields;
          renderFields();
          commit();
        });
        holder.appendChild(select);
        registerField(port, select, "select", labels.concat(["constructor"]), path,
          identity.concat([{ kind: "selector" }]), true);
      }
      holder.appendChild(fieldsHolder);
      renderFields();
      return;
    }
    if (schema.kind === "indexed") {
      var entryLabels = axisLabels(schema.axis);
      var tabs = element("div", "control-indexed");
      var rendered = 0;
      function renderPage() {
        var end = Math.min(entryLabels.length, rendered + 32);
        for (var index = rendered; index < end; index += 1) {
          var entryLabel = entryLabels[index];
          var fieldset = element("fieldset", "control-fieldset control-index-entry");
          fieldset.appendChild(element("legend", "control-legend", entryLabel));
          var entryPath = path.concat([{ kind: "entry", index: index }]);
          fieldset.setAttribute("data-binding-path", JSON.stringify(entryPath));
          appendStructuredEditor(
            fieldset, port, schema.element, draft.entries[index], commit,
            label + " " + entryLabel, false, entryPath,
            labels.concat([entryLabel]), identity.concat([{ kind: "entry", index: index }])
          );
          tabs.appendChild(fieldset);
        }
        rendered = end;
        if (more) more.hidden = rendered >= entryLabels.length;
      }
      var more = element("button", "control-more", "Show more entries");
      more.type = "button";
      more.addEventListener("click", function () {
        renderPage();
        if (workspace) workspace.refresh();
      });
      renderPage();
      tabs.appendChild(more);
      holder.appendChild(tabs);
      if (workspace) workspace.loader(more);
      return;
    }
    appendLeafEditor(holder, port, schema, draft, commit, label, root, labels, path, identity);
  }

  var errorId = 0;

  function makeControl(port, paramView) {
    var card = cardFor(port.name);
    if (!card) return null;
    card.className += " card--interactive";
    var description = card.querySelector ? card.querySelector(".card-doc") : null;
    parameterDescriptions.set(port.name, description ? description.textContent : "");
    var holder = element("div", "control");
    var editorHolder = element("div", "control-editor");
    var errorLine = element("p", "control-error");
    errorLine.hidden = true;
    var draftStatus = element("p", "control-draft-status");
    draftStatus.setAttribute("aria-live", "polite");
    var initialBinding = baselineBindingFor(port.name);
    var initialDraft = draftFromView(port, port.schema, paramView) || emptyDraft(port, port.schema);
    var draft = cloneStructured(initialDraft);
    var startingDraft = cloneStructured(initialDraft);
    var latestDefaultDraft = cloneStructured(initialDraft);
    var rawDraft = initialBinding && typeof initialBinding.expr === "string" ? initialBinding.expr : "";
    var startingRawDraft = rawDraft;
    var mode = "form";
    var dirty = false;
    var sourceChanged = false;
    var pending = false;
    var pendingBinding = null;
    var pendingMode = null;
    var pendingDraft = null;
    var revision = 0;
    var pendingRevision = 0;
    var submittedRequest = null;
    var autoRunTimer = null;
    var rawHolder = element("div", "control-raw");
    var rawField = element("textarea", "control-raw-field");
    rawField.setAttribute("aria-label", port.name + " complete closed value");
    rawField.placeholder = "complete closed Graphcal value";
    rawField.spellcheck = false;
    rawHolder.appendChild(rawField);

    var control = {
      name: port.name,
      schema: port.schema,
      initialBinding: initialBinding,
      currentBinding: initialBinding,
      bindingForEvaluation: function () { return pending ? pendingBinding : control.currentBinding; },
      setError: function (message, path) {
        errorLine.textContent = message;
        errorLine.hidden = !message;
        if (workspace) workspace.error(port.name, message, path);
        if (!holder.querySelectorAll) return;
        holder.querySelectorAll(".control-error--nested").forEach(function (node) { node.remove(); });
        if (!message || !path || !path.length) return;
        var encoded = JSON.stringify(path);
        var target = Array.from(holder.querySelectorAll("[data-binding-path]")).find(function (node) {
          return node.getAttribute("data-binding-path") === encoded;
        });
        if (!target) return;
        errorLine.hidden = true;
        var nested = element("p", "control-error control-error--nested", message);
        errorId += 1;
        nested.id = "binding-error-" + errorId;
        target.appendChild(nested);
        if (target.querySelector) {
          var input = target.querySelector("input, select, textarea");
          if (input) input.setAttribute("aria-describedby", nested.id);
        }
      },
      showView: function (view) {
        var next = draftFromView(port, port.schema, view);
        if (!next) return;
        if (control.currentBinding === null) latestDefaultDraft = cloneStructured(next);
        if (dirty) {
          if (control.currentBinding === null) sourceChanged = true;
          updateDraftStatus();
          return;
        }
        if (control.currentBinding === null || typeof control.currentBinding.expr === "string") {
          draft = next;
          startingDraft = cloneStructured(next);
          render();
        }
      },
      markSubmitted: function (id) { if (pending) submittedRequest = id; },
      acceptPending: function (id) {
        if (!pending || submittedRequest !== id) return;
        control.currentBinding = pendingBinding;
        pending = false;
        if (pendingMode === "form" && pendingDraft) startingDraft = cloneStructured(pendingDraft);
        if (pendingMode === "raw") startingRawDraft = pendingDraft;
        dirty = revision !== pendingRevision;
        sourceChanged = false;
        pendingMode = null;
        pendingDraft = null;
        submittedRequest = null;
        updateDraftStatus();
      },
      rejectPending: function (id) {
        if (!pending || submittedRequest !== id) return;
        pending = false;
        pendingMode = null;
        pendingDraft = null;
        submittedRequest = null;
        updateDraftStatus();
      },
      stageBaseline: function () {
        if (autoRunTimer) clearTimeout(autoRunTimer);
        autoRunTimer = null;
        pending = true;
        pendingBinding = initialBinding;
        pendingMode = "baseline";
        pendingDraft = null;
        pendingRevision = revision;
        submittedRequest = null;
        draft = cloneStructured(initialDraft);
        startingDraft = cloneStructured(initialDraft);
        rawDraft = initialBinding && typeof initialBinding.expr === "string" ? initialBinding.expr : "";
        startingRawDraft = rawDraft;
        dirty = false;
        sourceChanged = false;
        render();
      },
      restore: function () {
        if (autoRunTimer) clearTimeout(autoRunTimer);
        autoRunTimer = null;
        if (sourceChanged && control.currentBinding === null) startingDraft = cloneStructured(latestDefaultDraft);
        draft = cloneStructured(startingDraft);
        rawDraft = startingRawDraft;
        dirty = false;
        sourceChanged = false;
        control.setError("");
        render();
      },
      setAutoRun: function (enabled) {
        if (autoRunTimer) clearTimeout(autoRunTimer);
        autoRunTimer = null;
        if (enabled && dirty) queueAutoRun();
        updateDraftStatus();
      },
      isDirty: function () { return dirty && !pending; },
      submitIfDirty: function () { if (dirty && !pending) submitDraft(true); },
    };

    function updateDraftStatus() {
      draftStatus.textContent = pending
        ? "Updating the complete parameter value…"
        : sourceChanged
          ? "The reactive default changed; discard to reload it."
          : dirty
            ? autoRun
              ? "Draft differs from the last accepted value. Auto run will retry after the next edit."
              : "Unapplied edits. The last accepted results remain visible."
            : "";
      draftStatus.hidden = !draftStatus.textContent;
      if (workspace) {
        workspace.status(
          port.name,
          draftStatus.textContent,
          pending ? "pending" : sourceChanged ? "stale-default" : dirty ? "unapplied" : "none",
        );
      }
      updateApplyAll();
    }
    function queueAutoRun() {
      if (autoRunTimer) clearTimeout(autoRunTimer);
      autoRunTimer = setTimeout(function () {
        autoRunTimer = null;
        submitDraft(false);
      }, AUTO_RUN_DELAY_MS);
    }
    function editDraft() {
      revision += 1;
      dirty = true;
      sourceChanged = false;
      control.setError("");
      updateDraftStatus();
      if (workspace) workspace.refresh();
      if (autoRun) {
        queueAutoRun();
        setStatus("Waiting for the edit to finish", "warn", true);
      } else {
        setStatus("Edits not applied yet", "warn", true);
      }
    }
    function render() {
      editorHolder.replaceChildren();
      appendStructuredEditor(editorHolder, port, port.schema, draft, editDraft, port.name, true, []);
      rawField.value = rawDraft;
      editorHolder.hidden = mode !== "form";
      rawHolder.hidden = mode !== "raw";
      updateDraftStatus();
      if (workspace && holder.isConnected) workspace.refresh();
    }

    var isStructured = port.schema.kind === "algebraic" || port.schema.kind === "indexed";
    if (isStructured) {
      var modes = element("div", "control-modes");
      var formMode = element("button", "control-mode control-mode--active", "Form");
      var rawMode = element("button", "control-mode", "Raw literal");
      formMode.type = "button";
      rawMode.type = "button";
      formMode.addEventListener("click", function () {
        mode = "form";
        formMode.className = "control-mode control-mode--active";
        rawMode.className = "control-mode";
        render();
      });
      rawMode.addEventListener("click", function () {
        mode = "raw";
        rawMode.className = "control-mode control-mode--active";
        formMode.className = "control-mode";
        render();
      });
      modes.appendChild(formMode);
      modes.appendChild(rawMode);
      holder.appendChild(modes);
    }
    rawField.addEventListener("input", function () {
      rawDraft = rawField.value;
      editDraft();
    });
    render();
    holder.appendChild(editorHolder);
    holder.appendChild(rawHolder);
    holder.appendChild(element(
      "p",
      "control-snapshot-note",
      "Each run overrides the whole parameter, including unchanged fields.",
    ));
    function submitDraft(reportIncomplete) {
      var incomplete = mode === "form"
        ? incompleteDraft(draft, [])
        : rawDraft.trim()
          ? null
          : { path: [], message: "Enter a complete closed value before applying." };
      if (incomplete) {
        if (reportIncomplete) control.setError(incomplete.message, incomplete.path);
        updateDraftStatus();
        setStatus(reportIncomplete ? "Input incomplete; results not updated" : "Waiting for a complete input", "warn", true);
        return;
      }
      if (autoRunTimer) clearTimeout(autoRunTimer);
      autoRunTimer = null;
      pending = true;
      pendingMode = mode;
      pendingDraft = mode === "form" ? cloneStructured(draft) : rawDraft;
      pendingRevision = revision;
      submittedRequest = null;
      pendingBinding = mode === "form" && isStructured
        ? { name: port.name, value: cloneStructured(pendingDraft) }
        : { name: port.name, expr: mode === "form" ? pendingDraft.expr : pendingDraft };
      updateDraftStatus();
      scheduleEvaluate();
    }

    var actions = element("div", "control-actions");
    var apply = element("button", "control-apply", "Apply");
    apply.type = "button";
    apply.addEventListener("click", function () { submitDraft(true); });
    var discard = element("button", "control-discard", "Discard edits");
    discard.type = "button";
    discard.addEventListener("click", control.restore);
    actions.appendChild(apply);
    actions.appendChild(discard);
    if (port.has_default) {
      var clear = element("button", "control-clear", "Use default");
      clear.type = "button";
      clear.addEventListener("click", function () {
        if (autoRunTimer) clearTimeout(autoRunTimer);
        autoRunTimer = null;
        pending = true;
        pendingBinding = null;
        pendingMode = "default";
        pendingDraft = null;
        pendingRevision = revision;
        submittedRequest = null;
        dirty = false;
        sourceChanged = false;
        updateDraftStatus();
        scheduleEvaluate();
      });
      actions.appendChild(clear);
    }
    holder.appendChild(actions);
    holder.appendChild(draftStatus);
    holder.appendChild(errorLine);
    card.appendChild(holder);
    if (workspace) {
      var parameterActions = [
        { label: "Apply", run: function () { submitDraft(true); } },
        { label: "Discard edits", run: control.restore },
      ];
      if (clear) parameterActions.push({ label: "Use default", run: function () { clear.click(); } });
      workspace.parameter(port.name, parameterActions);
    }
    return control;
  }

  function buildControls(ports, evaluation) {
    if (!workspace && global.GraphcalReportWorkspace) workspace = global.GraphcalReportWorkspace.mount();
    var paramViews = {};
    for (var v = 0; v < evaluation.values.length; v += 1) {
      var declaration = evaluation.values[v];
      if (declaration.declaration_kind === "param" && declaration.outcome.status === "value") {
        paramViews[declaration.name] = declaration.outcome.value;
      }
    }
    for (var i = 0; i < ports.length; i += 1) {
      var control = makeControl(ports[i], paramViews[ports[i].name]);
      if (control) controls.set(control.name, control);
    }
    if (workspace) workspace.refresh(true);
    resetButton.addEventListener("click", function () {
      controls.forEach(function (control) {
        control.setError("");
        control.stageBaseline();
      });
      scheduleEvaluate();
    });
  }

  autoRunToggle.addEventListener("change", function () {
    autoRun = autoRunToggle.checked;
    controls.forEach(function (control) { control.setAutoRun(autoRun); });
  });

  function currentBindings() {
    var bindings = baselineBindings.filter(function (binding) {
      return !controls.has(binding.name);
    });
    controls.forEach(function (control) {
      var binding = control.bindingForEvaluation();
      if (binding) bindings.push(binding);
    });
    return bindings;
  }

  // --- Result rendering ----------------------------------------------------
  function buildEntriesTable(entries) {
    var table = element("table", "entries");
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

  function buildGrid(grid) {
    var columns = grid.columns;
    var rows = grid.rows;
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
        td.textContent = rows[r][1][c2];
        tr.appendChild(td);
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    return table;
  }

  // Shape and leaf formatting come from the exact native report projection.
  // JavaScript is only the DOM shell for the shared ValueBody contract.
  function renderView(view, name) {
    var result;
    switch (view.kind) {
      case "scalar": result = element("p", "card-value", view.body); break;
      case "entries": result = buildEntriesTable(view.body); break;
      case "grid": result = buildGrid(view.body); break;
      case "slices":
        result = element("div", "slices");
        for (var slice of view.body) {
          result.appendChild(element("h4", "slice-label", "[" + slice[0] + "]"));
          result.appendChild(buildGrid(slice[1]));
        }
        break;
      default: throw new Error("unknown report value body: " + view.kind);
    }
    if (view.kind !== "scalar") {
      var wrapper = element("div", "value-scroll");
      wrapper.setAttribute("role", "region");
      wrapper.setAttribute("tabindex", "0");
      wrapper.setAttribute("aria-label", name + " values");
      wrapper.appendChild(result);
      result = wrapper;
    }
    result.setAttribute("data-role", "value");
    return result;
  }

  // The first applied evaluation only verifies the published page; later ones
  // mark what they changed so readers can follow the effect of an edit.
  var hasApplied = false;
  function markChanged(target) {
    target.classList.remove("value-changed");
    void target.offsetWidth; // restart the highlight animation
    target.classList.add("value-changed");
  }

  function patchValues(evaluation) {
    document.querySelectorAll(".card.value-changed").forEach(function (card) { card.classList.remove("value-changed"); });
    for (var i = 0; i < evaluation.values.length; i += 1) {
      var declaration = evaluation.values[i];
      var card = document.querySelector('[data-decl="' + selectorEscape(declaration.name) + '"]');
      if (!card) continue;
      var slot = card.querySelector('[data-role="value"]');
      if (!slot) continue;
      var previousText = slot.textContent;
      var replacement;
      if (declaration.outcome.status === "value") {
        replacement = renderView(declaration.outcome.body, declaration.name);
      } else {
        var incomplete = declaration.outcome.status === "incomplete";
        var error = incomplete ? declaration.outcome.reason : declaration.outcome.error;
        var message =
          error.kind === "dependency_failed"
            ? "dependency failed: " + error.failed_dependencies.join(", ")
            : error.message;
        replacement = element("p", incomplete ? "notice" : "error-chip", (incomplete ? "" : "ERROR: ") + message);
        replacement.setAttribute("data-role", "value");
      }
      var changed = hasApplied && previousText !== replacement.textContent;
      if (slot.classList.contains("value-scroll") && replacement.classList.contains("value-scroll")) {
        // Retain keyboard focus and the reader's position across recalculation.
        var left = slot.scrollLeft;
        var top = slot.scrollTop;
        slot.replaceChildren(...replacement.childNodes);
        slot.scrollLeft = left;
        slot.scrollTop = top;
      } else {
        slot.replaceWith(replacement);
      }
      if (changed) markChanged(card);
    }
  }

  function patchParamControls(evaluation) {
    for (var i = 0; i < evaluation.values.length; i += 1) {
      var declaration = evaluation.values[i];
      if (declaration.declaration_kind !== "param") continue;
      var control = controls.get(declaration.name);
      if (!control) continue;
      if (declaration.outcome.status === "value") control.showView(declaration.outcome.value);
    }
  }

  function patchChecks(evaluation) {
    for (var i = 0; i < evaluation.assertions.length; i += 1) {
      var assertion = evaluation.assertions[i];
      var item = document.querySelector('[data-check="' + selectorEscape(assertion.name) + '"]');
      if (!item) continue;
      var status = assertion.outcome.status;
      var statusChanged = hasApplied && !item.classList.contains("check--" + status);
      item.className = "check check--" + status;
      if (statusChanged) markChanged(item);
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

  function patchEvaluationNotices(evaluation) {
    var banner = document.getElementById("incomplete-notice");
    if (!banner && evaluation.incomplete) {
      banner = element("p", "notice", "Model incomplete: unfinished formulas remain.");
      banner.id = "incomplete-notice";
      document.querySelector("main").appendChild(banner);
    }
    if (banner) banner.hidden = !evaluation.incomplete;
    var notices = evaluation.notices.filter(function (notice) { return notice.kind === "call_incomplete"; });
    var section = document.getElementById("call-notices");
    if (!section && notices.length === 0) return;
    if (!section) {
      section = element("section");
      section.id = "call-notices";
      document.querySelector("main").appendChild(section);
    }
    section.hidden = notices.length === 0;
    section.replaceChildren(element("h2", "", "DAG calls"));
    notices.forEach(function (notice) {
      section.appendChild(element("p", "notice", notice.name + ": " + notice.message));
    });
  }

  var figureStates = new Map();

  function patchFigures(evaluation) {
    var specs = new Map(evaluation.figures.map(function (figure) { return [figure.name, figure.spec]; }));
    var failures = new Map(evaluation.notices.filter(function (notice) {
      return notice.kind === "plot_error" || notice.kind === "plot_incomplete";
    }).map(function (notice) { return [notice.name, { message: notice.message, incomplete: notice.kind === "plot_incomplete" }]; }));
    var targets = new Map(Array.from(document.querySelectorAll("figure[data-figure]")).map(function (target) {
      return [target.getAttribute("data-figure"), target];
    }));
    var names = new Set([...targets.keys(), ...specs.keys(), ...failures.keys(), ...figureStates.keys()]);
    if (names.size === 0) return;
    var section = document.getElementById("plots");
    if (!section) {
      section = element("section");
      section.id = "plots";
      section.tabIndex = -1;
      section.appendChild(element("h2", "", "Plots"));
      document.querySelector("main").appendChild(section);
    }
    names.forEach(function (name) {
      var previous = figureStates.get(name);
      if (previous && previous.view) previous.view.finalize();
      var target = targets.get(name);
      if (!target) {
        target = element("figure", "plot");
        target.setAttribute("data-figure", name);
        section.appendChild(target);
      }
      target.tabIndex = 0;
      target.setAttribute("aria-label", name + " plot");
      var caption = target.querySelector("figcaption") || element("figcaption", "figure-name", name);
      var mount = element("div");
      mount.setAttribute("data-role", "figure");
      // A fresh mount detaches even a pending renderer from the current result.
      target.replaceChildren(caption, mount);
      var state = { view: null };
      figureStates.set(name, state);
      function fail(message, incomplete) {
        if (figureStates.get(name) !== state) return;
        mount.replaceChildren(element("p", incomplete ? "notice" : "error-chip", "Plot unavailable: " + message));
      }
      if (failures.has(name) || !specs.has(name)) {
        var failure = failures.get(name);
        fail(failure ? failure.message : "not present in the current evaluation", failure && failure.incomplete);
        return;
      }
      if (typeof window.vegaEmbed !== "function") {
        fail("Vega renderer is unavailable");
        return;
      }
      Promise.resolve().then(function () {
        return window.vegaEmbed(mount, specs.get(name), { actions: false });
      }).then(function (result) {
        if (figureStates.get(name) !== state) result.view.finalize();
        else state.view = result.view;
      }).catch(function (error) {
        fail("rendering failed: " + String(error));
        if (figureStates.get(name) === state) setStatus("Up to date; a chart failed to render", "warn");
      });
    });
  }

  function patchNotices(evaluation) {
    var section = document.getElementById("presentation");
    if (!section) {
      section = element("section");
      section.id = "presentation";
      section.tabIndex = -1;
      document.querySelector("main").appendChild(section);
    }
    section.hidden = evaluation.notices.length === 0;
    section.replaceChildren();
    if (!section.hidden) section.appendChild(element("h2", "", "Presentation diagnostics"));
    for (var notice of evaluation.notices) {
      section.appendChild(element("p", "notice", notice.message));
    }
  }

  function patchSectionNavigation() {
    var list = document.querySelector('.report-nav ul');
    if (!list) return;
    list.replaceChildren();
    if (workspace) workspace.reconcileResults();
    for (var section of document.querySelectorAll('main section[id], main footer[id]')) {
      var heading = section.querySelector('h2');
      if (section.hidden || !heading) continue;
      var item = element('li');
      var link = element('a', '', heading.textContent);
      link.setAttribute('href', '#' + section.id);
      item.appendChild(link);
      list.appendChild(item);
    }
  }

  function clearBindingErrors() {
    controls.forEach(function (control) {
      control.setError("");
    });
  }

  function applyOutcome(outcome, completedRequest) {
    if (outcome.status === "evaluated") {
      clearBindingErrors();
      controls.forEach(function (control) { control.acceptPending(completedRequest); });
      patchValues(outcome.evaluation);
      patchParamControls(outcome.evaluation);
      patchChecks(outcome.evaluation);
      patchFigures(outcome.evaluation);
      patchEvaluationNotices(outcome.evaluation);
      patchNotices(outcome.evaluation);
      if (outcome.html) {
        var provenance = new DOMParser().parseFromString(outcome.html, "text/html").getElementById("provenance");
        var oldProvenance = document.getElementById("provenance");
        if (provenance && oldProvenance) oldProvenance.replaceWith(provenance);
      }
      patchSectionNavigation();
      updateChecksSummary();
      hasApplied = true;
      // Failures are reported by the Checks cell and the tab counts; this cell
      // only says whether the results on screen are current.
      setStatus(
        outcome.evaluation.incomplete ? "Up to date; model incomplete" : "Up to date",
        outcome.evaluation.incomplete ? "warn" : "ok",
      );
    } else if (outcome.status === "binding_errors") {
      controls.forEach(function (control) { control.rejectPending(completedRequest); });
      clearBindingErrors();
      for (var i = 0; i < outcome.errors.length; i += 1) {
        var bindingError = outcome.errors[i];
        var control = controls.get(bindingError.name);
        if (control) control.setError(bindingError.message, bindingError.path || []);
      }
      setStatus("Input rejected; showing previous results", "warn", true);
    } else {
      setStatus("evaluation failed: " + outcome.message, "error");
    }
    // A parameter is overridden while its accepted binding differs from the baseline.
    var overriddenCount = 0;
    controls.forEach(function (control) {
      var active = JSON.stringify(control.currentBinding) !== JSON.stringify(control.initialBinding);
      if (active) overriddenCount += 1;
      if (workspace) workspace.overridden(control.name, active);
    });
    banner.hidden = overriddenCount === 0;
    publishedText.hidden = !banner.hidden;
    bannerText.textContent = overriddenCount === 1 ? "1 overridden" : overriddenCount + " overridden";
    inputsCell.cell.classList.toggle("is-modified", !banner.hidden);
    inputsCell.cell.title = banner.hidden
      ? "No input is overridden"
      : "Results use edited inputs; the source file is unchanged";
  }

  // --- Transport loop ------------------------------------------------------
  var transport = null;
  var requestId = 0;
  var activeRequest = null;
  var timeoutTimer = null;
  var debounceTimer = null;
  var evaluateQueued = false;
  var ready = false;
  var storedPorts = [];

  // Before the controls exist (the first evaluation seeds them), replay the
  // build-time baseline bindings so the initial result matches the page.
  function activeBindings() {
    return controls.size > 0 ? currentBindings() : baselineBindings;
  }

  function scheduleEvaluate() {
    if (debounceTimer) clearTimeout(debounceTimer);
    if (options.onPending) options.onPending();
    setStatus("Recalculating…", "busy", true);
    debounceTimer = setTimeout(function () {
      debounceTimer = null;
      runEvaluate();
    }, DEBOUNCE_MS);
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
    evaluateQueued = false;
    requestId += 1;
    activeRequest = requestId;
    setStatus("Recalculating…", "busy", true);
    if (!options.hostOwnsTimeout) timeoutTimer = setTimeout(function () {
      // Cancellation is worker teardown: a blocked evaluation cannot be
      // interrupted, so replace the whole engine and re-prepare.
      transport.terminate();
      controls.forEach(function (control) { control.rejectPending(activeRequest); });
      activeRequest = null;
      ready = false;
      evaluateQueued = false;
      setStatus("Timed out; restarting the engine", "warn", true);
      startTransport();
    }, EVALUATION_TIMEOUT_MS);
    controls.forEach(function (control) { control.markSubmitted(requestId); });
    transport.postMessage({ type: "evaluate", id: requestId, bindings: activeBindings() });
  }

  function handleMessage(msg) {
    if (msg.type === "ready") {
      ready = true;
      storedPorts = msg.ports;
      if (options.initial) {
        var initial = options.initial;
        options.initial = null;
        // A host-provided evaluation compares against source defaults, not a
        // published baseline.
        publishedText.textContent = "Source defaults";
        resetButton.textContent = "Reset parameters";
        resetButton.title = "Return every input to its source default";
        buildControls(storedPorts, initial.evaluation);
        initial.bindings.forEach(function (binding) {
          var control = controls.get(binding.name);
          if (control) control.currentBinding = binding;
        });
        applyOutcome({ status: "evaluated", evaluation: initial.evaluation });
        return;
      }
      // The first evaluation (over the baseline bindings) both verifies the
      // static page and supplies the evaluated param values the controls
      // seed their literal expressions from.
      evaluateQueued = false;
      runEvaluate();
      return;
    }
    if (msg.type === "result") {
      if (msg.id !== activeRequest) return;
      clearTimeout(timeoutTimer);
      activeRequest = null;
      if (controls.size === 0 && msg.outcome.status === "evaluated") {
        buildControls(storedPorts, msg.outcome.evaluation);
      }
      // A control edit invalidates old results even before its debounce fires.
      if (!evaluateQueued && !debounceTimer) applyOutcome(msg.outcome, msg.id);
      if (evaluateQueued && !debounceTimer) {
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

  function startTransport() {
    try {
      transport = options.createTransport({
        onMessage: handleMessage,
        onError: function (message) {
          fatal(message || "the report engine crashed");
        },
      });
      if (!transport || typeof transport.postMessage !== "function" ||
          typeof transport.terminate !== "function") {
        throw new Error("transport must provide postMessage and terminate");
      }
    } catch (error) {
      fatal("this browser refused to start the report engine (" + error + ")");
      return;
    }
    setStatus("Preparing model…", "busy");
  }

  startTransport();
  }

  global.GraphcalReport = { mount: mount };
})(window);

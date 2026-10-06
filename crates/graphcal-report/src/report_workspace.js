// Adaptive report workspace: DOM shell over explicitly registered typed controls.
// The typed controls stay mounted but hidden; outline rows mirror their widgets
// and reuse the same handlers and draft lifecycle. This module never creates bindings.
(function (global) {
  "use strict";
  var SEARCH_FIELD_LIMIT = 4096;
  var SEARCH_PAGE_LIMIT = 128;

  function mount() {
    var state = global.GraphcalReportOutlineState;
    var inputs = document.getElementById("inputs");
    var main = document.querySelector("main");
    if (!inputs || !main) return null;
    function element(tag, className, text) {
      var node = document.createElement(tag);
      if (className) node.className = className;
      if (text !== undefined) node.textContent = text;
      return node;
    }
    function text(node, value) { if (node.textContent !== value) node.textContent = value; }
    function button(label, action, className) {
      var node = element("button", className, label);
      node.type = "button";
      node.addEventListener("click", action);
      return node;
    }

    var fields = new Map();
    var parameters = new Map();
    var errors = new Map();
    var statuses = new Map();
    var refreshScheduled = false;
    var forceRefresh = false;
    function requestRefresh(force) {
      forceRefresh = forceRefresh || Boolean(force);
      if (refreshScheduled) return;
      refreshScheduled = true;
      queueMicrotask(function () {
        refreshScheduled = false;
        var requested = forceRefresh;
        forceRefresh = false;
        refresh(requested);
      });
    }
    var pins = new Set();
    // Parameters whose current results use an override, and those with unapplied drafts.
    var overridden = new Set();
    var drafted = new Set();
    var parameterGroups = new Map();
    var rows = new Map();
    var expanded = new Map();
    var loaders = new Set();
    var dirty = true;
    var serial = 0;
    var refreshing = false;
    var searchLimited = false;
    var lastKeys = [];

    var workspace = element("div", "report-workspace");
    var mobileNav = element("nav", "workspace-mobile-nav");
    mobileNav.setAttribute("aria-label", "Report workspace");
    [["inputs", "Inputs"], ["workspace-results", "Results"]].forEach(function (entry) {
      // A fragment link in srcdoc resolves against the host page's URL. Move
      // focus/scroll locally instead, without navigating the sandboxed report.
      var jump = button(entry[1], function () {
        var target = document.getElementById(entry[0]);
        target.focus({ preventScroll: true });
        target.scrollIntoView({ block: "start" });
      });
      jump.setAttribute("aria-controls", entry[0]);
      mobileNav.appendChild(jump);
    });
    main.append(mobileNav, workspace);
    workspace.appendChild(inputs);
    document.body.classList.add("report-workspace-active");
    // The typed controls stay mounted but hidden: outline rows mirror their
    // widgets, and print shows their cards in place of the outline.
    var original = element("div", "outline-advanced");
    original.hidden = true;
    original.appendChild(inputs.querySelector(".cards"));
    inputs.appendChild(original);
    var heading = inputs.querySelector("h2");
    var head = element("div", "outline-heading");
    if (heading) head.appendChild(heading);
    var search = element("input", "outline-search");
    search.type = "search";
    search.placeholder = "Search inputs by name, field, or description…";
    search.setAttribute("aria-label", "Search inputs");
    head.appendChild(search);
    var options = element("div", "outline-options");
    var pinnedOnly = element("input");
    pinnedOnly.type = "checkbox";
    var pinnedLabel = element("label");
    pinnedLabel.append(pinnedOnly, document.createTextNode("Pinned only"));
    var count = element("span", "outline-count");
    count.setAttribute("role", "status");
    options.append(pinnedLabel, count);
    head.appendChild(options);
    var toolbar = inputs.querySelector(".input-toolbar");
    if (toolbar) options.insertBefore(toolbar, count);
    var notice = element("p", "outline-notice");
    notice.setAttribute("role", "status");
    notice.hidden = true;
    var explorer = element("div", "outline-explorer");
    var favorites = element("section", "outline-favorites");
    favorites.appendChild(element("h3", "", "Pinned inputs"));
    var favoriteRows = element("div");
    favorites.appendChild(favoriteRows);
    var allHeading = element("h3", "outline-all-heading", "All inputs");
    var content = element("div", "outline-content");
    var empty = element("p", "outline-empty", "No matching inputs. Clear the search or turn off Pinned only.");
    empty.hidden = true;
    var more = button("Show more entries", function () {
      activeLoaders().forEach(function (loader) { loader.click(); });
      refresh(true);
    }, "outline-more");
    explorer.append(favorites, allHeading, content, more, empty);
    inputs.prepend(head, notice, explorer);

    var results = global.GraphcalReportResults.mount(main, workspace);

    function activeLoaders() {
      return Array.from(loaders).filter(function (loader) { return loader.isConnected && !loader.hidden; });
    }
    function liveFields() {
      return Array.from(fields.values()).filter(function (field) { return field.widget.isConnected; });
    }
    function fieldError(field) {
      var error = errors.get(field.parameter);
      return error && state.isPathPrefix(error.path, field.path) ? error.message : "";
    }
    // `root` marks a row that stands for its whole parameter (not a nested field);
    // only such rows carry the parameter description and override marker.
    function rowFor(field, contextLabel, root) {
      var cached = rows.get(field.key);
      if (!cached || cached.kind !== field.kind || Boolean(cached.slider) !== Boolean(field.slider)) {
        if (cached) cached.row.remove();
        var row = element("div", "outline-row");
        row.setAttribute("data-parameter", field.parameter);
        var pin = button("", function () {
          if (pins.has(field.key)) pins.delete(field.key); else pins.add(field.key);
          dirty = true;
          refresh();
        }, "outline-pin");
        pin.appendChild(global.GraphcalReportResults.pinIcon());
        pin.setAttribute("aria-label", "Pin input " + field.labels.join(" › "));
        var name = element("div", "outline-name");
        var label = element("label", "outline-label");
        var hint = element("span", "outline-hint");
        var description = element("span", "outline-desc", field.description);
        name.append(label, hint, description);
        var widget = element(field.kind === "select" ? "select" : "input", "outline-value");
        if (field.kind !== "select") widget.type = field.kind === "boolean" ? "checkbox" : "text";
        widget.spellcheck = false;
        serial += 1;
        widget.id = "outline-field-" + serial;
        label.htmlFor = widget.id;
        widget.setAttribute("aria-label", field.labels.join(" "));
        // The field box joins the editable literal and its unit tag into one
        // control. The bracketed tag is visual; assistive technology gets the
        // spelled-out dimension as part of the field's description instead.
        var fieldBox = element("span", "outline-field");
        var unitTag = element("span", "outline-unit", field.unit);
        unitTag.setAttribute("aria-hidden", "true");
        unitTag.title = field.unitTitle;
        unitTag.hidden = !field.unit;
        fieldBox.classList.toggle("has-unit", Boolean(field.unit));
        fieldBox.append(widget, unitTag);
        var errorLine = element("p", "outline-error");
        errorLine.id = widget.id + "-error";
        var describedBy = [errorLine.id];
        if (field.unitTitle) {
          var unitNote = element("span", "visually-hidden", field.unitTitle);
          unitNote.id = widget.id + "-unit";
          fieldBox.appendChild(unitNote);
          describedBy.push(unitNote.id);
        }
        widget.setAttribute("aria-describedby", describedBy.join(" "));
        widget.addEventListener(field.kind === "literal" ? "input" : "change", function () {
          var current = fields.get(field.key);
          if (!current || !current.widget.isConnected) return;
          if (current.kind === "boolean") current.widget.checked = widget.checked;
          else current.widget.value = widget.value;
          current.widget.dispatchEvent(new Event(current.kind === "literal" ? "input" : "change", { bubbles: true }));
        });
        var actions = element("details", "outline-row-actions");
        var summary = element("summary", "", "⋯");
        summary.setAttribute("aria-label", "Actions for " + field.labels.join(" › "));
        summary.title = "Apply, discard, or reset " + field.parameter;
        var menu = element("div", "outline-action-menu");
        if (field.labels.length > 1) menu.appendChild(element("small", "", "Actions apply to the whole " + field.parameter + " parameter."));
        var parameter = parameters.get(field.parameter);
        parameter.actions.forEach(function (action) {
          menu.appendChild(button(action.label, function () { action.run(); actions.open = false; }));
        });
        if (field.description && field.labels.length > 1) menu.appendChild(element("p", "", field.description));
        actions.append(summary, menu);
        row.append(pin, name, fieldBox, actions);
        var range = null;
        if (field.slider) {
          // Mirrors the typed control's slider; the native slider owns the draft literal.
          range = element("input", "outline-slider");
          range.type = "range";
          range.setAttribute("aria-label", field.labels.join(" ") + " slider");
          range.addEventListener("input", function () {
            var current = fields.get(field.key);
            if (!current || !current.slider || !current.slider.isConnected) return;
            current.slider.value = range.value;
            current.slider.dispatchEvent(new Event("input", { bubbles: true }));
          });
          row.appendChild(range);
        }
        row.appendChild(errorLine);
        cached = { row: row, pin: pin, label: label, hint: hint, description: description, widget: widget, slider: range, error: errorLine, kind: field.kind, context: field.labels.at(-1), root: field.labels.length === 1 };
        rows.set(field.key, cached);
      }
      if (typeof contextLabel === "string") cached.context = contextLabel;
      if (typeof root === "boolean") cached.root = root;
      var isPinned = pins.has(field.key);
      text(cached.label, isPinned ? field.labels.join(" › ") : cached.context);
      cached.label.title = field.labels.join(" › ");
      text(cached.hint, field.hint || "");
      cached.hint.hidden = !field.hint;
      cached.description.hidden = !field.description || !cached.root || isPinned;
      cached.row.classList.toggle("outline-row--overridden", cached.root && overridden.has(field.parameter));
      cached.row.classList.toggle("outline-row--draft", cached.root && drafted.has(field.parameter));
      cached.pin.setAttribute("aria-pressed", String(isPinned));
      cached.pin.title = isPinned ? "Unpin" : "Pin to the top of the list";
      if (cached.slider && field.slider) {
        cached.slider.min = field.slider.min;
        cached.slider.max = field.slider.max;
        cached.slider.step = field.slider.step;
        if (document.activeElement !== cached.slider) cached.slider.value = field.slider.value;
        cached.slider.classList.toggle("is-unplaced", field.slider.classList.contains("is-unplaced"));
        cached.slider.title = field.slider.title;
      }
      if (field.kind === "select") {
        var options = Array.from(field.widget.options).map(function (o) { return [o.value, o.textContent, o.disabled]; });
        var signature = JSON.stringify(options);
        if (signature !== cached.options) {
          cached.widget.replaceChildren.apply(cached.widget, options.map(function (o) {
            var option = element("option", "", o[1]); option.value = o[0]; option.disabled = o[2]; return option;
          }));
          cached.options = signature;
        }
      }
      if (field.kind === "boolean") {
        cached.widget.checked = field.widget.checked;
        cached.widget.indeterminate = field.widget.indeterminate;
      } else if (cached.widget.value !== field.widget.value) {
        // The native draft is authoritative, including reset and default changes
        // while a mirrored field still has focus (notably on Safari buttons).
        var start = cached.widget.selectionStart;
        var end = cached.widget.selectionEnd;
        cached.widget.value = field.widget.value;
        if (field.kind === "literal" && start !== null) cached.widget.setSelectionRange(start, end);
      }
      var message = fieldError(field);
      text(cached.error, message);
      cached.error.hidden = !message;
      cached.widget.setAttribute("aria-invalid", String(Boolean(message)));
      return cached.row;
    }
    function renderGroup(group, path, query) {
      var all = state.descendants(group);
      var visible = all.filter(function (field) { return !pins.has(field.key) && state.matches(field.labels, field.description, query); });
      if (!visible.length) return null;
      var isParameter = path.length === 1;
      if (all.length === 1) {
        // A lone field (including a payload-free constructor choice) reads as one row.
        var only = all[0];
        var inlineLabels = only.labels.slice(path.length - 1, only.constructorControl ? -1 : undefined);
        return rowFor(only, inlineLabels.join(" › "), isParameter);
      }
      var detail = element("details", "outline-group");
      if (isParameter) parameterGroups.set(group.name, detail);
      var summary = element("summary");
      summary.append(element("span", "", group.name), element("small", "", visible.length + (visible.length === 1 ? " field" : " fields")));
      if (isParameter && all[0].description) summary.appendChild(element("span", "outline-desc", all[0].description));
      // Standard serialization is confined to DOM identity/cache boundaries.
      var key = JSON.stringify(path);
      detail.open = Boolean(query) || visible.some(fieldError) || (expanded.has(key) ? expanded.get(key) : all.length <= 4);
      detail.addEventListener("toggle", function () { if (!query && detail.isConnected) expanded.set(key, detail.open); });
      var body = element("div", "outline-group-body");
      group.rows.filter(function (field) { return visible.includes(field); }).forEach(function (field) { body.appendChild(rowFor(field, field.labels.at(-1), false)); });
      group.groups.forEach(function (child) {
        var nested = renderGroup(child, path.concat([child.name]), query);
        if (nested) body.appendChild(nested);
      });
      detail.append(summary, body);
      return detail;
    }
    function refresh(force) {
      if (refreshing) { dirty = true; return; }
      refreshing = true;
      var focused = document.activeElement;
      try {
        searchLimited = false;
        if (search.value.trim()) {
          for (var page = 0; page < SEARCH_PAGE_LIMIT; page += 1) {
            var pending = activeLoaders();
            if (!pending.length) break;
            if (liveFields().length >= SEARCH_FIELD_LIMIT) { searchLimited = true; break; }
            pending[0].click();
          }
          searchLimited = searchLimited || activeLoaders().length > 0;
        }
        var items = liveFields();
        var keys = items.map(function (field) { return field.key; });
        var topologyChanged = keys.length !== lastKeys.length || keys.some(function (key, index) { return key !== lastKeys[index]; });
        if (force || dirty || topologyChanged) {
          var query = search.value.trim();
          var matching = items.filter(function (field) { return state.matches(field.labels, field.description, query); });
          var favoriteItems = matching.filter(function (field) { return pins.has(field.key); });
          favoriteRows.replaceChildren.apply(favoriteRows, favoriteItems.map(function (field) { return rowFor(field, undefined, false); }));
          favorites.hidden = !favoriteItems.length;
          content.replaceChildren();
          parameterGroups.clear();
          if (!pinnedOnly.checked) {
            var tree = state.tree(items);
            tree.rows.filter(function (field) { return matching.includes(field) && !pins.has(field.key); }).forEach(function (field) { content.appendChild(rowFor(field, field.labels.at(-1), true)); });
            tree.groups.forEach(function (group) {
              var node = renderGroup(group, [group.name], query);
              if (node) content.appendChild(node);
            });
          }
          allHeading.hidden = pinnedOnly.checked;
          content.hidden = pinnedOnly.checked;
          empty.hidden = (pinnedOnly.checked ? favoriteItems.length : matching.length) !== 0;
          text(count, (query ? matching.length + " of " : "") + items.length + (items.length === 1 ? " field" : " fields"));
          lastKeys = keys;
          dirty = false;
        } else items.forEach(function (field) { rowFor(field); });
        parameterGroups.forEach(function (detail, name) {
          detail.classList.toggle("outline-group--overridden", overridden.has(name));
          detail.classList.toggle("outline-group--draft", drafted.has(name));
        });
        fields.forEach(function (field, key) { if (!field.widget.isConnected) fields.delete(key); });
        rows.forEach(function (cached, key) { if (!fields.has(key)) { cached.row.remove(); rows.delete(key); } });
        more.hidden = !activeLoaders().length || pinnedOnly.checked;
        loaders.forEach(function (loader) { if (!loader.isConnected) loaders.delete(loader); });
        var messages = Array.from(errors.values()).filter(function (error) { return error.message; });
        // Only drafts that need the reader's action are listed here. Pending and
        // unapplied edits are already marked on their rows and in the header, and
        // listing them would shift the outline on every keystroke.
        var changed = Array.from(statuses).filter(function (entry) { return entry[1].state === "stale-default"; });
        var statusText = changed.slice(0, 3).map(function (entry) { return entry[0] + ": " + entry[1].message; }).join(" ");
        if (changed.length > 3) statusText += " And " + (changed.length - 3) + " more parameters.";
        text(notice, messages.length ? "Input rejected. The previous results stay visible until the highlighted input is fixed." : searchLimited ? "Search is limited to the loaded fields. Show more entries to continue." : statusText);
        notice.hidden = !messages.length && !searchLimited && !changed.length;
      } finally {
        refreshing = false;
        if (focused && focused.isConnected && explorer.contains(focused)) focused.focus({ preventScroll: true });
      }
    }
    search.addEventListener("input", function () { refresh(true); });
    pinnedOnly.addEventListener("change", function () { refresh(true); });
    return {
      parameter: function (name, actions) { parameters.set(name, { actions: actions }); },
      field: function (field) {
        field.key = JSON.stringify([field.parameter, field.identity]);
        fields.set(field.key, field);
      },
      loader: function (loader) { loaders.add(loader); },
      error: function (name, message, path) {
        var previous = errors.get(name);
        if (message || (previous && previous.message)) dirty = true;
        errors.set(name, { message: message, path: path || [] });
        requestRefresh();
      },
      // `draftState` ("none" | "pending" | "unapplied" | "stale-default") is the
      // typed lifecycle; `message` is display text only.
      status: function (name, message, draftState) {
        statuses.set(name, { message: message, state: draftState });
        if (draftState === "unapplied" || draftState === "stale-default") drafted.add(name); else drafted.delete(name);
        requestRefresh();
      },
      overridden: function (name, active) {
        if (overridden.has(name) === active) return;
        if (active) overridden.add(name); else overridden.delete(name);
        requestRefresh();
      },
      refresh: requestRefresh,
      reconcileResults: results.reconcile,
      showResults: results.select,
    };
  }
  global.GraphcalReportWorkspace = { mount: mount };
})(window);

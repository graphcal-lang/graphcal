// Adaptive report workspace: DOM shell over explicitly registered typed controls.
// The original controls remain the advanced form/raw surface. Both presentations
// use the same handlers and draft lifecycle; this module never creates bindings.
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
    var rows = new Map();
    var expanded = new Map();
    var loaders = new Set();
    var dirty = true;
    var serial = 0;
    var refreshing = false;
    var advancedMode = false;
    var searchLimited = false;
    var lastKeys = [];

    var workspace = element("div", "report-workspace");
    main.appendChild(workspace);
    workspace.appendChild(inputs);
    document.body.classList.add("report-workspace-active");
    var original = element("div", "outline-advanced");
    original.hidden = true;
    original.appendChild(inputs.querySelector(".cards"));
    inputs.appendChild(original);
    var heading = inputs.querySelector("h2");
    var head = element("div", "outline-heading");
    if (heading) head.appendChild(heading);
    head.appendChild(element("p", "outline-help", "Each run applies the whole parameter. Search nested fields or pin frequently used inputs."));
    var search = element("input", "outline-search");
    search.type = "search";
    search.placeholder = "Search names, field paths, or descriptions…";
    search.setAttribute("aria-label", "Search inputs");
    head.appendChild(search);
    var options = element("div", "outline-options");
    var pinnedOnly = element("input");
    pinnedOnly.type = "checkbox";
    var pinnedLabel = element("label");
    pinnedLabel.append(pinnedOnly, document.createTextNode(" Pinned only"));
    var advanced = button("Advanced controls", function () {
      advancedMode = !advancedMode;
      original.hidden = !advancedMode;
      explorer.hidden = advancedMode;
      text(advanced, advancedMode ? "Back to outline" : "Advanced controls");
      advanced.setAttribute("aria-expanded", String(advancedMode));
      if (!advancedMode) parameters.forEach(function (parameter) { parameter.showForm(); });
      refresh(true);
    });
    advanced.setAttribute("aria-expanded", "false");
    var count = element("span", "outline-count");
    count.setAttribute("role", "status");
    options.append(pinnedLabel, advanced, count);
    head.appendChild(options);
    var toolbar = inputs.querySelector(".input-toolbar");
    if (toolbar) head.appendChild(toolbar);
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
    function rowFor(field, contextLabel) {
      var cached = rows.get(field.key);
      if (!cached || cached.kind !== field.kind) {
        var row = element("div", "outline-row");
        row.setAttribute("data-parameter", field.parameter);
        var pin = button("☆", function () {
          if (pins.has(field.key)) pins.delete(field.key); else pins.add(field.key);
          dirty = true;
          refresh();
        }, "outline-pin");
        pin.setAttribute("aria-label", "Pin input " + field.labels.join(" › "));
        var label = element("label", "outline-label");
        var widget = element(field.kind === "select" ? "select" : "input", "outline-value");
        if (field.kind !== "select") widget.type = field.kind === "boolean" ? "checkbox" : "text";
        widget.spellcheck = false;
        serial += 1;
        widget.id = "outline-field-" + serial;
        label.htmlFor = widget.id;
        widget.setAttribute("aria-label", field.labels.join(" "));
        var errorLine = element("p", "outline-error");
        errorLine.id = widget.id + "-error";
        widget.setAttribute("aria-describedby", errorLine.id);
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
        var menu = element("div", "outline-action-menu");
        menu.appendChild(element("small", "", "Whole parameter: " + field.parameter));
        var parameter = parameters.get(field.parameter);
        parameter.actions.forEach(function (action) {
          menu.appendChild(button(action.label, function () { action.run(); actions.open = false; }));
        });
        if (field.description) menu.appendChild(element("p", "", field.description));
        actions.append(summary, menu);
        row.append(pin, label, widget, actions, errorLine);
        cached = { row: row, pin: pin, label: label, widget: widget, error: errorLine, kind: field.kind, context: field.labels.at(-1) };
        rows.set(field.key, cached);
      }
      if (typeof contextLabel === "string") cached.context = contextLabel;
      text(cached.label, pins.has(field.key) ? field.labels.join(" › ") : cached.context);
      cached.label.title = field.labels.join(" › ");
      text(cached.pin, pins.has(field.key) ? "★" : "☆");
      cached.pin.setAttribute("aria-pressed", String(pins.has(field.key)));
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
        // The native draft is authoritative, including reset/default/raw changes
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
      if (all.length === 1 && !all[0].constructorControl) return rowFor(all[0], all[0].labels.slice(path.length - 1).join(" › "));
      var detail = element("details", "outline-group");
      var summary = element("summary");
      summary.append(element("span", "", group.name), element("small", "", visible.length + (visible.length === 1 ? " field" : " fields")));
      // Standard serialization is confined to DOM identity/cache boundaries.
      var key = JSON.stringify(path);
      detail.open = Boolean(query) || visible.some(fieldError) || (expanded.has(key) ? expanded.get(key) : all.length <= 4);
      detail.addEventListener("toggle", function () { if (!query && detail.isConnected) expanded.set(key, detail.open); });
      var body = element("div", "outline-group-body");
      group.rows.filter(function (field) { return visible.includes(field); }).forEach(function (field) { body.appendChild(rowFor(field, field.labels.at(-1))); });
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
        if (search.value.trim() && !advancedMode) {
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
          favoriteRows.replaceChildren.apply(favoriteRows, favoriteItems.map(function (field) { return rowFor(field); }));
          favorites.hidden = !favoriteItems.length;
          content.replaceChildren();
          if (!pinnedOnly.checked) {
            var tree = state.tree(items);
            tree.rows.filter(function (field) { return matching.includes(field) && !pins.has(field.key); }).forEach(function (field) { content.appendChild(rowFor(field, field.labels.at(-1))); });
            tree.groups.forEach(function (group) {
              var node = renderGroup(group, [group.name], query);
              if (node) content.appendChild(node);
            });
          }
          allHeading.hidden = pinnedOnly.checked;
          content.hidden = pinnedOnly.checked;
          empty.hidden = (pinnedOnly.checked ? favoriteItems.length : matching.length) !== 0;
          text(count, items.length + " loaded fields · " + matching.length + " matches · " + pins.size + " pinned");
          lastKeys = keys;
          dirty = false;
        } else items.forEach(function (field) { rowFor(field); });
        fields.forEach(function (field, key) { if (!field.widget.isConnected) fields.delete(key); });
        rows.forEach(function (cached, key) { if (!fields.has(key)) { cached.row.remove(); rows.delete(key); } });
        more.hidden = !activeLoaders().length || pinnedOnly.checked;
        loaders.forEach(function (loader) { if (!loader.isConnected) loaders.delete(loader); });
        var messages = Array.from(errors.values()).filter(function (error) { return error.message; });
        var changed = Array.from(statuses).filter(function (entry) { return entry[1]; });
        var statusText = changed.slice(0, 3).map(function (entry) { return entry[0] + ": " + entry[1]; }).join(" ");
        if (changed.length > 3) statusText += " And " + (changed.length - 3) + " more parameters.";
        text(notice, messages.length ? "Input rejected. The last successful results remain visible. Open Advanced controls to inspect all parameter errors." : searchLimited ? "Search is limited to the loaded fields. Show more entries to continue." : statusText);
        notice.hidden = !messages.length && !searchLimited && !changed.length;
      } finally {
        refreshing = false;
        if (!advancedMode && focused && focused.isConnected && explorer.contains(focused)) focused.focus({ preventScroll: true });
      }
    }
    search.addEventListener("input", function () { refresh(true); });
    pinnedOnly.addEventListener("change", function () { refresh(true); });
    return {
      parameter: function (name, actions, showForm) { parameters.set(name, { actions: actions, showForm: showForm }); },
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
      status: function (name, message) { statuses.set(name, message); requestRefresh(); },
      refresh: requestRefresh,
      reconcileResults: results.reconcile,
    };
  }
  global.GraphcalReportWorkspace = { mount: mount };
})(window);

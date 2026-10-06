// Results-pane DOM shell. Moves existing result cards rather than copying them,
// preserving evaluator patch targets, chart instances, table focus and scroll.
(function (global) {
  "use strict";
  // Pushpin icon shared by every pin button. CSS draws it tilted and outlined
  // when unpinned, upright and filled when the button is aria-pressed.
  var SVG_NS = "http://www.w3.org/2000/svg";
  function pinIcon() {
    var svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("class", "pin-icon");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    ["M6.25 1.75V6.25L3.75 9H12.25L9.75 6.25V1.75Z", "M5 1.75H11", "M8 9V14.5"].forEach(function (d) {
      var path = document.createElementNS(SVG_NS, "path");
      path.setAttribute("d", d);
      svg.appendChild(path);
    });
    return svg;
  }
  function mount(main, container) {
    function element(tag, className, text) {
      var node = document.createElement(tag);
      if (className) node.className = className;
      if (text !== undefined) node.textContent = text;
      return node;
    }
    function button(label, action, className) {
      var node = element("button", className, label);
      node.type = "button";
      node.addEventListener("click", action);
      return node;
    }
    var results = element("div", "workspace-results");
    results.id = "workspace-results";
    results.tabIndex = -1;
    container.appendChild(results);
    var heading = element("div", "workspace-result-heading");
    var tabs = element("nav", "workspace-result-tabs");
    tabs.setAttribute("aria-label", "Result sections");
    heading.append(element("h2", "", "Results"), tabs);
    var search = element("input", "outline-search");
    search.type = "search";
    search.placeholder = "Filter by name…";
    search.setAttribute("aria-label", "Filter results by name");
    var body = element("div", "workspace-result-body");

    // Pinned board: values, checks, and plots the reader keeps in view while
    // editing inputs. Items move here (never copies), so patching still finds them.
    var board = element("section", "workspace-pinned is-empty");
    board.setAttribute("aria-label", "Pinned results");
    var boardHead = element("div", "pinned-heading");
    var boardHint = element("p", "pinned-hint");
    boardHint.append("Use ", pinIcon(), " on a value, plot, or check to keep it here while you edit inputs.");
    var unpinAll = button("Unpin all", function () {
      Array.from(placeholders.keys()).forEach(function (item) { setPinned(item, false); });
    }, "pinned-clear");
    boardHead.append(element("h3", "", "Pinned"), boardHint, unpinAll);
    // Each kind of result has its own pinned group and its own home section.
    var KINDS = {
      values: { selector: ".card[data-decl]", group: element("div", "pinned-values"), home: "#values .cards", noun: "value" },
      checks: { selector: ".check[data-check]", group: element("ul", "checks pinned-checks"), home: "#checks .checks", noun: "check" },
      plots: { selector: "figure[data-figure]", group: element("div", "pinned-plots"), home: "#plots", noun: "plot" },
    };
    // Values and checks stack in one column; plots sit beside them at their
    // authored width when the pane is wide enough, and wrap below otherwise.
    var boardStack = element("div", "pinned-stack");
    boardStack.append(KINDS.values.group, KINDS.checks.group);
    var boardBody = element("div", "pinned-body");
    boardBody.append(boardStack, KINDS.plots.group);
    board.append(boardHead, boardBody);
    var resizer = element("div", "pinned-resizer");
    resizer.tabIndex = 0;
    resizer.setAttribute("role", "separator");
    resizer.setAttribute("aria-orientation", "horizontal");
    resizer.setAttribute("aria-label", "Resize the pinned area (arrow keys; double-click to reset)");
    resizer.title = "Drag to resize the pinned area; double-click to reset";
    results.append(board, resizer, heading, search, body);
    var buttons = new Map();
    var activeSection = "values";
    // Every filterable result carries its declaration name in one data attribute.
    var FILTERABLE = ".card[data-decl], figure[data-figure], .check[data-check]";
    // Values with at most this many cells are shown inline rather than collapsed.
    var INLINE_CELL_LIMIT = 12;
    function itemName(item) { return item.dataset.decl || item.dataset.figure || item.dataset.check || ""; }
    function kindOf(item) {
      return Object.keys(KINDS).find(function (kind) { return item.matches(KINDS[kind].selector); });
    }
    function itemsOf(kind, section) {
      var selector = KINDS[kind].selector;
      return Array.from(section.querySelectorAll(selector)).concat(Array.from(KINDS[kind].group.querySelectorAll(selector)));
    }

    // Pin state lives here, keyed by element: the evaluator rewrites result
    // classes (e.g. a check's status), so a class cannot carry it.
    var placeholders = new Map();
    function setPinned(item, on) {
      if (on === placeholders.has(item)) return;
      var kind = kindOf(item);
      if (on) {
        item.classList.remove("workspace-result-filtered");
        var placeholder = document.createComment("pinned " + KINDS[kind].noun + " position");
        item.before(placeholder);
        placeholders.set(item, placeholder);
        KINDS[kind].group.appendChild(item);
      } else {
        var origin = placeholders.get(item);
        placeholders.delete(item);
        if (origin && origin.isConnected) origin.replaceWith(item);
        else {
          var home = document.querySelector(KINDS[kind].home);
          if (home) home.appendChild(item);
        }
      }
      var pin = item.querySelector(".workspace-output-pin");
      if (pin) {
        pin.setAttribute("aria-pressed", String(on));
        pin.title = on ? "Unpin" : "Pin to keep in view while editing inputs";
      }
      updateBoard();
      updateCounts();
      show();
    }
    function addPin(item, host, atStart) {
      if (item.querySelector(".workspace-output-pin")) return;
      var pin = button("", function () { setPinned(item, !placeholders.has(item)); }, "workspace-output-pin");
      pin.appendChild(pinIcon());
      pin.setAttribute("aria-label", "Pin " + KINDS[kindOf(item)].noun + " " + itemName(item));
      pin.setAttribute("aria-pressed", "false");
      pin.title = "Pin to keep in view while editing inputs";
      if (atStart) host.prepend(pin); else host.appendChild(pin);
    }
    function updateBoard() {
      var count = placeholders.size;
      board.classList.toggle("is-empty", count === 0);
      boardHint.hidden = count > 0;
      unpinAll.hidden = count < 2;
      resizer.hidden = count === 0;
    }

    // A reader-set height replaces the default cap until reset by double-click.
    var boardHeight = null;
    function applyBoardHeight() {
      board.classList.toggle("is-sized", boardHeight !== null);
      if (boardHeight === null) board.style.removeProperty("height");
      else board.style.height = boardHeight + "px";
    }
    function clampHeight(height) {
      var max = Math.max(120, results.clientHeight - 200);
      return Math.round(Math.min(Math.max(height, 64), max));
    }
    resizer.addEventListener("pointerdown", function (event) {
      if (event.button !== 0) return;
      event.preventDefault();
      var startY = event.clientY;
      var startHeight = board.getBoundingClientRect().height;
      resizer.setPointerCapture(event.pointerId);
      resizer.classList.add("is-dragging");
      function move(moveEvent) {
        boardHeight = clampHeight(startHeight + moveEvent.clientY - startY);
        applyBoardHeight();
      }
      function end() {
        resizer.classList.remove("is-dragging");
        resizer.removeEventListener("pointermove", move);
        resizer.removeEventListener("pointerup", end);
        resizer.removeEventListener("pointercancel", end);
      }
      resizer.addEventListener("pointermove", move);
      resizer.addEventListener("pointerup", end);
      resizer.addEventListener("pointercancel", end);
    });
    resizer.addEventListener("keydown", function (event) {
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
      event.preventDefault();
      var step = (event.shiftKey ? 96 : 24) * (event.key === "ArrowDown" ? 1 : -1);
      boardHeight = clampHeight(board.getBoundingClientRect().height + step);
      applyBoardHeight();
    });
    resizer.addEventListener("dblclick", function () {
      boardHeight = null;
      applyBoardHeight();
    });
    function show() {
      Array.from(body.children).forEach(function (section) {
        section.classList.toggle("workspace-section-inactive", section.id !== activeSection);
      });
      buttons.forEach(function (entry, id) { entry.button.setAttribute("aria-pressed", String(id === activeSection)); });
      var query = search.value.trim();
      body.querySelectorAll(FILTERABLE).forEach(function (item) {
        item.classList.toggle("workspace-result-filtered", !global.GraphcalReportOutlineState.matches([itemName(item)], "", query));
      });
      var active = activeSection ? document.getElementById(activeSection) : null;
      search.hidden = !active || !active.querySelector(FILTERABLE);
    }
    function select(id) {
      if (!buttons.has(id) || buttons.get(id).button.hidden) return false;
      activeSection = id;
      show();
      return true;
    }
    // Tab counts are read back from the patched DOM (including pinned items), so
    // they always describe exactly what the reader can see.
    function summarize(section) {
      var items;
      var bad;
      if (section.id === "values") {
        items = itemsOf("values", section);
        bad = items.filter(function (card) { return card.querySelector(".error-chip"); }).length;
        return bad
          ? { text: bad + (bad === 1 ? " error" : " errors"), tone: "bad", title: bad + " of " + items.length + " values failed to evaluate" }
          : { text: String(items.length), tone: "", title: items.length + " values" };
      }
      if (section.id === "plots") {
        items = itemsOf("plots", section);
        bad = items.filter(function (figure) { return figure.querySelector(".error-chip"); }).length;
        return bad
          ? { text: bad + " failed", tone: "bad", title: bad + " of " + items.length + " plots could not be drawn" }
          : { text: String(items.length), tone: "", title: items.length + " plots" };
      }
      if (section.id === "checks") {
        items = itemsOf("checks", section);
        bad = items.filter(function (check) { return check.matches(".check--fail, .check--error, .check--blocked"); }).length;
        return { text: (items.length - bad) + "/" + items.length, tone: bad ? "bad" : "pass", title: bad ? bad + " of " + items.length + " checks are not passing" : "All " + items.length + " checks pass" };
      }
      var notices = section.querySelectorAll(".notice").length;
      return notices ? { text: String(notices), tone: "bad", title: notices + " notices" } : null;
    }
    function updateCounts() {
      buttons.forEach(function (entry, id) {
        var section = document.getElementById(id);
        var summary = section ? summarize(section) : null;
        entry.count.hidden = !summary;
        if (!summary) return;
        entry.count.textContent = summary.text;
        entry.count.className = "tab-count" + (summary.tone ? " tab-count--" + summary.tone : "");
        entry.count.title = summary.title;
      });
    }
    function valueCells(value) {
      return value.querySelectorAll("td").length;
    }
    // Describe a collapsed value by its shape, so readers know what opening it shows.
    function describeValue(disclosure) {
      var meta = disclosure.querySelector(".details-meta");
      var value = disclosure.querySelector('[data-role="value"]');
      if (!meta || !value) return;
      var description = "Value";
      var slices = value.querySelectorAll(".slices > table");
      var grid = value.querySelector("table.grid");
      var entries = value.querySelector("table.entries");
      if (value.classList.contains("error-chip")) description = "Error";
      else if (slices.length) {
        var first = slices[0];
        description = slices.length + " slices of " + first.querySelectorAll("tbody tr").length + " × " + (first.querySelectorAll("thead th").length - 1);
      } else if (grid) {
        description = grid.querySelectorAll("tbody tr").length + " × " + (grid.querySelectorAll("thead th").length - 1) + " grid";
      } else if (entries) {
        var count = entries.querySelectorAll("tbody tr").length;
        description = count + (count === 1 ? " entry" : " entries");
      }
      if (meta.textContent !== description) meta.textContent = description;
    }
    function reconcile() {
      Array.from(main.children).forEach(function (section) {
        if (section.matches("section, footer")) body.appendChild(section);
      });
      var visible = Array.from(body.children).filter(function (section) { return !section.hidden; });
      if (!visible.some(function (section) { return section.id === activeSection; })) activeSection = visible.length ? visible[0].id : null;
      Array.from(body.children).forEach(function (section) {
        if (!buttons.has(section.id)) {
          var h2 = section.querySelector("h2");
          // The tab's name stays its label; the live count is its description.
          var count = element("span", "tab-count");
          count.id = "result-tab-count-" + section.id;
          count.setAttribute("aria-hidden", "true");
          var tab = button("", function () { select(section.id); });
          tab.setAttribute("aria-describedby", count.id);
          tab.append(element("span", "tab-label", h2 ? h2.textContent : section.id), count);
          buttons.set(section.id, { button: tab, count: count });
          tabs.appendChild(tab);
        }
        buttons.get(section.id).button.hidden = section.hidden;
      });
      updateCounts();
      results.querySelectorAll(".card[data-decl]").forEach(function (card) {
        var disclosure = card.querySelector(".workspace-output-details");
        if (disclosure && card.querySelector('[data-role="value"].error-chip')) disclosure.open = true;
        if (disclosure) describeValue(disclosure);
        if (card.querySelector(".workspace-output-pin")) return;
        var value = card.querySelector(".value-scroll");
        if (value) {
          disclosure = element("details", "workspace-output-details");
          var summaryLine = element("summary");
          summaryLine.appendChild(element("span", "details-meta"));
          disclosure.appendChild(summaryLine);
          value.before(disclosure);
          disclosure.appendChild(value);
          describeValue(disclosure);
          // Small tables read better inline; large ones stay collapsed until asked for.
          disclosure.open = valueCells(value) <= INLINE_CELL_LIMIT;
        }
        addPin(card, card, true);
      });
      results.querySelectorAll(KINDS.checks.selector).forEach(function (check) { addPin(check, check, false); });
      // The figure caption survives re-rendering (the runtime keeps it), so the pin lives there.
      results.querySelectorAll(KINDS.plots.selector).forEach(function (figure) {
        var caption = figure.querySelector("figcaption");
        if (caption) addPin(figure, caption, false);
      });
      show();
    }
    search.addEventListener("input", show);
    // Preserve programmatic/hash navigation even though the workspace uses tabs.
    function followHash() {
      select(location.hash.slice(1));
    }
    global.addEventListener("hashchange", followHash);
    updateBoard();
    reconcile();
    followHash();
    return { reconcile: reconcile, select: select };
  }
  global.GraphcalReportResults = { mount: mount, pinIcon: pinIcon };
})(window);

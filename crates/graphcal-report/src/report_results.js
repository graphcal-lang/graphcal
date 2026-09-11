// Results-pane DOM shell. Moves existing result cards rather than copying them,
// preserving evaluator patch targets, chart instances, table focus and scroll.
(function (global) {
  "use strict";
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
    heading.append(element("h2", "", "Results"), element("p", "", "Pin outputs to keep them in view while editing."));
    var tabs = element("nav", "workspace-result-tabs");
    tabs.setAttribute("aria-label", "Result sections");
    var search = element("input", "outline-search");
    search.type = "search";
    search.placeholder = "Filter output names…";
    search.setAttribute("aria-label", "Filter output names");
    var body = element("div", "workspace-result-body");
    var pinned = element("div", "workspace-pinned-outputs");
    pinned.setAttribute("aria-label", "Pinned outputs");
    results.append(heading, tabs, search, pinned, body);
    var buttons = new Map();
    var activeSection = "values";
    function show() {
      Array.from(body.children).forEach(function (section) {
        section.classList.toggle("workspace-section-inactive", section.id !== activeSection);
      });
      buttons.forEach(function (button, id) { button.setAttribute("aria-pressed", String(id === activeSection)); });
      body.querySelectorAll(".card").forEach(function (card) {
        card.classList.toggle("workspace-result-filtered", !global.GraphcalReportOutlineState.matches([card.dataset.decl], "", search.value.trim()));
      });
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
          var tab = button(h2 ? h2.textContent : section.id, function () { activeSection = section.id; show(); });
          buttons.set(section.id, tab);
          tabs.appendChild(tab);
        }
        buttons.get(section.id).hidden = section.hidden;
      });
      results.querySelectorAll(".card").forEach(function (card) {
        var disclosure = card.querySelector(".workspace-output-details");
        if (disclosure && card.querySelector('[data-role="value"].error-chip')) disclosure.open = true;
        if (card.querySelector(".workspace-output-pin")) return;
        var value = card.querySelector(".value-scroll");
        if (value) {
          disclosure = element("details", "workspace-output-details");
          disclosure.appendChild(element("summary", "", "View value"));
          value.before(disclosure);
          disclosure.appendChild(value);
        }
        var placeholder = document.createComment("pinned output position");
        var pin = button("Pin", function () {
          var on = card.classList.toggle("workspace-output-pinned");
          if (on) {
            card.classList.remove("workspace-result-filtered");
            card.before(placeholder);
            pinned.appendChild(card);
          } else placeholder.replaceWith(card);
          pin.setAttribute("aria-pressed", String(on));
          pin.textContent = on ? "Unpin" : "Pin";
          show();
        }, "workspace-output-pin");
        pin.setAttribute("aria-label", "Pin output " + card.dataset.decl);
        pin.setAttribute("aria-pressed", "false");
        card.prepend(pin);
      });
      show();
    }
    search.addEventListener("input", show);
    // Preserve programmatic/hash navigation even though the workspace uses tabs.
    function followHash() {
      var id = location.hash.slice(1);
      if (buttons.has(id)) { activeSection = id; show(); }
    }
    global.addEventListener("hashchange", followHash);
    reconcile();
    followHash();
    return { reconcile: reconcile };
  }
  global.GraphcalReportResults = { mount: mount };
})(window);

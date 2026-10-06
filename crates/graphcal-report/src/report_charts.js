// Report chart theme. Wraps vegaEmbed so the static first paint and every live
// re-render share one look, taken from the page's CSS tokens (including dark
// mode). Specs pass through unchanged; only the embed `config` is supplied.
// Hosts that wrap vegaEmbed themselves (the playground) load this script first,
// so their options, such as a sandboxed loader, reach every render, including
// the re-renders that follow a color-scheme change.
(function (global) {
  "use strict";
  var embed = global.vegaEmbed;
  if (typeof embed !== "function") return;

  function theme() {
    var css = getComputedStyle(document.documentElement);
    function token(name, fallback) { return css.getPropertyValue(name).trim() || fallback; }
    var ink = token("--ink", "#17202b");
    var muted = token("--muted", "#566272");
    var line = token("--line", "#dce1e7");
    var strong = token("--line-strong", "#bcc5cf");
    return {
      background: "transparent",
      font: token("--font-ui", "sans-serif"),
      view: { stroke: line },
      axis: {
        labelColor: muted, titleColor: muted, domainColor: strong, tickColor: strong, gridColor: line,
        labelFontSize: 11, titleFontSize: 11, titleFontWeight: 600, titlePadding: 8,
      },
      legend: { labelColor: muted, titleColor: muted, labelFontSize: 11, titleFontSize: 11, titleFontWeight: 600 },
      title: { color: ink, fontSize: 13, fontWeight: 600, anchor: "start", offset: 10 },
    };
  }

  // Live renders by mount element, so a color-scheme change can redraw them and
  // views whose mount left the document are finalized.
  var live = new Map();
  function sweep() {
    live.forEach(function (entry, target) {
      if (target.isConnected) return;
      live.delete(target);
      if (entry.result) entry.result.view.finalize();
    });
  }
  function render(target, entry) {
    var promise = embed(target, entry.spec, Object.assign({}, entry.options, { config: theme() }));
    promise.then(function (result) { entry.result = result; }, function () {});
    return promise;
  }
  function themedEmbed(el, spec, options) {
    sweep();
    var target = typeof el === "string" ? document.querySelector(el) : el;
    var entry = { spec: spec, options: options || {}, result: null };
    if (!target) return embed(el, spec, Object.assign({}, entry.options, { config: theme() }));
    var previous = live.get(target);
    if (previous && previous.result) previous.result.view.finalize();
    live.set(target, entry);
    return render(target, entry);
  }
  Object.keys(embed).forEach(function (key) { themedEmbed[key] = embed[key]; });
  global.vegaEmbed = themedEmbed;

  // Follow the OS light/dark switch without waiting for the next recalculation.
  var scheme = global.matchMedia ? global.matchMedia("(prefers-color-scheme: dark)") : null;
  if (scheme && scheme.addEventListener) scheme.addEventListener("change", function () {
    sweep();
    live.forEach(function (entry, target) {
      if (entry.result) entry.result.view.finalize();
      render(target, entry);
    });
  });
})(window);

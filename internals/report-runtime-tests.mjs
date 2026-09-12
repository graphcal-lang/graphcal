// Exercise the production report functions with a minimal DOM adapter and the
// actual embedded Wasm exports. DOM layout is not modeled by this Node suite.
import "./report-outline-tests.mjs";
import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { runInNewContext } from "node:vm";

// `just wasm-report` exports the bundle from the native build's OUT_DIR.
const glue = readFileSync("target/wasm-report/pkg/graphcal_wasm.js", "utf8");
globalThis.self = globalThis;
(0, eval)(`${glue}\nglobalThis.reportEngine = wasm_bindgen;`);
await reportEngine({ module_or_path: readFileSync("target/wasm-report/pkg/graphcal_wasm_bg.wasm") });

class Element {
  constructor(tag = "div") { this.tag = tag; this.children = []; this.events = {}; this.textContent = ""; this.className = ""; }
  appendChild(child) { this.children.push(child); return child; }
  insertBefore(child) { return this.appendChild(child); }
  addEventListener(event, listener) { this.events[event] = listener; }
  setAttribute(name, value) { this[name] = value; }
  replaceChildren(...children) { this.children = children; }
  querySelector(selector) { return selector === ".cards" ? this.children.find(child => child.className === "cards") : null; }
}
function runtime(source, baseline = [], files = [], enableAutoRun = false) {
  const entry = files.length ? "src/demo/main.gcl" : "main.gcl";
  const project = { entry, files: [{ path: entry, content: source }, ...files] };
  const prepared = reportEngine.prepareProject(project);
  const cards = new Map(prepared.parameterPorts().map(port => [port.name, new Element()]));
  const inputsSection = new Element("section");
  const inputCards = new Element();
  inputCards.className = "cards";
  inputsSection.appendChild(inputCards);
  const main = new Element("main");
  const payloads = {
    "graphcal-project": JSON.stringify(project), "graphcal-baseline": JSON.stringify(baseline),
    "graphcal-engine-glue": "unused", "graphcal-engine-wasm": "unused",
  };
  let nextTimer = 0;
  const timers = new Map();
  const context = {
    document: {
      body: new Element(), createElement: tag => new Element(tag), createTextNode: text => ({ textContent: text }),
      getElementById: id => id === "inputs" ? inputsSection
        : ["incomplete-notice", "call-notices"].includes(id) ? main.children.find(child => child.id === id) || null
        : { textContent: payloads[id] },
      querySelector: selector => selector === "main" ? main : cards.get(selector.match(/data-decl="([^"]+)"/)?.[1]),
    },
    window: {},
    setTimeout(callback) { nextTimer += 1; timers.set(nextTimer, callback); return nextTimer; },
    clearTimeout(id) { timers.delete(id); },
  };
  // Expose the production mount's functional UI core in this test only,
  // replacing transport startup.
  runInNewContext(readFileSync("crates/graphcal-report/src/report_form_state.js", "utf8"), context);
  const code = readFileSync("crates/graphcal-report/src/report_runtime.js", "utf8");
  const startup = "  startTransport();\n  }";
  assert.equal(code.split(startup).length, 2);
  runInNewContext(code.replace(startup,
    "  globalThis.api = { buildControls, currentBindings, patchParamControls, patchEvaluationNotices, controls, resetButton, renderView };\n  }"), context);
  context.window.GraphcalReport.mount({ baselineBindings: baseline, createTransport() {} });
  const toolbarNodes = node => [node, ...node.children.flatMap(child => child instanceof Element ? toolbarNodes(child) : [])];
  const autoRunToggle = toolbarNodes(inputsSection).find(child => child["aria-describedby"] === "auto-run-description");
  assert.equal(autoRunToggle.checked, true, "auto run is enabled by default");
  if (!enableAutoRun) {
    autoRunToggle.checked = false;
    autoRunToggle.events.change();
  }
  const { api } = context;
  api.buildControls(prepared.parameterPorts(), prepared.evaluateBindings(baseline).evaluation);
  const bindings = () => JSON.parse(JSON.stringify(api.currentBindings()));
  const evaluate = () => {
    const outcome = prepared.evaluateBindings(bindings());
    assert.equal(outcome.status, "evaluated", JSON.stringify(outcome));
    api.patchParamControls(outcome.evaluation);
    return outcome.evaluation.values;
  };
  const descendants = element => [element, ...element.children.flatMap(child => child instanceof Element ? descendants(child) : [])];
  const field = name => descendants(cards.get(name)).find(child => child.className === "control-field");
  const edit = (name, value) => { const input = field(name); input.value = value; input.events.input(); };
  const apply = name => descendants(cards.get(name)).find(child => child.className === "control-apply").events.click();
  const flushTimers = () => {
    const callbacks = Array.from(timers.values());
    timers.clear();
    callbacks.forEach(callback => callback());
  };
  return { api, cards, main, descendants, field, edit, apply, autoRunToggle, flushTimers, bindings, evaluate, prepared };
}
const model = `
param input: Dimensionless = 2.0;
param doubled: Dimensionless = @input * 2.0;
param enabled: Bool = true;
param samples: Int[Fin(2)] = table[Fin(2)] { 1; 2; };
node result: Dimensionless = @doubled;
`;
const value = (values, name) => values.find(item => item.name === name).outcome.value;
{
  const run = runtime(`
    param active: Bool = true;
    dag model { node unfinished: Length = todo {}; pub node known: Length = 2.0 m; }
    node known: Length = if @active { @model()::known } else { 1.0 m };
  `);
  for (const active of [true, false, true, false]) {
    const outcome = run.prepared.evaluateBindings([{ name: "active", expr: String(active) }]);
    assert.equal(outcome.status, "evaluated");
    assert.equal(outcome.evaluation.incomplete, active);
    assert.equal(outcome.evaluation.has_errors, false);
    run.api.patchEvaluationNotices(outcome.evaluation);
    const banner = run.main.children.find(child => child.id === "incomplete-notice");
    const calls = run.main.children.find(child => child.id === "call-notices");
    assert.equal(banner.hidden, !active);
    assert.equal(calls.hidden, !active);
    assert.equal(calls.children.length, active ? 2 : 1, "no stale or duplicate call notices");
  }
  run.prepared.free();
}
console.log("report runtime: incomplete banners and call notices refresh with the evaluated model");
{
  const run = runtime(model, [], [], true);
  run.edit("input", "3.0");
  assert.deepEqual(run.bindings(), [], "auto run waits for a quiet input period");
  run.flushTimers();
  assert.deepEqual(run.bindings(), [{ name: "input", expr: "3.0" }]);
  assert.equal(value(run.evaluate(), "result").value, 6);
  run.prepared.free();
}
console.log("report runtime: auto run stages complete edits after a quiet period");

for (const baseline of [[], [{ name: "doubled", expr: "8.0" }]]) {
  const run = runtime(model, baseline);
  assert.deepEqual(run.bindings(), baseline);
  assert.equal(run.field("input").value, "2.0");
  run.edit("input", "3.0");
  assert.deepEqual(run.bindings(), baseline, "keystrokes remain unapplied");
  run.descendants(run.cards.get("input")).find(child => child.className === "control-discard").events.click();
  assert.equal(run.field("input").value, "2.0", "discard restores the accepted snapshot");
  run.edit("input", "3.0");
  run.apply("input");
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 6);
  assert.equal(run.field("doubled").value, baseline.length ? "8.0" : "6.0");
  run.edit("input", "5.0");
  run.apply("input");
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 10);
  run.edit("doubled", "12.0");
  run.apply("doubled");
  assert.equal(value(run.evaluate(), "result").value, 12);
  run.descendants(run.cards.get("doubled")).find(child => child.className === "control-clear").events.click();
  assert.equal(value(run.evaluate(), "result").value, 10);
  assert.deepEqual(run.bindings(), [{ name: "input", expr: "5.0" }]);
  run.descendants(run.cards.get("input")).find(child => child.className === "control-clear").events.click();
  assert.equal(value(run.evaluate(), "result").value, 4);
  assert.deepEqual(run.bindings(), []);
  run.api.resetButton.events.click();
  assert.deepEqual(run.bindings(), baseline);
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 4);
  assert.deepEqual(
    run.descendants(run.cards.get("samples")).filter(child => child.className === "control-field").map(child => child.value),
    ["1", "2"],
  );
  run.prepared.free();
}
{
  const run = runtime(model);
  run.edit("doubled", "99.0");
  run.edit("input", "3.0");
  run.apply("input");
  assert.equal(value(run.evaluate(), "doubled").value, 6);
  assert.equal(run.field("doubled").value, "99.0", "a reactive update does not overwrite an open draft");
  run.api.controls.get("doubled").restore();
  assert.equal(run.field("doubled").value, "6.0", "discard reloads the latest reactive default, not an obsolete snapshot");
  assert.deepEqual(run.bindings(), [{ name: "input", expr: "3.0" }]);
  run.prepared.free();
}
console.log("report runtime: reactive defaults, explicit bindings, clear and reset passed");

{
  const run = runtime(`
    pub type Choice { Amount(value: Dimensionless), Switch(enabled: Bool), }
    param choice: Choice = Amount(value: 2.0);
    param samples: Int[Fin(2)] = table[Fin(2)] { 1; 2; };
  `);
  const constructor = run.descendants(run.cards.get("choice")).find(
    child => typeof child.className === "string" && child.className.includes("control-constructor"),
  );
  constructor.value = "1";
  constructor.events.change();
  run.apply("choice");
  assert.deepEqual(run.bindings(), [], "missing constructor fields are not fabricated or applied");
  const checkbox = run.descendants(run.cards.get("choice")).find(child => child.type === "checkbox");
  checkbox.checked = true;
  checkbox.events.change();
  constructor.value = "0";
  constructor.events.change();
  const amount = run.descendants(run.cards.get("choice")).find(child => child.className === "control-field");
  amount.value = "7.0";
  amount.events.input();
  constructor.value = "1";
  constructor.events.change();
  assert.equal(
    run.descendants(run.cards.get("choice")).find(child => child.type === "checkbox").checked,
    true,
    "constructor toggles retain prior drafts",
  );
  run.apply("choice");
  let evaluated = run.evaluate();
  assert.equal(value(evaluated, "choice").type_name, "Switch");
  assert.equal(value(evaluated, "choice").fields[0].value.value, true);
  const entries = run.descendants(run.cards.get("samples")).filter(child => child.className === "control-field");
  entries[1].value = "9";
  entries[1].events.input();
  run.apply("samples");
  evaluated = run.evaluate();
  assert.equal(value(evaluated, "samples").entries[1].value.decimal, "9");
  assert.deepEqual(run.bindings(), [
    {
      name: "choice",
      value: {
        kind: "algebraic",
        definition: 0,
        constructor: 1,
        fields: [{ kind: "literal", expr: "true" }],
      },
    },
    {
      name: "samples",
      value: {
        kind: "indexed",
        entries: [
          { kind: "literal", expr: "1" },
          { kind: "literal", expr: "9" },
        ],
      },
    },
  ]);
  run.prepared.free();
}
console.log("structured controls: constructor drafts, nested fields and fixed-axis edits passed");

{
  const run = runtime(`
    pub type Chain { Link(value: Int, next: Chain), End(value: Int), }
    param chain: Chain = Link(value: 1, next: End(value: 2));
  `);
  const port = run.prepared.parameterPorts()[0];
  assert.equal(port.definitions.length, 1, "recursive schemas use finite arena references");
  const fields = run.descendants(run.cards.get("chain")).filter(child => child.className === "control-field");
  assert.deepEqual(fields.map(field => field.value), ["1", "2"]);
  fields[1].value = "9";
  fields[1].events.input();
  run.apply("chain");
  const chain = value(run.evaluate(), "chain");
  assert.equal(chain.fields[1].value.type_name, "End");
  assert.equal(chain.fields[1].value.fields[0].value.decimal, "9");
  run.prepared.free();
}
console.log("recursive controls: finite schema graph and deep edits passed");

{
  const run = runtime(`pub type Choice { Amount(value: Dimensionless), Off, }
    param choice: Choice = Off;`);
  run.descendants(run.cards.get("choice")).find(child => child.textContent === "Raw literal").events.click();
  const raw = run.descendants(run.cards.get("choice")).find(child => child.className === "control-raw-field");
  raw.value = "Amount(value: 6.0)";
  raw.events.input();
  assert.deepEqual(run.bindings(), [], "raw edits remain unapplied");
  run.apply("choice");
  assert.equal(value(run.evaluate(), "choice").type_name, "Amount");
  run.prepared.free();
}
console.log("structured controls: raw mode preserves explicit apply semantics");

const vegaContext = { console, structuredClone };
runInNewContext(readFileSync("crates/graphcal-report/assets/vega.min.js", "utf8"), vegaContext);
runInNewContext(readFileSync("crates/graphcal-report/assets/vega-lite.min.js", "utf8"), vegaContext);
function assertJsonObjects(item) {
  assert.ok(!(item instanceof Map), "Vega JSON must not contain Maps");
  if (item && typeof item === "object") {
    if (!Array.isArray(item)) assert.equal(Object.getPrototypeOf(item), Object.prototype);
    for (const child of Object.values(item)) assertJsonObjects(child);
  }
}
for (const fixture of ["figure_basic", "layer_basic"]) {
  const project = { entry: "main.gcl", files: [{ path: "main.gcl", content: readFileSync(`tests/fixtures/valid/${fixture}.gcl`, "utf8") }] };
  const prepared = reportEngine.prepareProject(project);
  for (const outcome of [reportEngine.evaluateProject(project), prepared.evaluateBindings([])]) {
    assert.equal(outcome.status, "evaluated");
    assert.ok(outcome.evaluation.figures.length >= 1);
    assert.ok(outcome.evaluation.figures.some(figure => figure.spec.hconcat || figure.spec.vconcat || figure.spec.concat || figure.spec.layer));
    for (const figure of outcome.evaluation.figures) {
      assertJsonObjects(figure.spec);
      assert.ok(vegaContext.vegaLite.compile(figure.spec).spec);
    }
  }
  prepared.free();
}
console.log("Wasm figure transport: ordinary nested objects compile with vendored Vega-Lite");

// Compile the real report projection, including scale-bound selections, against
// the shipped renderer. Mixed categorical/continuous axes must not warn.
for (const [x, y, encodings] of [
  ["Category#Alpha", "2.0", ["y"]],
  ["2.0", "Category#Alpha", ["x"]],
  ["1.0", "2.0", ["x", "y"]],
  ["Category#Alpha", "Category#Beta", []],
  ['datetime("2026-01-01T00:00:00Z")', "2.0", ["x", "y"]],
  ["Category#Alpha", 'datetime("2026-01-01T00:00:00Z")', ["y"]],
]) {
  const project = { entry: "main.gcl", files: [{ path: "main.gcl", content:
    `index Category = { Alpha, Beta }; plot p = { mark: point, encode: { x: ${x}, y: ${y} } };` }] };
  const prepared = reportEngine.prepareProject(project);
  for (const outcome of [reportEngine.evaluateProject(project), prepared.evaluateBindings([])]) {
    assert.equal(outcome.status, "evaluated");
    const spec = outcome.evaluation.figures[0].spec;
    assert.deepEqual(spec.params?.[0].select.encodings ?? [], encodings);
    const warnings = [];
    vegaContext.console = { ...console, warn: (...args) => warnings.push(args) };
    assert.ok(vegaContext.vegaLite.compile(spec).spec);
    assert.deepEqual(warnings, []);
  }
  prepared.free();
}
vegaContext.console = console;
console.log("pan/zoom: mixed, continuous, categorical and temporal projections compile without warnings");

for (const [lower, upper, slider] of [
  ["0", "10", true], ["9007199254740990", "9007199254740991", true],
  ["-9007199254740991", "-9007199254740990", true],
  ["9007199254740991", "9007199254740992", false],
  ["-9007199254740992", "-9007199254740991", false],
  ["-9007199254740991", "9007199254740991", false],
  ["-9223372036854775808", "9223372036854775807", false],
  [null, "9223372036854775807", false], ["-9223372036854775808", null, false],
]) {
  const bounds = [lower && `min: ${lower}`, upper && `max: ${upper}`].filter(Boolean).join(", ");
  const run = runtime(`param iterations: Int(${bounds}) = ${lower || upper}; param enabled: Bool = true;`);
  const ports = run.prepared.parameterPorts();
  assert.equal(ports.length, 2, "large bounds must not disable other controls");
  assert.equal(ports[0].control.lower ?? null, lower);
  assert.equal(ports[0].control.upper ?? null, upper);
  assert.equal(run.descendants(run.cards.get("iterations")).some(child => child.className === "control-slider"), slider);
  for (const endpoint of [lower, upper].filter(Boolean)) {
    run.edit("iterations", endpoint);
    run.apply("iterations");
    assert.equal(value(run.evaluate(), "iterations").decimal, endpoint);
  }
  run.prepared.free();
}
console.log("integer controls: exact i64 bounds, one-sided domains and safe sliders passed");

for (const [prefix, index, files] of [
  ["pub index Mode = { Nominal, Safe };", "Mode", []],
  ["import demo.modes as config;", "config::Mode", [{ path: "graphcal.toml", content: '[package]\nname = "demo"' }, { path: "src/demo/modes.gcl", content: "pub index Mode = { Nominal, Safe };" }]],
  ["import demo.modes::{ index Mode as Setting };", "Setting", [{ path: "graphcal.toml", content: '[package]\nname = "demo"' }, { path: "src/demo/modes.gcl", content: "pub index Mode = { Nominal, Safe };" }]],
]) {
  const run = runtime(`${prefix} param mode: Key<${index}> = ${index}#Nominal; param enabled: Bool = true;`, [], files);
  const select = run.descendants(run.cards.get("mode")).find(child => child.tag === "select");
  assert.equal(select.value, `${index}#Nominal`);
  assert.deepEqual(select.children.map(child => child.value), [`${index}#Nominal`, `${index}#Safe`]);
  const checkbox = run.descendants(run.cards.get("enabled")).find(child => child.tag === "input" && child.type === "checkbox");
  checkbox.checked = false;
  checkbox.events.change();
  run.apply("enabled");
  assert.equal(value(run.evaluate(), "mode").variant, "Nominal");
  assert.deepEqual(run.bindings(), [{ name: "enabled", expr: "false" }]);
  for (const expected of ["Nominal", "Safe"]) {
    const liveSelect = run.descendants(run.cards.get("mode")).find(child => child.tag === "select");
    liveSelect.value = `${index}#${expected}`;
    liveSelect.events.change();
    run.apply("mode");
    assert.equal(value(run.evaluate(), "mode").variant, expected);
  }
  run.prepared.free();
}
console.log("named-key controls: actual default and option expressions bind, including qualified and aliased indexes");

const rankSource = Array.from({ length: 6 }, (_, rank) => {
  const axes = Array(rank).fill("Fin(2)").join(", ");
  const expression = Array.from({ length: rank }, (_, i) => `for i${i}: Fin(2) { `).join("") + "7.0" + " }".repeat(rank);
  return `node rank${rank}: Dimensionless${rank ? `[${axes}]` : ""} = ${expression};`;
}).join("\n") + `\ntype Reading { Reading(samples: Int[Fin(2)]), }
node nested: Reading[Fin(2)] = for i: Fin(2) { Reading(samples: table[Fin(2)] { 11; 12; }) };`;
const temporary = mkdtempSync(join(tmpdir(), "graphcal-value-body-"));
try {
  const sourcePath = join(temporary, "main.gcl");
  const output = join(temporary, "report.html");
  writeFileSync(sourcePath, rankSource);
  const cli = spawnSync("target/debug/graphcal", ["report", "build", sourcePath, "--static", "--output", output], { encoding: "utf8", timeout: 20000 });
  assert.equal(cli.status, 0, cli.stderr);
  const html = readFileSync(output, "utf8");
  const run = runtime(rankSource);
  function texts(element) { return [element.textContent, ...element.children.flatMap(texts)].filter(Boolean); }
  for (const declaration of run.evaluate()) {
    const view = run.api.renderView(declaration.outcome.body, declaration.name);
    assert.equal(view["data-role"], "value");
    if (declaration.outcome.body.kind !== "scalar") {
      assert.equal(view.className, "value-scroll");
      assert.equal(view.role, "region");
      assert.equal(view.tabindex, "0");
      assert.equal(view["aria-label"], declaration.name + " values");
      assert.ok(view.children.every(child => child["data-role"] === undefined));
    }
    const rendered = texts(view);
    const card = html.split(`data-decl="${declaration.name}"`)[1].split("</article>")[0];
    const native = card.slice(card.indexOf("</h3>") + 5).replace(/<[^>]*>/g, "\n").split("\n").map(text => text.trim()).filter(Boolean);
    assert.deepEqual(rendered, native, `static/hydrated labels and leaves: ${declaration.name}`);
    if (declaration.name.startsWith("rank")) {
      assert.equal(rendered.filter(text => text === "7").length, 2 ** Number(declaration.name.slice(4)));
    }
  }
  run.prepared.free();
} finally { rmSync(temporary, { recursive: true, force: true }); }
console.log("value body: static and hydrated ranks 0–5 and nested structures have identical labels/leaves");

// Exercise the public mount contract, not a source-rewritten UI helper.
{
  const timers = new Map();
  const transports = [];
  const body = new Element();
  const context = {
    window: {},
    document: {
      body,
      createElement: tag => new Element(tag),
      createTextNode: text => ({ textContent: text }),
      getElementById: () => null,
      querySelector: () => body,
    },
    setTimeout(fn, milliseconds) { const id = {}; timers.set(id, { fn, milliseconds }); return id; },
    clearTimeout(id) { timers.delete(id); },
  };
  runInNewContext(readFileSync("crates/graphcal-report/src/report_form_state.js", "utf8"), context);
  runInNewContext(readFileSync("crates/graphcal-report/src/report_runtime.js", "utf8"), context);
  context.window.GraphcalReport.mount({
    baselineBindings: [{ name: "mass", expr: "12.0 kg" }],
    createTransport(callbacks) {
      const transport = { callbacks, messages: [], terminated: false,
        postMessage(message) { this.messages.push(message); },
        terminate() { this.terminated = true; },
      };
      transports.push(transport);
      return transport;
    },
  });
  const first = transports[0];
  first.callbacks.onMessage({ type: "ready", ports: [] });
  assert.deepEqual(JSON.parse(JSON.stringify(first.messages)), [
    { type: "evaluate", id: 1, bindings: [{ name: "mass", expr: "12.0 kg" }] },
  ]);
  first.callbacks.onMessage({ type: "result", id: 999, outcome: { status: "eval_error", message: "stale" } });
  assert.equal(body.children[0].textContent, "computing…");
  [...timers.values()].find(timer => timer.milliseconds === 10000).fn();
  assert.equal(first.terminated, true);
  assert.equal(transports.length, 2);
  transports[1].callbacks.onMessage({ type: "ready", ports: [] });
  assert.equal(transports[1].messages[0].id, 2);
  transports[1].callbacks.onMessage({ type: "result", id: 2, outcome: { status: "eval_error", message: "missing input" } });
  assert.equal(body.children[0].textContent, "evaluation failed: missing input");
}
console.log("report transport: baseline replay, stale response rejection and timeout replacement passed");

// Exercise the production report functions with a minimal DOM adapter and the
// actual embedded Wasm exports. DOM layout is not modeled by this Node suite.
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
  constructor(tag = "div") { this.tag = tag; this.children = []; this.events = {}; this.textContent = ""; }
  appendChild(child) { this.children.push(child); return child; }
  insertBefore(child) { return this.appendChild(child); }
  addEventListener(event, listener) { this.events[event] = listener; }
  setAttribute(name, value) { this[name] = value; }
  replaceChildren(...children) { this.children = children; }
}
function runtime(source, baseline = [], files = []) {
  const entry = files.length ? "src/demo/main.gcl" : "main.gcl";
  const project = { entry, files: [{ path: entry, content: source }, ...files] };
  const prepared = reportEngine.prepareProject(project);
  const cards = new Map(prepared.parameterPorts().map(port => [port.name, new Element()]));
  const payloads = {
    "graphcal-project": JSON.stringify(project), "graphcal-baseline": JSON.stringify(baseline),
    "graphcal-engine-glue": "unused", "graphcal-engine-wasm": "unused",
  };
  const context = {
    document: {
      body: new Element(), createElement: tag => new Element(tag), createTextNode: text => ({ textContent: text }),
      getElementById: id => ({ textContent: payloads[id] }),
      querySelector: selector => cards.get(selector.match(/data-decl="([^"]+)"/)?.[1]),
    },
    window: {}, setTimeout() {}, clearTimeout() {},
  };
  // Expose the production closure in this test only, replacing worker startup.
  const code = readFileSync("crates/graphcal-report/src/report_runtime.js", "utf8");
  const startup = "  try {\n    var glueSource";
  assert.equal(code.split(startup).length, 2);
  runInNewContext(code.slice(0, code.indexOf(startup)) +
    "globalThis.api = { buildControls, currentBindings, patchParamControls, controls, resetButton, renderView };})();", context);
  const { api } = context;
  api.buildControls(prepared.parameterPorts(), prepared.evaluateBindings(baseline).evaluation);
  const bindings = () => JSON.parse(JSON.stringify(api.currentBindings()));
  const evaluate = () => {
    const outcome = prepared.evaluateBindings(bindings());
    assert.equal(outcome.status, "evaluated", JSON.stringify(outcome));
    api.patchParamControls(outcome.evaluation);
    return outcome.evaluation.values;
  };
  const field = name => cards.get(name).children[0].children.find(child => child.className === "control-field");
  const edit = (name, value) => { const input = field(name); input.value = value; input.events.input(); };
  return { api, cards, field, edit, bindings, evaluate, prepared };
}
const model = `
param input: Dimensionless = 2.0;
param doubled: Dimensionless = @input * 2.0;
param enabled: Bool = true;
param samples: Int[Fin(2)] = table[Fin(2)] { 1; 2; };
node result: Dimensionless = @doubled;
`;
const value = (values, name) => values.find(item => item.name === name).outcome.value;
for (const baseline of [[], [{ name: "doubled", expr: "8.0" }]]) {
  const run = runtime(model, baseline);
  assert.deepEqual(run.bindings(), baseline);
  assert.equal(run.field("input").value, "2.0");
  run.edit("input", "3.0");
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 6);
  assert.equal(run.field("doubled").value, baseline.length ? "8.0" : "6.0");
  run.edit("input", "5.0");
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 10);
  run.edit("doubled", "12.0");
  assert.equal(value(run.evaluate(), "result").value, 12);
  run.edit("doubled", "");
  assert.equal(value(run.evaluate(), "result").value, 10);
  assert.deepEqual(run.bindings(), [{ name: "input", expr: "5.0" }]);
  run.cards.get("input").children[0].children.find(child => child.className === "control-clear").events.click();
  assert.equal(value(run.evaluate(), "result").value, 4);
  assert.deepEqual(run.bindings(), []);
  run.api.resetButton.events.click();
  assert.deepEqual(run.bindings(), baseline);
  assert.equal(value(run.evaluate(), "result").value, baseline.length ? 8 : 4);
  assert.equal(run.field("samples").value, "");
  run.prepared.free();
}
console.log("report runtime: reactive defaults, explicit bindings, clear and reset passed");

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
  assert.equal(run.cards.get("iterations").children[0].children.some(child => child.className === "control-slider"), slider);
  for (const endpoint of [lower, upper].filter(Boolean)) {
    run.edit("iterations", endpoint);
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
  const select = run.cards.get("mode").children[0].children.find(child => child.tag === "select");
  assert.equal(select.value, `${index}#Nominal`);
  assert.deepEqual(select.children.map(child => child.value), [`${index}#Nominal`, `${index}#Safe`]);
  const checkbox = run.cards.get("enabled").children[0].children[0].children[0];
  checkbox.checked = false;
  checkbox.events.change();
  assert.equal(value(run.evaluate(), "mode").variant, "Nominal");
  assert.deepEqual(run.bindings(), [{ name: "enabled", expr: "false" }]);
  for (const option of select.children) {
    select.value = option.value;
    select.events.change();
    assert.equal(value(run.evaluate(), "mode").variant, option.textContent);
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
    const rendered = texts(run.api.renderView(declaration.outcome.body));
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

// Exercise the production report functions with a minimal DOM adapter and the
// actual embedded Wasm exports. DOM layout is not modeled by this Node suite.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

const glue = readFileSync("crates/graphcal-cli/assets/report-engine/graphcal_wasm.js", "utf8");
globalThis.self = globalThis;
(0, eval)(`${glue}\nglobalThis.reportEngine = wasm_bindgen;`);
await reportEngine({ module_or_path: readFileSync("crates/graphcal-cli/assets/report-engine/graphcal_wasm_bg.wasm") });

class Element {
  constructor(tag = "div") { this.tag = tag; this.children = []; this.events = {}; this.textContent = ""; }
  appendChild(child) { this.children.push(child); return child; }
  insertBefore(child) { return this.appendChild(child); }
  addEventListener(event, listener) { this.events[event] = listener; }
  setAttribute(name, value) { this[name] = value; }
  replaceChildren(...children) { this.children = children; }
}
function runtime(source, baseline = []) {
  const project = { entry: "main.gcl", files: [{ path: "main.gcl", content: source }] };
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

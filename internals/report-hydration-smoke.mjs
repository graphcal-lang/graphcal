// Drive a hydrated report's embedded payload through the real browser engine
// under Node: decode the engine and project from the artifact, prepare the
// project, read the typed parameter ports, and re-evaluate with a binding —
// everything the in-page worker does except DOM patching.
//
// Usage: node internals/report-hydration-smoke.mjs <report.html>

import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const reportPath = process.argv[2];
assert.ok(reportPath, "usage: node report-hydration-smoke.mjs <report.html>");
const html = readFileSync(reportPath, "utf8");

function payload(id) {
  const marker = `id="${id}"`;
  const start = html.indexOf(marker);
  assert.notEqual(start, -1, `payload element ${id} is missing`);
  const open = html.indexOf(">", start);
  const close = html.indexOf("</script>", open);
  assert.notEqual(close, -1, `payload element ${id} is unterminated`);
  return html.slice(open + 1, close);
}

const project = JSON.parse(payload("graphcal-project"));
const baseline = JSON.parse(payload("graphcal-baseline"));
const glueSource = Buffer.from(payload("graphcal-engine-glue").trim(), "base64").toString("utf8");
const wasmBytes = new Uint8Array(Buffer.from(payload("graphcal-engine-wasm").trim(), "base64"));

assert.equal(project.entry, "rocket.gcl");
assert.ok(Array.isArray(project.files) && project.files.length >= 1);
assert.ok(Array.isArray(baseline));

// The no-modules glue targets browser globals and declares
// `let wasm_bindgen`, which stays inside the eval's lexical scope — capture
// it onto globalThis from within the same eval (the in-page worker instead
// concatenates glue and driver into one script, sharing that scope).
globalThis.self = globalThis;
(0, eval)(`${glueSource}\nglobalThis.wasm_bindgen = wasm_bindgen;`);
const engine = globalThis.wasm_bindgen;
assert.equal(typeof engine, "function", "no-modules glue must define wasm_bindgen");

await engine({ module_or_path: wasmBytes });
const prepared = engine.prepareProject(project);

const ports = prepared.parameterPorts();
const names = ports.map((port) => port.name);
assert.deepEqual(names, ["dry_mass", "fuel_mass", "isp"]);
const isp = ports[2];
assert.equal(isp.control.kind, "quantity");
assert.equal(isp.control.unit, "s");

const baselineOutcome = prepared.evaluateBindings([]);
assert.equal(baselineOutcome.status, "evaluated");
const deltaVBaseline = baselineOutcome.evaluation.values.find((v) => v.name === "delta_v");
assert.equal(deltaVBaseline.outcome.status, "value");

const boosted = prepared.evaluateBindings([{ name: "isp", expr: "450.0 s" }]);
assert.equal(boosted.status, "evaluated");
const deltaVBoosted = boosted.evaluation.values.find((v) => v.name === "delta_v");
assert.ok(
  deltaVBoosted.outcome.value.si_value > deltaVBaseline.outcome.value.si_value,
  "a higher isp must raise delta_v",
);

const rejected = prepared.evaluateBindings([{ name: "isp", expr: "450.0 kg" }]);
assert.equal(rejected.status, "binding_errors");
assert.equal(rejected.errors[0].name, "isp");

const injection = prepared.evaluateBindings([{ name: "isp", expr: "@g0 * 40.0 s^2/m" }]);
assert.equal(injection.status, "binding_errors", "readers bind closed values only");

// The controls must emit real-literal syntax: `1200 kg` is not a quantity
// literal, `1200.0 kg` is. The runtime derives its initial field text and
// slider expressions with a forced decimal point for exactly this reason.
const integerStyle = prepared.evaluateBindings([{ name: "dry_mass", expr: "1200 kg" }]);
assert.equal(integerStyle.status, "binding_errors", "integer-style quantity text must be rejected");
const decimalStyle = prepared.evaluateBindings([{ name: "dry_mass", expr: "1200.0 kg" }]);
assert.equal(decimalStyle.status, "evaluated", "decimal-style quantity text must bind");

console.log("report hydration smoke: ok");

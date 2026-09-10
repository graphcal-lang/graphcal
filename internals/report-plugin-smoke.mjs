// Execute the SDK-plugin report produced by plugin_e2e.rs using its own engine.
// No model sources or raw evaluation logs are printed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInThisContext } from "node:vm";

const html = readFileSync(process.argv[2], "utf8");
function payload(id) {
  const match = html.match(new RegExp(`<script id="${id}"[^>]*>([\\s\\S]*?)</script>`));
  assert.ok(match, `missing ${id}`);
  return match[1];
}
const project = JSON.parse(payload("graphcal-project"));
runInThisContext(Buffer.from(payload("graphcal-engine-glue"), "base64").toString("utf8"));
const started = performance.now();
await wasm_bindgen({ module_or_path: Buffer.from(payload("graphcal-engine-wasm"), "base64") });
const engineMs = performance.now() - started;
const preparing = performance.now();
const prepared = wasm_bindgen.prepareReportBundle(JSON.stringify(project));
const prepareMs = performance.now() - preparing;
function evaluate(bindings) {
  const outcome = prepared.evaluateBindings(bindings);
  assert.equal(outcome.status, "evaluated");
  return new Map(outcome.evaluation.values.map(value => [value.name, value.outcome]));
}
const evaluating = performance.now();
for (const [bindings, expectedMid] of [[[], 2], [[{ name: "a", expr: "5.0 m" }], 4], [[], 2]]) {
  const values = evaluate(bindings);
  assert.equal(values.get("mid").value.si_value, expectedMid);
  assert.equal(values.get("fine").value.si_value, 3);
  assert.equal(values.get("ascent_share").value.si_value, 0.75);
  assert.equal(values.get("bad").status, "error");
}
const evaluateMs = (performance.now() - evaluating) / 3;
prepared.free();
const plugin = project.files.find(file => file.kind === "plugin");
assert.ok(plugin);
const tampered = structuredClone(project);
tampered.files.find(file => file.kind === "plugin").content = Buffer.from([0, 97, 115, 109]).toString("base64");
assert.throws(() => wasm_bindgen.prepareReportBundle(JSON.stringify(tampered)), "lockfile digest must be checked");
const missing = structuredClone(project);
missing.files = missing.files.filter(file => file.kind !== "plugin");
assert.throws(() => wasm_bindgen.prepareReportBundle(JSON.stringify(missing)), "missing binary must fail");
const exhausted = structuredClone(project);
exhausted.files.find(file => file.kind === "manifest").content += "\n[plugins]\nfuel_per_call = 1\n";
const bounded = wasm_bindgen.prepareReportBundle(JSON.stringify(exhausted));
const failed = bounded.evaluateBindings([]);
assert.equal(failed.status, "evaluated");
assert.equal(failed.evaluation.values.find(value => value.name === "mid").outcome.status, "error");
bounded.free();
assert.throws(() => wasm_bindgen.prepareReportBundle({}), "non-string transport must be rejected");
console.log(`SDK plugin report: offline replay, edits/reset, arrays, failure containment, pins and fuel passed (engine ${engineMs.toFixed(1)} ms, prepare ${prepareMs.toFixed(1)} ms, evaluation ${evaluateMs.toFixed(1)} ms)`);

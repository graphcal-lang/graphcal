// Pure outline projection tests: no browser, source model, or evaluator needed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

const context = { window: {} };
runInNewContext(readFileSync("crates/graphcal-report/src/report_outline_state.js", "utf8"), context);
const { matches, isPathPrefix, tree, descendants } = context.window.GraphcalReportOutlineState;
assert.equal(matches(["sensor", "reading", "value"], "Calibration setting", "READING › calibration"), true);
assert.equal(matches(["sensor", "value"], "", "sensor absent"), false);
assert.equal(matches(["samples", "#39"], "", "samples #39"), true);
assert.equal(matches(["a"], "", "  "), true);
const path = [{ kind: "field", index: 0 }, { kind: "entry", index: 2 }];
assert.equal(isPathPrefix([], path), true);
assert.equal(isPathPrefix(path.slice(0, 1), path), true);
assert.equal(isPathPrefix(path, path.slice(0, 1)), false);
assert.equal(isPathPrefix([{ kind: "entry", index: 0 }], path), false);
assert.equal(isPathPrefix([{ kind: "field", index: 1 }], path), false);
const fields = [
  { labels: ["scalar"] },
  { labels: ["record", "left"] },
  { labels: ["record", "nested", "right"] },
];
const before = JSON.stringify(fields);
const projected = tree(fields);
assert.equal(projected.rows[0], fields[0]);
assert.equal(projected.groups.get("record").rows[0], fields[1]);
assert.equal(projected.groups.get("record").groups.get("nested").rows[0], fields[2]);
assert.equal(descendants(projected).length, 3);
assert.equal(JSON.stringify(fields), before, "projection does not mutate registrations");
console.log("outline state: contextual token search, typed diagnostic prefixes, immutable tree projection passed");

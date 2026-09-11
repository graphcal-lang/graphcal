// Synthetic-only acceptance coverage for the shared adaptive workspace.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

export async function testReportOutline({ temporary, openReport }) {
  const source = join(temporary, "outline.gcl");
  const output = join(temporary, "outline.html");
  writeFileSync(source, `// Entirely synthetic UI stress fixture.
pub type Reading { Reading(value: Length), }
pub type Pair { Pair(left: Length, right: Length), }
pub type Bundle { Bundle(first: Pair, second: Pair, last: Reading), }
pub type Choice { Amount(value: Length), Other(value: Length), Off, }
pub type Marker { Marker, }
pub type Toggle { Disabled, Enabled(value: Bool), }
param marker: Marker = Marker;
param toggle: Toggle = Disabled;
${Array.from({ length: 48 }, (_, i) => `param input_${i}: Length = 1.0 m;`).join("\n")}
/// Calibration annotation <untrusted> is plain text, not markup.
param reading: Reading = Reading(value: 2.0 m);
param derived: Reading = Reading(value: @input_0);
param pair: Pair = Pair(left: 2.0 m, right: 3.0 m);
param bundle: Bundle = Bundle(first: Pair(left: 1.0 m, right: 2.0 m), second: Pair(left: 3.0 m, right: 4.0 m), last: Reading(value: 5.0 m));
param choice: Choice = Amount(value: 2.0 m);
param samples: Int[Fin(40)] = for i: Fin(40) { 1 };
node doubled: Length = @input_0 * 2.0;
node selected: Choice = @choice;
node reciprocal: Dimensionless[Fin(2)] = for i: Fin(2) { 1.0 / (@input_0 / (1.0 m)) };
node last: Int = @samples[39];
assert positive = @input_0 > 0.0 m;
`);
  const built = spawnSync("target/debug/graphcal", ["report", "build", source, "--output", output], { encoding: "utf8", timeout: 30000 });
  assert.equal(built.status, 0, built.stderr);
  const { command, evaluate, wait, exceptions, close } = await openReport(output);
  const field = label => `.outline-value[aria-label=${JSON.stringify(label)}]`;
  const query = selector => `document.querySelector(${JSON.stringify(selector)})`;
  const fill = async (selector, value, event = "input") => evaluate(`(() => { const node = ${query(selector)}; node.focus(); node.value = ${JSON.stringify(value)}; node.dispatchEvent(new Event(${JSON.stringify(event)}, { bubbles: true })); })()`);
  const click = async selector => evaluate(`${query(selector)}.click()`);
  const result = name => `${query(`[data-decl="${name}"] [data-role="value"]`)}.textContent.trim()`;
  const toolbar = text => `Array.from(document.querySelectorAll('.outline-options button')).find(button => button.textContent === ${JSON.stringify(text)})`;
  await wait(`document.querySelector('.hydration-status')?.textContent === 'live'`);
  assert.equal(await evaluate("document.querySelectorAll('.outline-content > .outline-row').length"), 50, "48 scalars plus two one-field records are direct rows");
  assert.equal(await evaluate(`${query(field("reading value"))}.closest('.outline-group') === null`), true);
  assert.equal(await evaluate(`${query(field("pair left"))}.closest('.outline-group').open`), true, "small records start expanded");
  assert.equal(await evaluate(`${query(field("bundle first left"))}.closest('.outline-content > .outline-group').open`), false, "large records start collapsed");
  assert.equal(await evaluate(`document.querySelectorAll('[data-decl="samples"] .control-index-entry').length`), 32, "initial indexed rendering remains paginated");

  assert.equal(await evaluate(`${query(field("marker constructor"))}.value`), "0", "single nullary parameters remain visible");
  await fill(field("toggle constructor"), "1", "change");
  assert.equal(await evaluate(`${query(field("toggle value"))}.indeterminate`), true, "missing booleans are explicit, not a fabricated false");
  await evaluate(`${query(field("toggle value"))}.checked = false; ${query(field("toggle value"))}.dispatchEvent(new Event('change', { bubbles: true }))`);
  await wait(`${result("toggle")}.includes('false')`);

  await fill(field("input_0"), "3.0 m");
  await wait(`${result("doubled")} === '6 m'`);
  assert.equal(await evaluate(`document.activeElement === ${query(field("input_0"))}`), true, "successful evaluation retains field focus");
  await fill(field("input_0"), "3.0 s");
  await wait(`${query(field("input_0"))}.getAttribute('aria-invalid') === 'true'`);
  assert.equal(await evaluate(result("doubled")), "6 m", "invalid units retain results");
  await fill(field("input_0"), "4.0 m");
  await wait(`${result("doubled")} === '8 m'`);
  assert.equal(await evaluate(`${query(field("input_0"))}.getAttribute('aria-invalid')`), "false");

  await fill(".outline-search", "calibration annotation");
  assert.equal(await evaluate("document.querySelectorAll('.outline-content .outline-row').length"), 1, "descriptions participate in search");
  assert.equal(await evaluate("document.querySelectorAll('untrusted').length"), 0, "description text is not HTML");
  await fill(".outline-search", "bundle › second › right");
  assert.equal(await evaluate(`${query(field("bundle second right"))}.checkVisibility()`), true, "search expands ancestors and accepts pasted display paths");
  await click('[aria-label="Pin input bundle › second › right"]');
  assert.equal(await evaluate(`${query(field("bundle second right"))}.closest('.outline-favorites') !== null`), true);
  await fill(field("bundle second right"), "9.0 s");
  await wait(`${query(field("bundle second right"))}.getAttribute('aria-invalid') === 'true'`);
  assert.equal(await evaluate(`${query(field("bundle second right"))}.getAttribute('aria-describedby') === ${query(field("bundle second right"))}.closest('.outline-row').querySelector('.outline-error').id`), true);
  await fill(field("bundle second right"), "9.0 m");
  await wait(`${result("bundle")}.includes('9 m')`);
  await fill(".outline-search", "samples #39");
  assert.equal(await evaluate(`${query(field("samples #39"))}.checkVisibility()`), true, "search can discover entries beyond the first page");
  await fill(field("samples #39"), "7");
  await wait(`${result("last")} === '7'`);

  await fill(".outline-search", "choice");
  await click('[aria-label="Pin input choice › value"]');
  await fill(field("choice constructor"), "1", "change");
  assert.equal(await evaluate(`${query(field("choice value"))}.value`), "", "new constructor leaves are not invented");
  assert.equal(await evaluate(`${query(field("choice value"))}.closest('.outline-favorites') === null`), true, "pins do not transfer to another constructor's same-named field");
  await fill(field("choice value"), "5.0 m");
  await wait(`${result("selected")}.includes('5 m')`);
  await fill(field("choice constructor"), "0", "change");
  assert.equal(await evaluate(`${query(field("choice value"))}.value`), "2.0 m", "constructor drafts survive");
  assert.equal(await evaluate(`${query(field("choice value"))}.closest('.outline-favorites') !== null`), true, "returning constructor restores its pin");
  await wait(`${result("selected")}.includes('2 m')`);

  const acceptedChoice = await evaluate(result("selected"));
  await click(".auto-run-toggle input");
  await fill(".outline-search", "derived");
  await fill(field("derived value"), "99.0 m");
  await fill(".outline-search", "input_0");
  await fill(field("input_0"), "5.0 m");
  await evaluate(`document.querySelector('[data-decl="input_0"] .control-apply').click()`);
  await wait(`${result("derived")}.includes('5 m')`);
  await fill(".outline-search", "derived");
  assert.equal(await evaluate(`${query(field("derived value"))}.value`), "99.0 m", "reactive defaults do not overwrite open drafts");
  assert.equal(await evaluate("document.querySelector('.outline-notice').textContent.includes('reactive default changed')"), true);
  await click(`${field("derived value")} ~ .outline-row-actions > summary`);
  await evaluate(`Array.from(${query(field("derived value"))}.closest('.outline-row').querySelectorAll('button')).find(button => button.textContent === 'Discard edits').click()`);
  assert.equal(await evaluate(`${query(field("derived value"))}.value`), "5.0 m", "discard reloads the latest reactive default in the outline");
  await fill(".outline-search", "choice");
  await fill(field("choice value"), "6.0 m");
  await evaluate("new Promise(resolve => setTimeout(resolve, 800))");
  assert.equal(await evaluate(result("selected")), acceptedChoice, "manual drafts do not evaluate");
  await click(`${field("choice value")} ~ .outline-row-actions > summary`);
  await evaluate(`Array.from(${query(field("choice value"))}.closest('.outline-row').querySelectorAll('button')).find(button => button.textContent === 'Apply').click()`);
  await wait(`${result("selected")}.includes('6 m')`);

  await evaluate(`${toolbar("Advanced controls")}.click()`);
  await evaluate(`Array.from(document.querySelectorAll('[data-decl="choice"] .control-mode')).find(button => button.textContent === 'Raw literal').click()`);
  await fill('[data-decl="choice"] .control-raw-field', "Amount(value: 7.0 m)");
  await click('[data-decl="choice"] .control-apply');
  await wait(`${result("selected")}.includes('7 m')`);
  await evaluate(`${toolbar("Back to outline")}.click()`);
  assert.equal(await evaluate(`${query(field("choice value"))}.value`), "7.0 m", "raw changes reproject into the outline");
  await click('.workspace-output-pin[aria-label="Pin output doubled"]');
  await evaluate(`Array.from(document.querySelectorAll('.workspace-result-tabs button')).find(button => button.textContent === 'Checks').click()`);
  assert.equal(await evaluate(`${query('[data-decl="doubled"]')}.checkVisibility()`), true, "pinned output remains visible across result tabs");
  await fill('[aria-label="Filter output names"]', 'no-match');
  assert.equal(await evaluate(`${query('[data-decl="doubled"]')}.checkVisibility()`), true, "filter does not remove pinned outputs");
  await evaluate(`${query(field("choice value"))}.focus()`);
  await click(".modified-banner__reset");
  await wait(`${result("doubled")} === '2 m'`);
  await wait(`${result("selected")}.includes('2 m')`);
  assert.equal(await evaluate(`${query(field("choice value"))}.value`), "2.0 m", "reset reprojects even a focused pinned field");
  await fill(".outline-search", "");
  await click(".outline-options input[type=checkbox]");
  assert.equal(await evaluate("document.querySelector('.outline-content').hidden"), true);
  assert.equal(await evaluate("document.querySelectorAll('.outline-favorites .outline-row').length"), 2, "reset keeps session UI pins");
  await click(".outline-options input[type=checkbox]");
  await click(".auto-run-toggle input");
  await fill(field("input_0"), "0.0 m");
  await wait(`${result("reciprocal")}.includes('division by zero')`);
  assert.equal(await evaluate("document.querySelector('[data-decl=reciprocal] .workspace-output-details').open"), true, "collapsed outputs reveal evaluation failures");
  await fill(field("input_0"), "1.0 m");
  await wait(`${result("doubled")} === '2 m'`);

  for (const width of [1440, 390, 280]) {
    await command("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile: false });
    assert.equal(await evaluate("document.documentElement.scrollWidth <= innerWidth"), true, `${width}px: no horizontal page overflow`);
    assert.equal(await evaluate("document.documentElement.scrollHeight <= innerHeight + 1"), true, `${width}px: independent panes fit the viewport`);
    assert.equal(await evaluate("document.querySelector('.outline-explorer').scrollHeight > document.querySelector('.outline-explorer').clientHeight"), true);
    await evaluate("document.querySelector('.outline-explorer').scrollTop = 10000");
    assert.equal(await evaluate(`${query('[data-decl="doubled"]')}.checkVisibility()`), true, "browsing inputs does not scroll pinned results away");
  }
  await command("Emulation.setEmulatedMedia", { media: "print" });
  assert.equal(await evaluate("getComputedStyle(document.querySelector('.outline-advanced')).display"), "block", "print exposes the original accepted parameter values");
  assert.equal(await evaluate("getComputedStyle(document.getElementById('values')).display"), "block", "print exposes inactive result tabs");
  assert.equal(await evaluate("document.querySelector('[data-decl=selected] .value-scroll').clientHeight > 0"), true, "print exposes closed structured outputs");
  await command("Emulation.setEmulatedMedia", { media: "" });
  assert.deepEqual(exceptions, [], "workspace must not throw browser exceptions");
  await close();
  const fallback = await openReport(output, `window.Worker = function () { throw new Error('synthetic startup failure'); };`);
  await fallback.wait("document.querySelector('.hydration-status')?.textContent === 'interactive mode unavailable'");
  assert.equal(await fallback.evaluate("document.querySelector('.report-workspace') === null"), true, "failed hydration leaves the static document intact");
  assert.equal(await fallback.evaluate("document.querySelector('[data-decl=reading] [data-role=value]').checkVisibility()"), true);
  assert.deepEqual(fallback.exceptions, []);
  await fallback.close();
  console.log("Chrome: synthetic adaptive outline, many scalars, one-field records, deep search, pins, constructors, pagination, manual/raw, reset, responsive panes and print passed");
}

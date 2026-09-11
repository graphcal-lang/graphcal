// Synthetic direct/transitive, multi-version package fixture; original checkout
// and cache were deleted by the native test before this process was started.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInThisContext } from "node:vm";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const html = readFileSync(process.argv[2], "utf8");
function payload(id) {
  const match = html.match(new RegExp(`<script id="${id}"[^>]*>([\\s\\S]*?)</script>`));
  assert.ok(match, `missing ${id}`);
  return match[1];
}
const bundle = JSON.parse(payload("graphcal-project"));
assert.equal(bundle.dependencies.length, 3);
const provenance = html.match(/<ul class="sources">([\s\S]*?)<\/ul>/)?.[1];
assert.ok(provenance);
assert.ok(provenance.includes(`<code>${bundle.entry}</code>`));
for (const package_ of bundle.dependencies) {
  for (const file of package_.files.filter(file => file.kind === "source")) {
    assert.ok(provenance.includes(`<code>${package_.id} / ${file.path}</code>`), "dependency provenance must retain package identity and relative path");
  }
}
const plugins = bundle.dependencies.filter(package_ => package_.files.some(file => file.kind === "plugin"));
assert.equal(plugins.length, 2);
assert.notEqual(plugins[0].id, plugins[1].id);
assert.equal(plugins[0].files.find(file => file.kind === "plugin").path, plugins[1].files.find(file => file.kind === "plugin").path);
assert.ok(plugins.every(package_ => package_.files.some(file => file.kind === "auxiliary")));
runInThisContext(Buffer.from(payload("graphcal-engine-glue"), "base64").toString("utf8"));
await wasm_bindgen({ module_or_path: Buffer.from(payload("graphcal-engine-wasm"), "base64") });
const prepared = wasm_bindgen.prepareReportBundle(JSON.stringify(bundle));
for (const [bindings, factor] of [[[], 4], [[{ name: "input", expr: "5.0" }], 5], [[], 4]]) {
  const outcome = prepared.evaluateBindings(bindings);
  assert.equal(outcome.status, "evaluated");
  assert.equal(outcome.evaluation.has_errors, false);
  const values = new Map(outcome.evaluation.values.map(value => [value.name, value.outcome]));
  assert.equal(values.get("first").value.si_value, 2 * factor);
  assert.equal(values.get("later_result").value.si_value, 3 * factor);
}
prepared.free();
function rejects(change) {
  const changed = structuredClone(bundle);
  change(changed);
  assert.throws(() => wasm_bindgen.prepareReportBundle(JSON.stringify(changed)));
}
function pluginPackage(changed) {
  return changed.dependencies.find(package_ => package_.files.some(file => file.kind === "plugin"));
}
rejects(changed => changed.dependencies.pop());
rejects(changed => changed.dependencies.push(structuredClone(changed.dependencies[0])));
rejects(changed => { changed.dependencies[0].id = "unknown-package"; });
rejects(changed => { pluginPackage(changed).files.find(file => file.kind === "plugin").content = "AGFzbQ=="; });
rejects(changed => { const package_ = pluginPackage(changed); package_.files = package_.files.filter(file => file.kind !== "plugin"); });
rejects(changed => { pluginPackage(changed).files.find(file => file.kind === "auxiliary").content = "AA=="; });
rejects(changed => { pluginPackage(changed).files.find(file => file.kind === "manifest").content += "\n# altered\n"; });
rejects(changed => { pluginPackage(changed).files.push({ path: "unverified.bin", kind: "auxiliary", content: "AA==" }); });
rejects(changed => { pluginPackage(changed).files[0].path = "../escape"; });
rejects(changed => { changed.files = changed.files.filter(file => file.kind !== "lockfile"); });
rejects(changed => { changed.files.find(file => file.kind === "manifest").content = changed.files.find(file => file.kind === "manifest").content.replace("package = 'bridge'", "package = 'unknown'"); });

if (process.env.GRAPHCAL_PACKAGE_BROWSER_TEST === "1") {
  const require = createRequire(new URL("../web/playground/package.json", import.meta.url));
  const { chromium, firefox, webkit, expect } = require("@playwright/test");
  for (const browserType of [chromium, firefox, webkit]) {
    const browser = await browserType.launch();
    try {
      const page = await browser.newPage();
      await page.route(/^https?:/, route => route.abort());
      await page.goto(pathToFileURL(process.argv[2]).href);
      const status = page.locator(".hydration-status");
      await expect(status).toHaveText("live");
      const first = page.locator('[data-decl="first"] [data-role="value"]');
      const later = page.locator('[data-decl="later_result"] [data-role="value"]');
      await expect(first).toHaveText("8");
      await expect(later).toHaveText("12");
      const input = page.locator('[data-decl="input"] .control-field');
      await input.fill("5.0");
      await page.locator('[data-decl="input"] .control-apply').click();
      await expect(first).toHaveText("10");
      await expect(later).toHaveText("15");
      await page.locator(".modified-banner__reset").click();
      await expect(first).toHaveText("8");
      await page.evaluate(() => {
        globalThis.__timeoutObserved = false;
        // Keep cancellation deterministic on engines that exhaust two billion
        // fuel units before ten seconds. Only this test shortens the deadline.
        globalThis.__originalSetTimeout = window.setTimeout;
        window.setTimeout = (callback, delay, ...args) => globalThis.__originalSetTimeout(callback, delay === 10000 ? 100 : delay, ...args);
        const status = document.querySelector(".hydration-status");
        // Restart can replace the timeout status in the same task. Inspect the
        // recorded text nodes, not only the final DOM at observer delivery.
        new MutationObserver(records => {
          if (records.some(record => Array.from(record.addedNodes).some(node => node.textContent.includes("timed out")))) globalThis.__timeoutObserved = true;
        }).observe(status, { childList: true, characterData: true, subtree: true });
      });
      // Negative input enters a metered loop in a dependency-owned plugin.
      // The worker deadline must tear down that interpreter, then recover.
      await input.fill("-1.0");
      await page.locator('[data-decl="input"] .control-apply').click();
      try {
        await expect.poll(() => page.evaluate(() => globalThis.__timeoutObserved), { timeout: 30000 }).toBe(true);
      } catch (error) {
        throw new Error(`blocked plugin did not time out: status=${await status.textContent()}, first=${await first.textContent()}`, { cause: error });
      }
      await page.evaluate(() => { window.setTimeout = globalThis.__originalSetTimeout; });
      await input.fill("5.0");
      await page.locator('[data-decl="input"] .control-apply').click();
      await expect(status).toHaveText("live", { timeout: 20000 });
      await expect(first).toHaveText("10");
      await expect(later).toHaveText("15");
    } finally {
      await browser.close();
    }
  }
  console.log("Package reports: offline Chromium/Firefox/WebKit edits/reset and blocked-plugin worker replacement passed");
}
console.log("Package reports: transitive multi-version parity, deterministic replay, complete hash coverage and malformed closure rejection passed");

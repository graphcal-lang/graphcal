import { readFile, readdir, stat } from "node:fs/promises";
import assert from "node:assert/strict";
const site = new URL("../site/", import.meta.url);
const text = (path) => readFile(new URL(path, site), "utf8");
assert.match(await text("index.html"), /url=\/docs\//);
assert.equal((await text("CNAME")).trim(), "graphcal.org");
for (const file of ["404.html", "docs/index.html", "playground/index.html", "playground/pkg/graphcal_wasm.js", "playground/pkg/graphcal_wasm_bg.wasm", "playground/vega/vega.min.js", "playground/vega/vega-lite.min.js", "playground/vega/vega-embed.min.js"]) {
  assert.ok((await stat(new URL(file, site))).size > 0, file);
}
assert.ok((await stat(new URL("playground/pkg/graphcal_wasm_bg.wasm", site))).size <= 5 * 1024 * 1024, "5 MiB Wasm budget");
const assets = await readdir(new URL("playground/assets/", site));
assert.ok(assets.some((name) => /^evaluation-worker-.*\.js$/.test(name)), "bundled worker");
const entryScripts = assets.filter((name) => /^index-.*\.js$/.test(name));
assert.ok(entryScripts.length > 0);
const entryBytes = (await Promise.all(entryScripts.map(async (name) => (await stat(new URL(`playground/assets/${name}`, site))).size))).reduce((sum, size) => sum + size, 0);
assert.ok(entryBytes <= 600 * 1024, `600 KiB raw entry JS budget: ${entryBytes}`);
assert.doesNotMatch(await text("docs/index.html"), /javascripts\/playground|stylesheets\/playground/);
const catalog = JSON.parse(await readFile(new URL("../web/playground/examples/catalog.json", import.meta.url), "utf8"));
for (const example of catalog) {
  assert.equal(await text(`playground/examples/${example.id}.gcl`), await readFile(new URL(`../${example.source_path}`, import.meta.url), "utf8"));
}
console.log(`Published site verified: ${catalog.length} examples, ${entryBytes} bytes entry JavaScript`);

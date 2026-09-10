import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { test } from "node:test";
import { assembleSite } from "./site-assemble.mjs";

async function fixture(t) {
  const path = await mkdtemp(join(tmpdir(), "graphcal-site-"));
  t.after(() => rm(path, { recursive: true, force: true }));
  const root = pathToFileURL(path + "/");
  for (const dir of [
    "target/docs-site/en/",
    "target/docs-site/ja/",
    "web/playground/dist/",
    "site-root/",
  ]) {
    await mkdir(new URL(dir, root), { recursive: true });
    await writeFile(new URL(dir + "index.html", root), dir);
  }
  await writeFile(new URL("target/docs-site/en/404.html", root), "not found");
  await writeFile(new URL("site-root/CNAME", root), "graphcal.org\n");
  return root;
}

test("assembly retains both locales, shared assets, playground, and root files", async (t) => {
  const root = await fixture(t);
  await assembleSite(root);
  assert.equal(
    await readFile(new URL("site/docs/index.html", root), "utf8"),
    "target/docs-site/en/",
  );
  assert.equal(
    await readFile(new URL("site/docs/ja/index.html", root), "utf8"),
    "target/docs-site/ja/",
  );
  assert.equal(
    await readFile(new URL("site/playground/index.html", root), "utf8"),
    "web/playground/dist/",
  );
  assert.equal(await readFile(new URL("site/CNAME", root), "utf8"), "graphcal.org\n");
  assert.equal(await readFile(new URL("site/404.html", root), "utf8"), "not found");
  assert.match(
    await readFile(new URL("site/sitemap.xml", root), "utf8"),
    /\/docs\/ja\/sitemap.xml/,
  );
  await writeFile(new URL("site/docs/deleted.html", root), "stale");
  await writeFile(new URL("site/docs/ja/deleted.html", root), "stale");
  await assembleSite(root);
  await assert.rejects(stat(new URL("site/docs/deleted.html", root)), { code: "ENOENT" });
  await assert.rejects(stat(new URL("site/docs/ja/deleted.html", root)), { code: "ENOENT" });
  assert.ok((await stat(new URL("site/playground/index.html", root))).size > 0);
});

test("an overlapping English output is rejected before removing the current site", async (t) => {
  const root = await fixture(t);
  await assembleSite(root);
  await mkdir(new URL("target/docs-site/en/ja/", root));
  await assert.rejects(assembleSite(root), /reserved ja/);
  assert.ok((await stat(new URL("site/docs/ja/index.html", root))).size > 0);
});

test("a missing build input is rejected before touching the current artifact", async (t) => {
  const root = await fixture(t);
  await assembleSite(root);
  await rm(new URL("target/docs-site/ja/index.html", root));
  await assert.rejects(assembleSite(root), { code: "ENOENT" });
  assert.ok((await stat(new URL("site/docs/index.html", root))).size > 0);
});

import assert from "node:assert/strict";
import { cp, rm, stat } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

// Each builder owns an isolated directory. Only this shell owns the final site.
export async function assembleSite(root) {
  for (const path of [
    "target/docs-site/en/index.html",
    "target/docs-site/en/404.html",
    "target/docs-site/ja/index.html",
    "web/playground/dist/index.html",
    "site-root/index.html",
    "site-root/CNAME",
  ]) {
    assert.ok((await stat(new URL(path, root))).size > 0, `Missing build input: ${path}`);
  }
  await rm(new URL("site/", root), { recursive: true, force: true });
  await cp(new URL("target/docs-site/en/", root), new URL("site/docs/en/", root), {
    recursive: true,
  });
  await cp(new URL("target/docs-site/ja/", root), new URL("site/docs/ja/", root), {
    recursive: true,
  });
  await cp(new URL("site-root/", root), new URL("site/", root), { recursive: true });
  await cp(new URL("target/docs-site/en/404.html", root), new URL("site/404.html", root));
  await cp(new URL("web/playground/dist/", root), new URL("site/playground/", root), {
    recursive: true,
  });
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  await assembleSite(new URL("../", import.meta.url));
}

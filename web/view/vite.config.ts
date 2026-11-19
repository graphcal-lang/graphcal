import { readFileSync } from "node:fs";
import { defineConfig } from "vite-plus";

/**
 * Where the page is served, chosen by `VIEW_BASE`: `/view/` on graphcal.org
 * (the documentation site, the default) or `/` on https://view.graphcal.org/
 * (the release workflow's `build-view`). Nothing else is a published address.
 */
const BASES = ["/view/", "/"];
const base = process.env.VIEW_BASE ?? "/view/";
if (!BASES.includes(base)) {
  throw new Error(`VIEW_BASE must be one of ${BASES.join(", ")}, not ${JSON.stringify(base)}`);
}

/**
 * The page's CSP, as public/_headers sends it on view.graphcal.org, for a
 * meta tag: graphcal.org (GitHub Pages) sends no headers. A meta CSP cannot
 * carry `frame-ancestors` (browsers ignore it there), so it is left out; only
 * the header can forbid framing.
 */
function metaCsp(): string {
  const headers = readFileSync("public/_headers", "utf8").split(/\r?\n/);
  const line = headers.find((l) => /^\s+Content-Security-Policy:/i.test(l));
  if (line === undefined) throw new Error("public/_headers has no Content-Security-Policy");
  return line
    .slice(line.indexOf(":") + 1)
    .split(";")
    .map((directive) => directive.trim())
    .filter((directive) => directive !== "" && !directive.startsWith("frame-ancestors"))
    .join("; ");
}

export default defineConfig({
  base,
  plugins: [
    {
      // The page's markup lives in src/shell.html so the VS Code webview can share it.
      name: "graphcal-view-shell",
      transformIndexHtml: (html) =>
        html.replace("<!-- shell -->", () => readFileSync("src/shell.html", "utf8")),
    },
    {
      // Dev builds skip it so the dev client and its overlay keep working.
      name: "graphcal-view-csp",
      apply: "build",
      transformIndexHtml: () => [
        {
          tag: "meta",
          attrs: { "http-equiv": "Content-Security-Policy", content: metaCsp() },
          injectTo: "head-prepend",
        },
      ],
    },
  ],
  test: { include: ["src/**/*.test.ts"] },
  build: { target: "es2023" },
  // The contract fixtures are `graphcal view` output, compared byte for byte.
  fmt: { ignorePatterns: ["src/contract/**"] },
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
});

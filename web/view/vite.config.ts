import { readFileSync } from "node:fs";
import { defineConfig } from "vite-plus";

export default defineConfig({
  base: "/view/",
  plugins: [
    {
      // The page's markup lives in src/shell.html so the VS Code webview can share it.
      name: "graphcal-view-shell",
      transformIndexHtml: (html) =>
        html.replace("<!-- shell -->", () => readFileSync("src/shell.html", "utf8")),
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

import { defineConfig } from "vite-plus";

/**
 * The published page talks to its own origin and api.github.com only, runs
 * its own scripts (and the WASM engine), and loads no third-party resource.
 * Dev builds skip this so the dev client and its overlay keep working.
 */
const CSP = [
  "default-src 'self'",
  "script-src 'self' 'wasm-unsafe-eval'",
  "style-src 'self' 'unsafe-inline'",
  "connect-src 'self' https://api.github.com",
  "img-src 'self' data:",
  "object-src 'none'",
  "base-uri 'none'",
  "frame-ancestors 'none'",
].join("; ");

export default defineConfig({
  base: "/view/",
  plugins: [
    {
      name: "graphcal-view-csp",
      apply: "build",
      transformIndexHtml: () => [
        {
          tag: "meta",
          attrs: { "http-equiv": "Content-Security-Policy", content: CSP },
          injectTo: "head-prepend",
        },
      ],
    },
  ],
  test: { include: ["src/**/*.test.ts"] },
  build: { target: "es2023" },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
});

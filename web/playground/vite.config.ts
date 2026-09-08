import { defineConfig } from "vite-plus";

export default defineConfig({
  base: "/playground/",
  test: { include: ["src/**/*.test.ts"] },
  worker: { format: "es" },
  build: { target: "es2023", license: { fileName: "licenses.md" } },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
});

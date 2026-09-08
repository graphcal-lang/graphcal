import { defineConfig } from "vite-plus";

export default defineConfig({
  base: "/playground/",
  worker: { format: "es" },
  build: { target: "es2023", license: { fileName: "licenses.md" } },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
});

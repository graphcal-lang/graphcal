import { defineConfig } from "vite-plus";

export default defineConfig({
  base: "/view/",
  test: { include: ["src/**/*.test.ts"] },
  build: { target: "es2023" },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    options: { typeAware: true, typeCheck: true },
  },
});

// Builds the view as one script and one stylesheet for the VS Code webview.
// `VIEW_OUT` chooses the output directory (default dist-webview).
import { defineConfig } from "vite-plus";

export default defineConfig({
  base: "./",
  // The engine bundle under public/ is the dev shell's; the webview has none.
  publicDir: false,
  build: {
    target: "es2023",
    outDir: process.env.VIEW_OUT ?? "dist-webview",
    emptyOutDir: true,
    rollupOptions: {
      input: "src/webview.ts",
      output: {
        // One script: the shell goes in, then the view (a lazily run module, not a second file).
        inlineDynamicImports: true,
        entryFileNames: "view.js",
        assetFileNames: "view.[ext]",
        chunkFileNames: "[name].js",
      },
    },
  },
});

import init, { evaluateProject } from "../assets/playground/pkg/graphcal_wasm.js";

const WASM_URL = new URL("../assets/playground/pkg/graphcal_wasm_bg.wasm", import.meta.url);
let initialization;

function initialize() {
  initialization ??= fetch(WASM_URL)
    .then((response) => {
      if (!response.ok) throw new Error(`WebAssembly download returned HTTP ${response.status}`);
      return response.arrayBuffer();
    })
    .then((module_or_path) => init({ module_or_path }));
  return initialization;
}

self.addEventListener("message", async (event) => {
  const { id, request } = event.data;
  try {
    await initialize();
    const result = evaluateProject(request);
    self.postMessage({ id, result });
  } catch (error) {
    self.postMessage({
      id,
      workerError: error instanceof Error ? error.message : String(error),
    });
  }
});

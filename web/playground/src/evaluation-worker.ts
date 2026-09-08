import { evaluationRequest, validateDocument } from "./document";
import { outcomeSchema, type WorkerReply } from "./protocol";

function reply(message: WorkerReply) {
  self.postMessage(message);
}
function failure(error: unknown) {
  reply({
    kind: "error",
    message: error instanceof Error ? error.message : "Unknown worker failure",
  });
}

// Fixed, same-origin deployment assets; never derived from snippet contents.
const moduleUrl = new URL(`${import.meta.env.BASE_URL}pkg/graphcal_wasm.js`, self.location.origin)
  .href;
const ready = (async () => {
  const engine = (await import(/* @vite-ignore */ moduleUrl)) as {
    default: (options: { module_or_path: ArrayBuffer }) => Promise<unknown>;
    evaluateProject: (request: ReturnType<typeof evaluationRequest>) => unknown;
  };
  const response = await fetch(
    new URL(`${import.meta.env.BASE_URL}pkg/graphcal_wasm_bg.wasm`, self.location.origin),
    { signal: AbortSignal.timeout(30_000) },
  );
  if (!response.ok) throw new Error(`WebAssembly download returned HTTP ${response.status}`);
  await engine.default({ module_or_path: await response.arrayBuffer() });
  return engine;
})();
void ready.then(() => reply({ kind: "ready" }), failure);
self.addEventListener("message", (event: MessageEvent<unknown>) => {
  void (async () => {
    const data = event.data;
    if (
      typeof data !== "object" ||
      data === null ||
      !("id" in data) ||
      !Number.isSafeInteger(data.id) ||
      !("document" in data)
    )
      throw new Error("Invalid evaluation message");
    const request = evaluationRequest(validateDocument(data.document));
    const engine = await ready;
    reply({
      kind: "result",
      id: data.id as number,
      outcome: outcomeSchema.parse(engine.evaluateProject(request)),
    });
  })().catch(failure);
});

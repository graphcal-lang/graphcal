import { evaluationRequest, validateDocument, sameDocument, type SourceDocument } from "./document";
import { bindingsSchema, type Binding } from "./bindings";
import type { WorkerReply } from "./protocol";
import { portsSchema, outcomeSchema } from "./protocol";
import { boundedOutcome } from "./output-budget";

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
interface PreparedProject {
  evaluateReport(bindings: Binding[]): unknown;
  parameterPorts(): unknown;
  free(): void;
}
let prepared: PreparedProject | undefined;
let preparedDocument: SourceDocument | undefined;
const ready = (async () => {
  const engine = (await import(/* @vite-ignore */ moduleUrl)) as {
    default: (options: { module_or_path: ArrayBuffer }) => Promise<unknown>;
    prepareProject: (request: ReturnType<typeof evaluationRequest>) => PreparedProject;
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
    const document = validateDocument(data.document);
    const bindings = bindingsSchema.parse("bindings" in data ? data.bindings : []);
    const engine = await ready;
    if (!prepared || !preparedDocument || !sameDocument(document, preparedDocument)) {
      prepared?.free();
      prepared = undefined;
      try {
        prepared = engine.prepareProject(evaluationRequest(document));
        preparedDocument = document;
      } catch (error) {
        const failure = outcomeSchema.safeParse(error);
        if (!failure.success || failure.data.status === "evaluated") throw error;
        reply({ kind: "result", id: data.id as number, outcome: failure.data, ports: [] });
        return;
      }
    }
    reply({
      kind: "result",
      id: data.id as number,
      outcome: boundedOutcome(prepared.evaluateReport(bindings)),
      ports: portsSchema.parse(prepared.parameterPorts()),
    });
  })().catch(failure);
});

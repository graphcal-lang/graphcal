import type { SourceDocument } from "./document";
import type { Binding } from "./bindings";
import { workerReplySchema, type ReportOutcome, type ParameterPort } from "./protocol";

export type ExecutionEvent =
  | { kind: "loading" | "running" }
  | { kind: "result"; outcome: ReportOutcome; ports: ParameterPort[]; bindings: Binding[] }
  | { kind: "error"; message: string };

/** One active request, invalidated synchronously on edits/Stop/load. */
export class WorkerClient {
  private worker?: Worker;
  private ready = false;
  private sequence = 0;
  private active?: { id: number; document: SourceDocument; bindings: Binding[] };
  private timer?: ReturnType<typeof setTimeout>;
  private readonly emit: (event: ExecutionEvent) => void;
  private readonly createWorker: () => Worker;
  constructor(
    emit: (event: ExecutionEvent) => void,
    createWorker = () =>
      new Worker(new URL("./evaluation-worker.ts", import.meta.url), { type: "module" }),
  ) {
    this.emit = emit;
    this.createWorker = createWorker;
  }

  run(document: SourceDocument, bindings: Binding[] = []) {
    if (this.active) this.stop();
    this.active = { id: ++this.sequence, document, bindings };
    try {
      if (!this.worker) {
        const worker = this.createWorker();
        this.worker = worker;
        worker.addEventListener("message", (event: MessageEvent<unknown>) => {
          if (this.worker !== worker) return;
          const parsed = workerReplySchema.safeParse(event.data);
          if (!parsed.success) return this.fail("Invalid response from the browser engine.");
          const message = parsed.data;
          switch (message.kind) {
            case "ready":
              this.ready = true;
              this.send();
              break;
            case "error":
              this.fail(message.message);
              break;
            case "result":
              if (message.id !== this.active?.id) return;
              clearTimeout(this.timer);
              const bindings = this.active.bindings;
              this.active = undefined;
              this.emit({
                kind: "result",
                outcome: message.outcome,
                ports: message.ports,
                bindings,
              });
          }
        });
        worker.addEventListener("error", () => {
          if (this.worker === worker)
            this.fail("Browser engine failed to load or execute. Press Run to retry.");
        });
        worker.addEventListener("messageerror", () => {
          if (this.worker === worker) this.fail("Could not read browser engine response.");
        });
      }
      if (this.ready) this.send();
      else {
        this.emit({ kind: "loading" });
        this.timer = setTimeout(
          () => this.fail("Browser engine loading exceeded 30 seconds. Press Run to retry."),
          30_000,
        );
      }
    } catch (error) {
      this.fail(error instanceof Error ? error.message : "Workers are unavailable.");
    }
  }

  private send() {
    if (!this.active) return;
    clearTimeout(this.timer);
    this.emit({ kind: "running" });
    this.worker!.postMessage(this.active);
    this.timer = setTimeout(
      () => this.fail("Evaluation exceeded 10 seconds. Worker stopped; press Run to retry."),
      10_000,
    );
  }

  private fail(message: string) {
    this.stop();
    this.emit({ kind: "error", message });
  }
  stop() {
    clearTimeout(this.timer);
    this.active = undefined;
    this.worker?.terminate();
    this.worker = undefined;
    this.ready = false;
  }
  invalidate() {
    if (this.active) this.stop();
  }
}

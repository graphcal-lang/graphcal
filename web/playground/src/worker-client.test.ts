import { afterEach, expect, it, vi } from "vite-plus/test";
import { EditorState } from "@codemirror/state";
import { offsetAt } from "./editor";
import { WorkerClient, type ExecutionEvent } from "./worker-client";

class FakeWorker extends EventTarget {
  terminated = false;
  sent: unknown[] = [];
  terminate() {
    this.terminated = true;
  }
  postMessage(value: unknown) {
    this.sent.push(value);
  }
  reply(data: unknown) {
    this.dispatchEvent(new MessageEvent("message", { data }));
  }
}
const document = { filename: "main.gcl", source: "" };
afterEach(() => vi.useRealTimers());
function harness() {
  const workers: FakeWorker[] = [];
  const events: ExecutionEvent[] = [];
  const client = new WorkerClient(
    (event) => events.push(event),
    () => {
      const worker = new FakeWorker();
      workers.push(worker);
      return worker as unknown as Worker;
    },
  );
  return { client, workers, events };
}
it("waits for initialization and ignores replaced worker responses", () => {
  const { client, workers, events } = harness();
  client.run(document);
  expect(workers[0].sent).toEqual([]);
  workers[0].reply({ kind: "ready" });
  expect(workers[0].sent).toEqual([{ id: 1, document }]);
  client.invalidate();
  expect(workers[0].terminated).toBe(true);
  workers[0].reply({ kind: "error", message: "stale" });
  expect(events.at(-1)?.kind).toBe("running");
  client.run(document);
  workers[1].reply({ kind: "ready" });
  workers[1].reply({
    kind: "result",
    id: 1,
    outcome: { status: "rejected", error: { message: "old" } },
  });
  expect(events.at(-1)?.kind).toBe("running");
  workers[1].reply({
    kind: "result",
    id: 2,
    outcome: { status: "rejected", error: { message: "current" } },
  });
  expect(events.at(-1)?.kind).toBe("result");
  client.stop();
});
it("bounds loading and execution separately and supports retry", () => {
  vi.useFakeTimers();
  const { client, workers, events } = harness();
  client.run(document);
  vi.advanceTimersByTime(30_000);
  expect(workers[0].terminated).toBe(true);
  expect(events.at(-1)?.kind).toBe("error");
  client.run(document);
  workers[1].reply({ kind: "ready" });
  vi.advanceTimersByTime(10_000);
  expect(workers[1].terminated).toBe(true);
  expect(events.at(-1)?.kind).toBe("error");
  client.stop();
});
it("rejects malformed results and frees the worker", () => {
  const { client, workers, events } = harness();
  client.run(document);
  workers[0].reply({ kind: "result", id: 1, outcome: { status: "invented" } });
  expect(events.at(-1)?.kind).toBe("error");
  expect(workers[0].terminated).toBe(true);
});
it("maps UTF-16 diagnostic positions and preserves CRLF source", () => {
  const source = "// 🙂\r\nnode x = true;\n";
  const state = EditorState.create({
    doc: source,
    extensions: [EditorState.lineSeparator.of("\n")],
  });
  expect(state.doc.toString()).toBe(source);
  expect(offsetAt(state.doc, { line: 0, character: 5 })).toBe(5);
  expect(offsetAt(state.doc, { line: 1, character: 5 })).toBe(source.indexOf("x"));
});

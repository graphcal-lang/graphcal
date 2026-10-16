import { afterEach, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import type { ViewModel } from "./model.ts";
import { VsCodeHost } from "./vscodeHost.ts";

let sent: { type: string; [key: string]: unknown }[] = [];
let deliver: (message: unknown) => void = () => {};

beforeEach(() => {
  sent = [];
  vi.stubGlobal("acquireVsCodeApi", () => ({
    postMessage: (message: { type: string }) => sent.push(message),
  }));
  vi.stubGlobal("window", {
    addEventListener: (_: string, listener: (event: { data: unknown }) => void) => {
      deliver = (message) => listener({ data: message });
    },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function modelOf(project: string, file: string, text: string, revision: number): void {
  deliver({
    type: "model",
    model: {
      version: 1,
      module: { id: `${project}.main`, file, package: project },
      indexes: [],
      nodes: [],
    },
    project: `file:///work/${project}`,
    sources: { [file]: text },
    revision,
    main: true,
  });
}

function shown(host: VsCodeHost, project: string): ViewModel {
  const model = host.model(`${project}.main`);
  if (model === null) throw new Error("no model");
  return model;
}

function lastEdit(): { type: string; [key: string]: unknown } {
  const edit = sent.findLast((m) => m.type === "edit");
  if (edit === undefined) throw new Error("no edit sent");
  return edit;
}

describe("VsCodeHost", () => {
  test("names the copy's project and revision, and waits for the answer to take the edit", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 3);
    const model = shown(host, "a");
    let settled = false;
    const result = host.edit(model, { file: "src/main.gcl", next: "param n = 2;\n" });
    void result.then(() => (settled = true));
    const edit = lastEdit();
    expect(edit).toMatchObject({
      project: "file:///work/a",
      file: "src/main.gcl",
      revision: 3,
      start: 10,
      end: 11,
      text: "2",
    });
    await Promise.resolve();
    expect(settled).toBe(false);
    expect(host.source(model, "src/main.gcl")).toBe("param n = 1;\n");
    deliver({ type: "edited", id: edit.id, ok: true });
    expect(await result).toEqual({ ok: true, models: [], note: null });
    expect(host.source(model, "src/main.gcl")).toBe("param n = 2;\n");
  });

  test("keeps its copy when the extension refuses the edit", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 3);
    const model = shown(host, "a");
    const result = host.edit(model, { file: "src/main.gcl", next: "param n = 2;\n" });
    deliver({ type: "edited", id: lastEdit().id, ok: false, message: "changed" });
    expect(await result).toEqual({ ok: false, message: "changed" });
    expect(host.source(model, "src/main.gcl")).toBe("param n = 1;\n");
  });

  test("keeps the same file of two projects apart", () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 1);
    modelOf("b", "src/main.gcl", "param m = 5;\n", 2);
    expect(host.source(shown(host, "a"), "src/main.gcl")).toBe("param n = 1;\n");
    expect(host.source(shown(host, "b"), "src/main.gcl")).toBe("param m = 5;\n");
    void host.edit(shown(host, "a"), { file: "src/main.gcl", next: "param n = 2;\n" });
    expect(lastEdit()).toMatchObject({ project: "file:///work/a", revision: 1 });
  });

  test("refuses to edit a file it holds no copy of", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 1);
    expect(await host.edit(shown(host, "a"), { file: "src/other.gcl", next: "" })).toEqual({
      ok: false,
      message: "unknown file src/other.gcl",
    });
  });
});

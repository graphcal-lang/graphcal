import { afterEach, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import type { ViewModel } from "./model.ts";
import type { Range } from "./model.ts";
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

function edits(): { type: string; [key: string]: unknown }[] {
  return sent.filter((m) => m.type === "edit");
}

function lastEdit(): { type: string; [key: string]: unknown } {
  const edit = edits().at(-1);
  if (edit === undefined) throw new Error("no edit sent");
  return edit;
}

function on(line: number, from: number, to: number): Range {
  return { start: { line, character: from }, end: { line, character: to } };
}

/** Let the host take its turn. */
async function tick(): Promise<void> {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

describe("VsCodeHost", () => {
  test("sends offsets on the copy's revision and takes the edit once applied", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 3);
    const model = shown(host, "a");
    expect(host.projectedAt(model)).toBe(3);
    let settled = false;
    const result = host.edit(model, {
      file: "src/main.gcl",
      revision: 3,
      range: on(0, 10, 11),
      text: "2",
    });
    void result.then(() => (settled = true));
    await tick();
    const edit = lastEdit();
    expect(edit).toMatchObject({
      project: "file:///work/a",
      file: "src/main.gcl",
      revision: 3,
      start: 10,
      end: 11,
      text: "2",
    });
    expect(settled).toBe(false);
    expect(host.source(model, "src/main.gcl")).toEqual({ text: "param n = 1;\n", revision: 3 });
    deliver({ type: "edited", id: edit.id, ok: true, revision: 4 });
    expect(await result).toEqual({
      ok: true,
      models: [],
      note: null,
      source: { text: "param n = 2;\n", revision: 4 },
    });
    expect(host.source(model, "src/main.gcl")).toEqual({ text: "param n = 2;\n", revision: 4 });
  });

  test("two edits made on the same model both land, the second after the first (#2067)", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\nparam m = 1;\n", 3);
    const model = shown(host, "a");
    const first = host.edit(model, {
      file: "src/main.gcl",
      revision: 3,
      range: on(0, 10, 11),
      text: "20",
    });
    const second = host.edit(model, {
      file: "src/main.gcl",
      revision: 3,
      range: on(1, 10, 11),
      text: "30",
    });
    await tick();
    // One edit in flight at a time.
    expect(edits()).toHaveLength(1);
    deliver({ type: "edited", id: lastEdit().id, ok: true, revision: 4 });
    await first;
    await tick();
    expect(edits()).toHaveLength(2);
    // Placed on the copy the first edit left, at its revision.
    expect(lastEdit()).toMatchObject({ revision: 4, start: 24, end: 25, text: "30" });
    deliver({ type: "edited", id: lastEdit().id, ok: true, revision: 5 });
    expect(await second).toMatchObject({
      ok: true,
      source: { text: "param n = 20;\nparam m = 30;\n", revision: 5 },
    });
  });

  test("keeps its copy when the extension refuses the edit", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 3);
    const model = shown(host, "a");
    const result = host.edit(model, {
      file: "src/main.gcl",
      revision: 3,
      range: on(0, 10, 11),
      text: "2",
    });
    await tick();
    deliver({ type: "edited", id: lastEdit().id, ok: false, message: "changed" });
    expect(await result).toEqual({ ok: false, message: "changed" });
    expect(host.source(model, "src/main.gcl")).toEqual({ text: "param n = 1;\n", revision: 3 });
  });

  test("an answer it cannot read ends the edit", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 3);
    const result = host.edit(shown(host, "a"), {
      file: "src/main.gcl",
      revision: 3,
      range: on(0, 10, 11),
      text: "2",
    });
    await tick();
    // An extension from before revisions were answered.
    deliver({ type: "edited", id: lastEdit().id, ok: true });
    expect(await result).toMatchObject({ ok: false });
  });

  test("keeps the same file of two projects apart", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 1);
    modelOf("b", "src/main.gcl", "param m = 5;\n", 2);
    expect(host.source(shown(host, "a"), "src/main.gcl")?.text).toBe("param n = 1;\n");
    expect(host.source(shown(host, "b"), "src/main.gcl")?.text).toBe("param m = 5;\n");
    void host.edit(shown(host, "a"), {
      file: "src/main.gcl",
      revision: 1,
      range: on(0, 10, 11),
      text: "2",
    });
    await tick();
    expect(lastEdit()).toMatchObject({ project: "file:///work/a", revision: 1 });
  });

  test("refuses to edit a file it holds no copy of", async () => {
    const host = new VsCodeHost();
    modelOf("a", "src/main.gcl", "param n = 1;\n", 1);
    expect(
      await host.edit(shown(host, "a"), {
        file: "src/other.gcl",
        revision: 1,
        range: on(0, 0, 0),
        text: "",
      }),
    ).toEqual({ ok: false, message: "unknown file src/other.gcl" });
  });
});

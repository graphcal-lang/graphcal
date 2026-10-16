import { describe, expect, test } from "vite-plus/test";
import { diffModels, previousValue, summarize } from "./compare.ts";
import type { Value, ViewModel, ViewNode } from "./model.ts";

function node(
  id: string,
  expr: string,
  value: Value | null,
  extra: Partial<ViewNode> = {},
): ViewNode {
  return {
    id,
    name: id,
    module: "p.m",
    kind: "node",
    axes: [],
    dimension: "Power",
    unit: "W",
    pub: false,
    deps: [],
    expr,
    reduce: null,
    label: id,
    origin: { kind: "local" },
    source: null,
    value,
    ...extra,
  };
}
const q = (display: string): Value => ({
  kind: "quantity",
  display,
  magnitude: display,
  si_value: Number(display),
  unit: "W",
});
function model(nodes: ViewNode[]): ViewModel {
  return { version: 0, module: { id: "p.m", file: "src/m.gcl", package: "p" }, indexes: [], nodes };
}

describe("diffModels", () => {
  test("tells added, redefined, revalued and removed nodes apart", () => {
    const base = model([
      node("a", "1.0 W", q("1")),
      node("b", "@a * 2.0", q("2")),
      node("c", "3.0 W", q("3")),
      node("gone", "4.0 W", q("4")),
    ]);
    const head = model([
      node("a", "1.5 W", q("1.5")),
      node("b", "@a  * 2.0", q("3")),
      node("c", "3.0 W", q("3")),
      node("fresh", "5.0 W", q("5")),
    ]);
    const diff = diffModels(base, head);
    expect([...diff.nodes.entries()].map(([id, c]) => `${id}:${c.kind}`)).toEqual([
      "a:expr",
      "b:value",
      "fresh:added",
    ]);
    expect(diff.removed.map((n) => n.id)).toEqual(["gone"]);
    expect(summarize(diff)).toBe("1 new · 2 changed · 1 removed (gone)");
  });

  test("everything downstream of a change is lit, even with the same value", () => {
    const base = model([
      node("p", "60.0 W", q("60")),
      node("ok", "@need < @p", { kind: "bool", display: "PASS", value: true }, { deps: ["p"] }),
      node("far", "@ok", { kind: "bool", display: "PASS", value: true }, { deps: ["ok"] }),
      node("other", "1.0 W", q("1")),
    ]);
    const head = model([
      node("p", "49.0 W", q("49")),
      node("ok", "@need < @p", { kind: "bool", display: "PASS", value: true }, { deps: ["p"] }),
      node("far", "@ok", { kind: "bool", display: "PASS", value: true }, { deps: ["ok"] }),
      node("other", "1.0 W", q("1")),
    ]);
    const diff = diffModels(base, head);
    expect([...diff.nodes.entries()].map(([id, c]) => `${id}:${c.kind}`)).toEqual([
      "p:expr",
      "ok:downstream",
      "far:downstream",
    ]);
    expect(summarize(diff)).toBe("1 changed · 2 downstream");
    expect(previousValue(diff.nodes.get("ok"), null, [])).toBeUndefined();
  });

  test("a file new at the base has every node added", () => {
    const diff = diffModels(null, model([node("a", "1.0 W", q("1"))]));
    expect(diff.nodes.get("a")).toEqual({ kind: "added" });
    expect(summarize(diffModels(model([]), model([])))).toBe("no change");
  });

  test("previousValue finds the base cell that differs", () => {
    const indexed = (entries: [string, string][]): Value => ({
      kind: "indexed",
      display: "[C]",
      index: "C",
      entries: entries.map(([key, d]) => ({ key, value: q(d) })),
    });
    const before = node(
      "t",
      "table",
      indexed([
        ["x", "1"],
        ["y", "2"],
      ]),
      { axes: ["C"] },
    );
    const after = node(
      "t",
      "table",
      indexed([
        ["x", "1"],
        ["y", "9"],
      ]),
      { axes: ["C"] },
    );
    const change = diffModels(model([before]), model([after])).nodes.get("t");
    expect(change?.kind).toBe("value");
    expect(previousValue(change, after.value, ["x"])).toBeUndefined();
    expect(previousValue(change, after.value, ["y"])).toEqual(q("2"));
    expect(previousValue({ kind: "added" }, after.value, ["y"])).toBeUndefined();
  });
});

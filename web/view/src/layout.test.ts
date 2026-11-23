import { describe, expect, test } from "vite-plus/test";
import { computeLayout, receivedBy } from "./layout.ts";
import { viewModel, viewNode } from "./testModels.ts";

const imported = (id: string) =>
  viewNode(id, { kind: "const", module: "p.nominal", origin: { kind: "imported" } });

describe("a param that only names an imported value", () => {
  test("stands for it: one column, under the param, with the value's origin", () => {
    const model = viewModel([
      imported("n::margin"),
      viewNode("margin", { deps: ["n::margin"], receives: "n::margin" }),
      viewNode("total", { kind: "node", deps: ["margin"] }),
    ]);
    const layout = computeLayout(model);
    expect([...layout.columns.keys()].sort()).toEqual(["margin", "total"]);
    expect(layout.columns.get("margin")?.received?.id).toBe("n::margin");
    expect(layout.columns.get("total")?.received).toBeNull();
    expect([...layout.standIns]).toEqual([["n::margin", "margin"]]);
  });

  test("a value a formula also reads directly keeps its own column", () => {
    const model = viewModel([
      imported("n::limit"),
      viewNode("limit", { deps: ["n::limit"], receives: "n::limit" }),
      viewNode("check", { kind: "assert", deps: ["limit", "n::limit"] }),
    ]);
    expect(receivedBy(model).size).toBe(0);
    expect(computeLayout(model).columns.size).toBe(3);
  });

  test("two params that receive the same value are both drawn, with the value", () => {
    const model = viewModel([
      imported("n::margin"),
      viewNode("a", { deps: ["n::margin"], receives: "n::margin" }),
      viewNode("b", { deps: ["n::margin"], receives: "n::margin" }),
    ]);
    expect(receivedBy(model).size).toBe(0);
  });

  test("a copy of a value of this file, or of another sheet's result, is not merged", () => {
    const model = viewModel([
      viewNode("own"),
      viewNode("copy", { deps: ["own"], receives: "own" }),
      viewNode("e::out", {
        kind: "node",
        module: "p.e",
        origin: { kind: "included", instance: { id: "i", name: "e", bound: false } },
      }),
      viewNode("out", { deps: ["e::out"], receives: "e::out" }),
    ]);
    expect(receivedBy(model).size).toBe(0);
  });

  test("a model from an engine that does not say what a param receives is drawn as before", () => {
    const model = viewModel([imported("n::margin"), viewNode("margin", { deps: ["n::margin"] })]);
    expect(computeLayout(model).columns.size).toBe(2);
  });
});

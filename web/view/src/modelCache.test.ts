import { describe, expect, test } from "vite-plus/test";
import type { ViewModel } from "./model.ts";
import { ModelCache } from "./modelCache.ts";

function model(id: string, revision: number): ViewModel {
  return {
    version: 1,
    module: { id, file: `src/${id}.gcl`, package: "p" },
    indexes: [],
    nodes: [
      {
        id: "x",
        name: "x",
        module: id,
        kind: "node",
        axes: [],
        dimension: "Dimensionless",
        unit: null,
        pub: true,
        deps: [],
        expr: String(revision),
        label: "x",
        origin: { kind: "local" },
        element: "quantity",
        presentation: [],
        source: null,
        value: null,
      },
    ],
  };
}

describe("ModelCache", () => {
  test("after an edit only the re-projected models are current", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1), model("p.b", 1)]);
    // `a` was on screen and re-projected; `b`, which reads `a`, was not.
    cache.edited([model("p.a", 2)]);
    expect(cache.current("p.a")?.nodes[0]?.expr).toBe("2");
    expect(cache.current("p.b")).toBeNull();
    expect(cache.isStale("p.b")).toBe(true);
    expect(cache.currentModels().map((m) => m.module.id)).toEqual(["p.a"]);
    // The stale model still says where the module is, to project it again.
    expect(cache.last("p.b")?.module.file).toBe("src/p.b.gcl");
    expect(cache.moduleOfFile("src/p.b.gcl")).toBe("p.b");
    expect(cache.ids()).toEqual(["p.a", "p.b"]);
  });

  test("projecting a stale module again makes it current", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1), model("p.b", 1)]);
    cache.edited([]);
    cache.set(model("p.b", 2));
    expect(cache.current("p.b")?.nodes[0]?.expr).toBe("2");
    expect(cache.current("p.a")).toBeNull();
  });

  test("a reset to the models as read makes them all current again", () => {
    const cache = new ModelCache();
    const read = [model("p.a", 1), model("p.b", 1)];
    cache.replace(read);
    cache.edited([model("p.a", 2)]);
    cache.replace(read);
    expect(cache.current("p.a")?.nodes[0]?.expr).toBe("1");
    expect(cache.current("p.b")?.nodes[0]?.expr).toBe("1");
  });
});

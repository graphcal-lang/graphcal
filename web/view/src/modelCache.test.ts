import { describe, expect, test } from "vite-plus/test";
import { ModelCache } from "./modelCache.ts";
import { viewModel, viewNode } from "./testModels.ts";

function model(id: string, tag: number) {
  return viewModel([viewNode("x", { module: id, expr: String(tag) })], {
    module: { id, file: `src/${id}.gcl`, package: "p" },
  });
}

describe("ModelCache", () => {
  test("after an edit only the re-projected models are current", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1), model("p.b", 1)], 1);
    // `a` was on screen and re-projected; `b`, which reads `a`, was not.
    const a = model("p.a", 2);
    cache.edited([a], 2);
    expect(cache.current("p.a")?.nodes[0]?.expr).toBe("2");
    expect(cache.revisionOf(a)).toBe(2);
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
    cache.replace([model("p.a", 1), model("p.b", 1)], 1);
    cache.edited([], 2);
    expect(cache.set(model("p.b", 2), 2)).toBe(true);
    expect(cache.current("p.b")?.nodes[0]?.expr).toBe("2");
    expect(cache.current("p.a")).toBeNull();
  });

  test("a projection finished after the sources moved on is not kept", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1)], 1);
    cache.edited([model("p.a", 3)], 3);
    expect(cache.set(model("p.a", 2), 2)).toBe(false);
    expect(cache.current("p.a")?.nodes[0]?.expr).toBe("3");
  });

  test("a reset to the models as read makes them all current again", () => {
    const cache = new ModelCache();
    const read = [model("p.a", 1), model("p.b", 1)];
    cache.replace(read, 1);
    cache.edited([model("p.a", 2)], 2);
    cache.replace(read, 3);
    expect(cache.current("p.a")?.nodes[0]?.expr).toBe("1");
    expect(cache.current("p.b")?.nodes[0]?.expr).toBe("1");
  });
});

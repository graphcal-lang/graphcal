import { describe, expect, test } from "vite-plus/test";
import type { ViewModel } from "./model.ts";
import { ModelCache } from "./modelCache.ts";
import { moduleTarget, type ViewTarget } from "./target.ts";
import { viewModel, viewNode } from "./testModels.ts";

function model(id: string, tag: number) {
  return viewModel([viewNode("x", { module: id, expr: String(tag) })], {
    module: { id, file: `src/${id}.gcl`, package: "p" },
  });
}

/** The view of `lib` as the include instance `instance` of `p.main`. */
function instance(instance: string, tag: number): ViewModel {
  return viewModel([viewNode("x", { module: "p.lib", expr: String(tag) })], {
    module: { id: "p.lib", file: "src/lib.gcl", package: "p" },
    target: {
      kind: "instance",
      root: { id: "p.main", file: "src/main.gcl", package: "p" },
      instance: { id: instance, name: instance, bound: true },
    },
  });
}

const a = moduleTarget("p.a");
const b = moduleTarget("p.b");

describe("ModelCache", () => {
  test("after an edit only the re-projected models are current", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1), model("p.b", 1)], 1);
    // `a` was on screen and re-projected; `b`, which reads `a`, was not.
    const fresh = model("p.a", 2);
    cache.edited([fresh], 2);
    expect(cache.current(a)?.nodes[0]?.expr).toBe("2");
    expect(cache.revisionOf(fresh)).toBe(2);
    expect(cache.current(b)).toBeNull();
    expect(cache.isStale(b)).toBe(true);
    expect(cache.currentModels().map((m) => m.module.id)).toEqual(["p.a"]);
    // The stale model still says where the module is, to project it again.
    expect(cache.last(b)?.module.file).toBe("src/p.b.gcl");
    expect(cache.moduleOfFile("src/p.b.gcl")).toBe("p.b");
    expect(cache.moduleIds()).toEqual(["p.a", "p.b"]);
  });

  test("projecting a stale module again makes it current", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1), model("p.b", 1)], 1);
    cache.edited([], 2);
    expect(cache.set(model("p.b", 2), 2)).toBe(true);
    expect(cache.current(b)?.nodes[0]?.expr).toBe("2");
    expect(cache.current(a)).toBeNull();
  });

  test("a projection finished after the sources moved on is not kept", () => {
    const cache = new ModelCache();
    cache.replace([model("p.a", 1)], 1);
    cache.edited([model("p.a", 3)], 3);
    expect(cache.set(model("p.a", 2), 2)).toBe(false);
    expect(cache.current(a)?.nodes[0]?.expr).toBe("3");
  });

  test("a reset to the models as read makes them all current again", () => {
    const cache = new ModelCache();
    const read = [model("p.a", 1), model("p.b", 1)];
    cache.replace(read, 1);
    cache.edited([model("p.a", 2)], 2);
    cache.replace(read, 3);
    expect(cache.current(a)?.nodes[0]?.expr).toBe("1");
    expect(cache.current(b)?.nodes[0]?.expr).toBe("1");
  });

  test("an include instance is kept apart from its module alone and from other instances", () => {
    const cache = new ModelCache();
    const lib = moduleTarget("p.lib");
    const p: ViewTarget = { kind: "instance", root: "p.main", instance: "i:p" };
    const q: ViewTarget = { kind: "instance", root: "p.main", instance: "i:q" };
    cache.replace([model("p.lib", 1), instance("i:p", 2), instance("i:q", 3)], 1);
    expect(cache.current(lib)?.nodes[0]?.expr).toBe("1");
    expect(cache.current(p)?.nodes[0]?.expr).toBe("2");
    expect(cache.current(q)?.nodes[0]?.expr).toBe("3");
    // Only a module alone is a module of the host; its file names it.
    expect(cache.moduleIds()).toEqual(["p.lib"]);
    expect(cache.moduleOfFile("src/p.lib.gcl")).toBe("p.lib");
    // An instance's file is its template's, but no module of the host by itself.
    expect(cache.moduleOfFile("src/lib.gcl")).toBeNull();
    // The same instance id under another root is another target.
    expect(cache.current({ kind: "instance", root: "p.other", instance: "i:p" })).toBeNull();
    cache.edited([instance("i:p", 4)], 2);
    expect(cache.current(p)?.nodes[0]?.expr).toBe("4");
    expect(cache.isStale(q)).toBe(true);
    expect(cache.isStale(lib)).toBe(true);
  });
});

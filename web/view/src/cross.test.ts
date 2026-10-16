import { describe, expect, test } from "vite-plus/test";
import { crossLinks, type CrossSheet } from "./cross.ts";
import { computeLayout } from "./layout.ts";
import type { ViewModel, ViewNode } from "./model.ts";
import { keyOf, type PaneKey } from "./panes.ts";

function node(id: string, extra: Partial<ViewNode> = {}): ViewNode {
  return {
    id,
    name: id,
    module: "p.main",
    kind: "node",
    axes: [],
    dimension: "Dimensionless",
    unit: null,
    pub: false,
    deps: [],
    expr: null,
    reduce: null,
    label: id,
    origin: { kind: "local" },
    element: "quantity",
    presentation: [],
    source: null,
    value: null,
    ...extra,
  };
}

function sheet(key: PaneKey, id: string, nodes: ViewNode[]): CrossSheet {
  const model: ViewModel = {
    version: 0,
    module: { id, file: `src/${id}.gcl`, package: "p" },
    indexes: [],
    nodes,
  };
  // The links read only the models and layouts; no element is touched.
  const element = {} as HTMLElement;
  return { key, model, layout: computeLayout(model), viewEl: element, paneEl: element };
}

/** The main sheet reading `lib`'s `out` through the include `l`, and `lib` pinned as that include. */
function sheets(bound: boolean): CrossSheet[] {
  const external = node("l::out", {
    name: "out",
    module: "p.lib",
    origin: { kind: "included", instance: { name: "l", bound } },
  });
  const main = sheet({ module: "p.main", instance: null }, "p.main", [
    external,
    node("y", { deps: ["l::out"] }),
  ]);
  const lib = sheet(keyOf(external), "p.lib", [node("out", { module: "p.lib", pub: true })]);
  return [main, lib];
}

describe("crossLinks", () => {
  test("wires an unbound include's value to its pane", () => {
    expect(crossLinks(sheets(false)).map((link) => [link.fromId, link.toId])).toEqual([
      ["out", "l::out"],
    ]);
  });

  test("never wires a bound include's value to the module evaluated alone", () => {
    expect(crossLinks(sheets(true))).toEqual([]);
  });
});

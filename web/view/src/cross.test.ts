import { describe, expect, test } from "vite-plus/test";
import { crossLinks, type CrossSheet } from "./cross.ts";
import { computeLayout } from "./layout.ts";
import type { ViewModel, ViewNode } from "./model.ts";
import { depthsOf, references } from "./panes.ts";
import { moduleTarget, readTarget, targetOf, type ViewTarget } from "./target.ts";

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
    label: id,
    origin: { kind: "local" },
    element: "quantity",
    presentation: [],
    source: null,
    value: null,
    ...extra,
  };
}

const MAIN = { id: "p.main", file: "src/main.gcl", package: "p" };
const LIB = { id: "p.lib", file: "src/lib.gcl", package: "p" };

function sheetOf(model: ViewModel): CrossSheet {
  // The links read only the models and layouts; no element is touched.
  const element = {} as HTMLElement;
  return {
    key: targetOf(model),
    model,
    layout: computeLayout(model),
    viewEl: element,
    paneEl: element,
  };
}

const instance = { id: "i:l", name: "l", bound: true };

/**
 * `main` reads `lib`'s `out` through the include `l`, which binds `gain` to
 * main's `k`; the instance's view reads `k` back as its includer's.
 */
function project(): { main: ViewModel; l: ViewModel; alone: ViewModel } {
  const main: ViewModel = {
    version: 1,
    module: MAIN,
    target: { kind: "module" },
    indexes: [],
    nodes: [
      node("k", { kind: "param" }),
      node("i:l/out", {
        label: "l::out",
        name: "out",
        module: "p.lib",
        origin: { kind: "included", instance },
      }),
      node("y", { deps: ["i:l/out"] }),
    ],
  };
  const l: ViewModel = {
    version: 1,
    module: LIB,
    target: { kind: "instance", root: MAIN, instance },
    indexes: [],
    nodes: [
      node("i:l/gain", { name: "gain", module: "p.lib", kind: "param", deps: ["k"] }),
      node("i:l/out", { name: "out", module: "p.lib", pub: true, deps: ["i:l/gain"] }),
      node("k", { origin: { kind: "includer" }, kind: "param" }),
    ],
  };
  const alone: ViewModel = {
    version: 1,
    module: LIB,
    target: { kind: "module" },
    indexes: [],
    nodes: [
      node("lib/gain", { name: "gain", module: "p.lib", kind: "param" }),
      node("lib/out", { name: "out", module: "p.lib", pub: true, deps: ["lib/gain"] }),
    ],
  };
  return { main, l, alone };
}

function pairs(sheets: CrossSheet[]): [string, string][] {
  return crossLinks(sheets).map((link) => [link.fromId, link.toId]);
}

describe("readTarget", () => {
  const { main, l } = project();
  const at = (model: ViewModel, id: string): ViewNode => {
    const found = model.nodes.find((n) => n.id === id);
    if (!found) throw new Error(id);
    return found;
  };

  test("an include member is read from the instance in the reader's root", () => {
    expect(readTarget(targetOf(main), at(main, "i:l/out"))).toEqual({
      kind: "instance",
      root: "p.main",
      instance: "i:l",
    });
  });

  test("an imported value is read from its module alone", () => {
    const imported = node("x", { module: "p.units", origin: { kind: "imported" } });
    expect(readTarget(targetOf(l), imported)).toEqual(moduleTarget("p.units"));
  });

  test("an includer's declaration and a call output have no sheet to pin", () => {
    expect(readTarget(targetOf(l), at(l, "k"))).toBeNull();
    const called = node("c", {
      origin: { kind: "called", instance: { name: "f(…)", bound: true } },
    });
    expect(readTarget(targetOf(main), called)).toBeNull();
  });

  test("a nested include is read in the same root", () => {
    const nested = node("i:l/i:q/x", {
      origin: { kind: "included", instance: { id: "i:l/i:q", name: "q", bound: false } },
    });
    expect(readTarget(targetOf(l), nested)).toEqual({
      kind: "instance",
      root: "p.main",
      instance: "i:l/i:q",
    });
  });
});

describe("crossLinks", () => {
  test("wires a bound include's value from its instance's pane", () => {
    const { main, l } = project();
    expect(pairs([sheetOf(main), sheetOf(l)])).toEqual([
      ["i:l/out", "i:l/out"],
      // The includer's `k`, read by the include's argument, goes into the pane.
      ["k", "k"],
    ]);
  });

  test("never wires an include's value to its module evaluated alone", () => {
    const { main, alone } = project();
    expect(pairs([sheetOf(main), sheetOf(alone)])).toEqual([]);
  });

  test("wires an imported value to its module's pane by name", () => {
    const units: ViewModel = {
      version: 1,
      module: { id: "p.units", file: "src/units.gcl", package: "p" },
      target: { kind: "module" },
      indexes: [],
      nodes: [node("units/g", { name: "g", module: "p.units" })],
    };
    const reader: ViewModel = {
      ...project().main,
      nodes: [node("g", { module: "p.units", origin: { kind: "imported" } })],
    };
    expect(pairs([sheetOf(reader), sheetOf(units)])).toEqual([["units/g", "g"]]);
  });
});

describe("panes", () => {
  test("an instance's pane sits beside its reader, and is the target it reads", () => {
    const { main, l } = project();
    const expected: ViewTarget = { kind: "instance", root: "p.main", instance: "i:l" };
    expect(references(main)).toEqual([expected]);
    // The includer's declaration does not make the main sheet a pane of the instance.
    expect(references(l)).toEqual([]);
    expect(depthsOf([main, l].map((model) => ({ key: targetOf(model), model })))).toEqual([0, 1]);
  });
});

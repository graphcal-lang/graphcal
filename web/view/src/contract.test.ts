// The model contract: what `graphcal view` writes for the fixtures in
// `contract/` (kept current by the CLI's `view_contract_fixtures_are_current`
// test) parses with the page's schemas, and the page can join every part of
// it: dependencies to nodes, axes to indexes, value entries to axis entries.
import { describe, expect, test } from "vite-plus/test";
import { computeLayout, entryName } from "./layout.ts";
import { parseViewModel, type Value, type ViewModel } from "./model.ts";
import { valueAt } from "./values.ts";

const fixtures = import.meta.glob<unknown>("./contract/*.json", {
  eager: true,
  import: "default",
});

function models(): [string, ViewModel][] {
  return Object.entries(fixtures).map(([path, raw]) => [path, parseViewModel(raw)]);
}

/** Every path of entry keys through an indexed value, outermost first. */
function keyPaths(value: Value | null, depth: number): string[][] {
  if (depth === 0) return [[]];
  if (value === null || value.kind !== "indexed") return [];
  return value.entries.flatMap((entry) =>
    keyPaths(entry.value, depth - 1).map((rest) => [entry.key, ...rest]),
  );
}

describe("view model contract", () => {
  test("there are fixtures", () => {
    expect(Object.keys(fixtures).length).toBeGreaterThan(0);
  });

  test.each(models())("%s parses and joins", (_path, model) => {
    const ids = new Set(model.nodes.map((n) => n.id));
    expect(ids.size).toBe(model.nodes.length);
    const indexes = new Map(model.indexes.map((i) => [i.id, i]));
    for (const node of model.nodes) {
      for (const dep of node.deps) expect(ids.has(dep), `${node.label} reads ${dep}`).toBe(true);
      for (const axis of node.axes) expect(indexes.has(axis), `${node.label} axis`).toBe(true);
    }
    const layout = computeLayout(model);
    expect(layout.columns.size).toBe(model.nodes.length);
  });

  test.each(models())("%s: every cell of a value over known axes finds its value", (_p, model) => {
    const layout = computeLayout(model);
    for (const node of model.nodes) {
      if (node.value === null || node.axes.length === 0 || node.axes.length > 2) continue;
      const axes = node.axes.map((axis) => layout.axes.get(axis));
      if (axes.some((axis) => axis === undefined || axis.keys.length === 0)) continue;
      // The cells the sheet draws: one per entry key of each axis.
      const [rows = [], subs = [null]] = axes.map((axis) => axis?.keys ?? []);
      for (const row of rows) {
        for (const sub of subs.length > 0 ? subs : [null]) {
          const keys = sub === null ? [row] : [row, sub];
          expect(valueAt(node.value, keys), `${node.label}[${keys.join(", ")}]`).not.toBeNull();
        }
      }
      // And the value has no entry the axes do not list.
      for (const path of keyPaths(node.value, node.axes.length)) {
        path.forEach((key, i) => expect(axes[i]?.keys).toContain(key));
      }
    }
  });

  test("a coordinate axis shows its coordinates (#2060)", () => {
    const model = models().find(([path]) => path.endsWith("range_index.json"))?.[1];
    expect(model).toBeDefined();
    if (!model) return;
    const layout = computeLayout(model);
    const x = model.nodes.find((n) => n.label === "x");
    const axis = x?.axes[0] ?? "";
    const names = (layout.axes.get(axis)?.keys ?? []).map((key) =>
      entryName(layout.axes, axis, key),
    );
    expect(names).toEqual(["0 s", "0.5 s", "1 s"]);
  });
});

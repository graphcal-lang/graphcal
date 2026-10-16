import { describe, expect, test } from "vite-plus/test";
import { literalFor, planCellEdit, planExprEdit } from "./editing.ts";
import type { Range } from "./model.ts";
import { quantity, viewNode } from "./testModels.ts";

function range(line: number, from: number, to: number): Range {
  return { start: { line, character: from }, end: { line, character: to } };
}

describe("literalFor", () => {
  test("a quantity gets a decimal point and the cell's unit", () => {
    const node = viewNode("p");
    expect(literalFor(node, "W", "5")).toBe("5.0 W");
    expect(literalFor(node, "W", "-2.5e3")).toBe("-2.5e3 W");
    expect(literalFor(node, null, "5")).toBe("5.0");
  });

  test("a unit typed out is kept as written", () => {
    expect(literalFor(viewNode("p"), "W", " 5.0 kW ")).toBe("5.0 kW");
  });

  test("an Int is written as typed", () => {
    expect(
      literalFor(viewNode("n", { element: "int", dimension: "Int", unit: null }), null, "5"),
    ).toBe("5");
  });

  test("a Bool is written as typed", () => {
    expect(literalFor(viewNode("b", { element: "bool", dimension: "Bool" }), null, "true")).toBe(
      "true",
    );
  });
});

describe("planning an edit", () => {
  const scalar = viewNode("p", {
    expr: "2.0 W",
    value: quantity("2", "W"),
    source: { file: "src/m.gcl", line: 1, expr_range: range(0, 17, 22), cells: [] },
  });

  test("a scalar cell replaces the expression's range, on the model's revision", () => {
    expect(planCellEdit(scalar, [], 4, "3")).toEqual({
      ok: true,
      edit: { file: "src/m.gcl", revision: 4, range: range(0, 17, 22), text: "3.0 W" },
    });
  });

  test("an expression shared by several declarations is not replaced", () => {
    const shared = viewNode("p", {
      expr: "table[…]",
      source: { file: "src/m.gcl", line: 1, expr_range: null, cells: [] },
    });
    expect(planExprEdit(shared, 1, "1.0 W")).toEqual({
      ok: false,
      message: "this expression is shared with other declarations; edit its cells",
    });
  });

  test("a table cell replaces the cell's own range, in the unit it is shown in", () => {
    const node = viewNode("p", {
      axes: ["m"],
      expr: "table[M] { a: 1.0 W; b: 2.0 kW; }",
      value: {
        kind: "indexed",
        display: "…",
        index: "m",
        entries: [
          { key: "ka", value: quantity("1", "W") },
          { key: "kb", value: quantity("2", "kW") },
        ],
      },
      source: {
        file: "src/m.gcl",
        line: 1,
        expr_range: range(0, 20, 53),
        cells: [
          { keys: ["ka"], range: range(0, 34, 39) },
          { keys: ["kb"], range: range(0, 44, 50) },
        ],
      },
    });
    expect(planCellEdit(node, ["kb"], 2, "7")).toEqual({
      ok: true,
      edit: { file: "src/m.gcl", revision: 2, range: range(0, 44, 50), text: "7.0 kW" },
    });
  });

  test("nothing is planned when the host does not hold the file", () => {
    expect(planCellEdit(scalar, [], null, "3")).toEqual({
      ok: false,
      message: "this value cannot be edited",
    });
  });

  test("a cell the compiler reports no range for is read-only", () => {
    const node = viewNode("t", {
      axes: ["m"],
      expr: "for m: M { 1.0 W }",
      source: { file: "src/m.gcl", line: 1, expr_range: range(0, 0, 18), cells: [] },
    });
    expect(planCellEdit(node, ["ka"], 1, "2")).toEqual({
      ok: false,
      message: "this value cannot be edited",
    });
  });
});

import { describe, expect, test } from "vite-plus/test";
import { EditError, type Range, replaceTableCell } from "./source.ts";

/** The range of the expression that starts at `table[` on 0-based `line` and ends after the `}` on `endLine`. */
function tableRange(source: string, line: number, endLine: number): Range {
  const rows = source.split("\n");
  const start = (rows[line] ?? "").indexOf("table[");
  const end = (rows[endLine] ?? "").lastIndexOf("}") + 1;
  return { start: { line, character: start }, end: { line: endLine, character: end } };
}

describe("replaceTableCell", () => {
  test("replaces a one-axis entry of its own table", () => {
    const source = ["param p: Power[C] = table[C] {", "    a: 1.0 W;", "    b: 2.0 W;", "};"].join(
      "\n",
    );
    expect(replaceTableCell(source, tableRange(source, 0, 3), "b", null, "5.0 W")).toBe(
      source.replace("2.0 W", "5.0 W"),
    );
  });

  test("replaces the column a header names", () => {
    const source = [
      "param d: Dimensionless[C, M] = table[C, M] {",
      "    : Safe, Nominal;",
      "    a: 0.1, 0.2;",
      "};",
    ].join("\n");
    expect(replaceTableCell(source, tableRange(source, 0, 3), "a", "Nominal", "0.5")).toBe(
      source.replace("0.2", "0.5"),
    );
  });

  test("never reaches the next declaration's table", () => {
    // `a` closes on its own line; a search past its range would find `b`'s row `x`.
    const source = [
      "param a: Power[M] = table[M] { x: 1.0 W; y: 2.0 W; };",
      "param b: Power[M] = table[M] {",
      "    x: 3.0 W;",
      "    y: 4.0 W;",
      "};",
    ].join("\n");
    expect(() => replaceTableCell(source, tableRange(source, 0, 0), "x", null, "9.0 W")).toThrow(
      EditError,
    );
    const edited = replaceTableCell(source, tableRange(source, 1, 4), "x", null, "9.0 W");
    expect(edited).toBe(source.replace("3.0 W", "9.0 W"));
  });

  test("a row outside the range is not found", () => {
    const source = [
      "param a: Power[M] = table[M] {",
      "    y: 2.0 W;",
      "};",
      "param b: Power[M] = table[M] {",
      "    x: 3.0 W;",
      "};",
    ].join("\n");
    expect(() => replaceTableCell(source, tableRange(source, 0, 2), "x", null, "9.0 W")).toThrow(
      "row x is not in the table literal",
    );
  });

  test("refuses an expression that is not a table literal", () => {
    const source = "node n: Power[M] = for m: M {\n    1.0 W\n};";
    const range = { start: { line: 0, character: 19 }, end: { line: 2, character: 1 } };
    expect(() => replaceTableCell(source, range, "x", null, "9.0 W")).toThrow(EditError);
  });

  test("refuses a table shared by several declarations", () => {
    const source = [
      "param p: Power[C],",
      "param n: Int[C]",
      "    = table[C, (_, _)] {",
      "        : _, _;",
      "        a: 10.0 W, 1;",
      "    };",
    ].join("\n");
    expect(() => replaceTableCell(source, tableRange(source, 2, 5), "a", null, "9.0 W")).toThrow(
      "a table shared by several declarations cannot be edited cell by cell",
    );
  });
});

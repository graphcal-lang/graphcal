import { describe, expect, test } from "vite-plus/test";
import { relatesTo, selectedEntries } from "./highlight.ts";
import { viewNode } from "./testModels.ts";

describe("related cells", () => {
  const entries = selectedEntries(
    { id: "a", row: "safe", sub: "x", mode: "deps" },
    viewNode("a", { axes: ["mode", "component"] }),
  );

  test("a cell agreeing on every shared axis relates", () => {
    const cell = new Map([["mode", "safe"]]);
    expect(relatesTo(entries, ["mode"], (axis) => cell.get(axis))).toBe(true);
  });

  test("a cell differing on a shared axis does not", () => {
    const cell = new Map([["mode", "nominal"]]);
    expect(relatesTo(entries, ["mode"], (axis) => cell.get(axis))).toBe(false);
  });

  test("a node sharing no axis has no related cell", () => {
    expect(relatesTo(entries, ["other"], () => "safe")).toBe(false);
  });
});

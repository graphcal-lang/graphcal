import { describe, expect, test } from "vite-plus/test";
import { axesOf } from "./layout.ts";
import { inlaysOf } from "./inlays.ts";
import { quantity, viewModel, viewNode } from "./testModels.ts";

const at = (line: number) => ({ file: "src/m.gcl", line, expr_range: null, cells: [] });

describe("inlaysOf", () => {
  const model = viewModel(
    [
      viewNode("p", { source: at(1), value: quantity("2", "W") }),
      viewNode("t", {
        axes: ["ix"],
        source: at(2),
        value: {
          kind: "indexed",
          display: "…",
          index: "ix",
          entries: [{ key: "k0", value: quantity("5", "W") }],
        },
      }),
      viewNode("elsewhere", { source: { ...at(3), file: "src/other.gcl" } }),
    ],
    {
      indexes: [
        { id: "ix", name: "Mode", module: "p.m", entries: [{ key: "k0", display: "Safe" }] },
      ],
    },
  );

  test("shows scalars and the axes of indexed values on their lines", () => {
    expect(inlaysOf(model, axesOf(model), null)).toEqual([
      { line: 1, text: "= 2 W", selected: false },
      { line: 2, text: "[Mode]", selected: false },
    ]);
  });

  test("shows the selected cell's entry and value", () => {
    const selection = { id: "t", row: "k0", sub: null, mode: "deps" as const };
    expect(inlaysOf(model, axesOf(model), selection)[1]).toEqual({
      line: 2,
      text: "[Mode] · Safe = 5 W",
      selected: true,
    });
  });
});

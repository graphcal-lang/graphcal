import { describe, expect, test } from "vite-plus/test";
import powerMax from "./contract/power_max.json";
import { acceptEdit, projectionNote, projectionOf } from "./projection.ts";
import { viewModel, viewNode } from "./testModels.ts";

describe("projectionOf", () => {
  test("reads an evaluated view", () => {
    const projection = projectionOf({ status: "evaluated", view: powerMax });
    expect(projection.status).toBe("evaluated");
  });

  test("a required param without a value keeps the structure", () => {
    const structure = viewModel([viewNode("span")]);
    expect(
      projectionOf({
        status: "needs_values",
        view: structure,
        message: "required param `span` has no value",
      }),
    ).toEqual({
      status: "needs_values",
      view: structure,
      message: "required param `span` has no value",
    });
  });

  test("rejected bindings and evaluator failures are failed evaluations", () => {
    const structure = viewModel([]);
    expect(
      projectionOf({
        status: "binding_errors",
        view: structure,
        errors: [{ name: "x", message: "bad" }],
      }),
    ).toEqual({ status: "eval_failed", view: structure, message: "x: bad" });
  });

  test("a view that could not be projected is invalid", () => {
    expect(projectionOf({ status: "projection_failed", message: "broken" })).toEqual({
      status: "invalid",
      message: "broken",
    });
  });
});

describe("acceptEdit", () => {
  const library = viewModel([viewNode("span")]);

  test("accepts an edit that leaves a library waiting for its includers' values", () => {
    expect(
      acceptEdit([
        {
          file: "src/lib.gcl",
          projection: { status: "needs_values", view: library, message: "required param" },
        },
      ]),
    ).toEqual({
      ok: true,
      models: [library],
      note: "src/lib.gcl has no values of its own: required param",
    });
  });

  test("refuses an edit after which the sources no longer check", () => {
    expect(
      acceptEdit([
        { file: "src/m.gcl", projection: { status: "evaluated", view: library } },
        { file: "src/n.gcl", projection: { status: "invalid", message: "type mismatch" } },
      ]),
    ).toEqual({ ok: false, message: "type mismatch" });
  });

  test("an evaluated module says nothing", () => {
    expect(projectionNote("src/m.gcl", { status: "evaluated", view: library })).toBeNull();
  });
});

import { readFile } from "node:fs/promises";
import { beforeAll, expect, it } from "vite-plus/test";
import catalog from "../examples/catalog.json";
import { evaluationRequest } from "./document";
import { outcomeSchema } from "./protocol";

let evaluate: (request: ReturnType<typeof evaluationRequest>) => unknown;
beforeAll(async () => {
  const moduleUrl = new URL("../public/pkg/graphcal_wasm.js", import.meta.url).href;
  const engine = await import(/* @vite-ignore */ moduleUrl);
  const bytes = await readFile(new URL("../public/pkg/graphcal_wasm_bg.wasm", import.meta.url));
  await engine.default({ module_or_path: new Uint8Array(bytes).buffer });
  evaluate = engine.evaluateProject;
});
it.each(catalog)(
  "evaluates $id with real Wasm and validates its presentation contract",
  async (example) => {
    const source = await readFile(
      new URL(`../../../${example.source_path}`, import.meta.url),
      "utf8",
    );
    const result = outcomeSchema.parse(
      evaluate(evaluationRequest({ filename: example.filename, source })),
    );
    expect(result.status).toBe("evaluated");
    if (result.status !== "evaluated") throw new Error(JSON.stringify(result));
    expect(result.evaluation.has_errors).toBe(false);
    expect(result.evaluation.notices).toEqual([]);
    expect(result.evaluation.figures).toHaveLength(example.expected_figures);
    for (const name of example.expected_values)
      expect(result.evaluation.values.some((value) => value.name === name)).toBe(true);
    expect(
      result.evaluation.assertions.every((assertion) => assertion.outcome.status === "pass"),
    ).toBe(true);
  },
);
it.each([
  ["node bad: Length = 1.0 s;", "compile_error"],
  [
    'import plugin "graphcal:demo" as demo { fn f(x: Dimensionless) -> Dimensionless; }',
    "rejected",
  ],
  [
    "param x: Dimensionless = 0.0; node y: Dimensionless = 1.0 / @x; node z: Dimensionless = @y; assert p = true; assert f = false; assert e = @y == 0.0;",
    "evaluated",
  ],
  [
    'index Axis = { A, B }; type Pair { Pair(left: Dimensionless, right: Bool), } node a: Length = 3.0 m; node b: Bool = true; node c: Int = 3; node d: Key<Axis> = Axis#A; node e: Complex<Length> = complex(3.0 m, 4.0 m); node f: Pair = Pair(left: 1.0, right: true); node g: Datetime = datetime("2024-11-05T12:00:00Z");',
    "evaluated",
  ],
])("checks Rust transport variants: %s", (source, status) => {
  expect(
    outcomeSchema.parse(evaluate(evaluationRequest({ filename: "main.gcl", source }))).status,
  ).toBe(status);
});

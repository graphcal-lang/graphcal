import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { z } from "zod";
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
it("ships the current workspace compiler version, not a stale cached Wasm artifact", () => {
  const metadata = z
    .object({ packages: z.array(z.object({ name: z.string(), version: z.string() })) })
    .parse(
      JSON.parse(
        execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
          cwd: new URL("../../../", import.meta.url),
          encoding: "utf8",
        }),
      ),
    );
  const version = metadata.packages.find((entry) => entry.name === "graphcal-wasm")?.version;
  expect(version).toBeDefined();
  const result = outcomeSchema.parse(
    evaluate(evaluationRequest({ filename: "main.gcl", source: "" })),
  );
  if (result.status !== "evaluated") throw new Error("Empty document did not evaluate");
  expect(result.evaluation.compiler_version).toBe(version);
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
it("retains the shared report projection for multidimensional tables", async () => {
  const source = await readFile(
    new URL("../../../tests/fixtures/valid/table_literal.gcl", import.meta.url),
    "utf8",
  );
  const result = outcomeSchema.parse(
    evaluate(evaluationRequest({ filename: "table_literal.gcl", source })),
  );
  if (result.status !== "evaluated") throw new Error(JSON.stringify(result));
  const matrix = result.evaluation.values.find((value) => value.name === "spacecraft_mass");
  expect(matrix?.outcome.status === "value" && matrix.outcome.body.kind).toBe("grid");
  const cube = result.evaluation.values.find((value) => value.name === "mass_3d");
  if (cube?.outcome.status !== "value" || cube.outcome.body.kind !== "slices") {
    throw new Error("mass_3d had no table-slice projection");
  }
  expect(cube.outcome.body.body.map(([label]) => label)).toEqual(["Nominal", "Contingency"]);
});

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

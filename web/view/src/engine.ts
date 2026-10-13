// The Graphcal engine in the browser: the same WASM bundle the playground
// uses (built by `wasm-pack` into public/pkg, see package.json "engine").
// One evaluation = prepare the whole project from in-memory sources, run it,
// free it. Projects here are small, so this is milliseconds.
import { z } from "zod";

export interface ProjectFile {
  path: string;
  content: string;
}

export type EngineValue =
  | { kind: "quantity"; display: string; value: number; si_value: number; unit?: string | null }
  | { kind: "bool"; display: string; value: boolean }
  | { kind: "int"; display: string; decimal: string }
  | { kind: "label"; display: string; index: string; variant: string }
  | { kind: "complex" | "datetime"; display: string }
  | { kind: "struct"; display: string }
  | {
      kind: "indexed";
      display: string;
      index: string;
      entries: { display_key: string; value: EngineValue }[];
    };

const engineValueSchema: z.ZodType<EngineValue> = z.lazy(() =>
  z.discriminatedUnion("kind", [
    z.object({
      kind: z.literal("quantity"),
      display: z.string(),
      value: z.number(),
      si_value: z.number(),
      unit: z.string().nullish(),
    }),
    z.object({ kind: z.literal("bool"), display: z.string(), value: z.boolean() }),
    z.object({ kind: z.literal("int"), display: z.string(), decimal: z.string() }),
    z.object({
      kind: z.literal("label"),
      display: z.string(),
      index: z.string(),
      variant: z.string(),
    }),
    z.object({ kind: z.enum(["complex", "datetime"]), display: z.string() }),
    z.object({ kind: z.literal("struct"), display: z.string() }),
    z.object({
      kind: z.literal("indexed"),
      display: z.string(),
      index: z.string(),
      entries: z.array(z.object({ display_key: z.string(), value: engineValueSchema })),
    }),
  ]),
);

const unavailableSchema = z.object({ kind: z.string(), message: z.string().optional() });

export const evaluationSchema = z.object({
  values: z.array(
    z.object({
      name: z.string(),
      declaration_kind: z.enum(["const", "param", "node"]),
      outcome: z.discriminatedUnion("status", [
        z.object({ status: z.literal("value"), value: engineValueSchema }),
        z.object({ status: z.literal("error"), error: unavailableSchema }),
        z.object({ status: z.literal("incomplete"), reason: unavailableSchema }),
      ]),
    }),
  ),
  assertions: z.array(
    z.object({
      name: z.string(),
      outcome: z.discriminatedUnion("status", [
        z.object({ status: z.literal("pass") }),
        z.object({ status: z.enum(["fail", "error", "blocked"]), message: z.string() }),
      ]),
    }),
  ),
});
export type Evaluation = z.infer<typeof evaluationSchema>;

const diagnosticSchema = z.object({
  message: z.string(),
  range: z
    .object({ start: z.object({ line: z.number(), column: z.number().optional() }) })
    .optional(),
  path: z.string().nullish(),
});

export const outcomeSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("rejected"), error: z.object({ message: z.string() }) }),
  z.object({ status: z.literal("compile_error"), diagnostics: z.array(diagnosticSchema) }),
  z.object({ status: z.literal("evaluated"), evaluation: evaluationSchema }),
]);
export type Outcome = z.infer<typeof outcomeSchema>;

interface Prepared {
  evaluateReport(bindings: unknown[]): unknown;
  free(): void;
}
interface Engine {
  default(options: { module_or_path: ArrayBuffer }): Promise<unknown>;
  prepareProject(request: { entry: string; files: ProjectFile[] }): Prepared;
}

let loading: Promise<Engine> | null = null;

/** Load the WASM bundle once; a missing bundle rejects with a readable hint. */
export function loadEngine(): Promise<Engine> {
  loading ??= (async () => {
    const base = import.meta.env.BASE_URL;
    const moduleUrl = new URL(`${base}pkg/graphcal_wasm.js`, location.origin).href;
    const wasmUrl = new URL(`${base}pkg/graphcal_wasm_bg.wasm`, location.origin).href;
    let engine: Engine;
    try {
      engine = (await import(/* @vite-ignore */ moduleUrl)) as Engine;
    } catch {
      throw new Error("engine not built: run `pnpm run engine` in web/view");
    }
    const response = await fetch(wasmUrl, { signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`engine download failed: HTTP ${response.status}`);
    await engine.default({ module_or_path: await response.arrayBuffer() });
    return engine;
  })();
  return loading;
}

/** Evaluate a whole project from in-memory sources. */
export async function evaluateProject(entry: string, files: ProjectFile[]): Promise<Outcome> {
  const engine = await loadEngine();
  let prepared: Prepared;
  try {
    prepared = engine.prepareProject({ entry, files });
  } catch (error) {
    const failure = outcomeSchema.safeParse(error);
    if (failure.success) return failure.data;
    throw error instanceof Error ? error : new Error(String(error));
  }
  try {
    return outcomeSchema.parse(prepared.evaluateReport([]));
  } finally {
    prepared.free();
  }
}

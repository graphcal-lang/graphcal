// The Graphcal engine in the browser: the same WASM bundle the playground
// uses (built by `wasm-pack` into public/pkg, see package.json "engine").
// One projection = prepare the whole project from in-memory sources, evaluate
// it, free it. Projects here are small, so this is milliseconds.
import { z } from "zod";
import { sharedUntilRejected } from "./memo.ts";
import { projectionOf, type ViewProjection } from "./projection.ts";

export interface ProjectFile {
  path: string;
  content: string;
}

const diagnosticSchema = z.object({
  message: z.string(),
  range: z
    .object({ start: z.object({ line: z.number(), column: z.number().optional() }) })
    .optional(),
  path: z.string().nullish(),
});

/** Why a project could not be prepared, as `prepareViewProject` throws it. */
const prepareFailureSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("rejected"), error: z.object({ message: z.string() }) }),
  z.object({ status: z.literal("compile_error"), diagnostics: z.array(diagnosticSchema) }),
]);
type PrepareFailure = z.infer<typeof prepareFailureSchema>;

interface Prepared {
  evaluateView(bindings: unknown[], target: EngineTarget): unknown;
  free(): void;
}
interface Engine {
  default(options: { module_or_path: ArrayBuffer }): Promise<unknown>;
  /** `prepareProject` that also projects the entry file's view (the playground skips it). */
  prepareViewProject(request: { entry: string; files: ProjectFile[] }): Prepared;
  /** The request limits the engine validates against. */
  projectLimits(): unknown;
  /** The package name and source directory a `graphcal.toml` declares. */
  packageLayout(manifest: string): unknown;
}

const projectLimitsSchema = z.object({
  max_files: z.number(),
  max_file_bytes: z.number(),
  max_content_bytes: z.number(),
  max_path_bytes: z.number(),
  max_path_total_bytes: z.number(),
});

/** What one request to the engine may hold (`crates/graphcal-wasm/src/project.rs`). */
export interface ProjectLimits {
  maxFiles: number;
  maxFileBytes: number;
  maxContentBytes: number;
  maxPathBytes: number;
  /** The entry path and every file path together. */
  maxPathTotalBytes: number;
}

/** The engine's request limits. */
export async function projectLimits(): Promise<ProjectLimits> {
  const limits = projectLimitsSchema.parse((await loadEngine()).projectLimits());
  return {
    maxFiles: limits.max_files,
    maxFileBytes: limits.max_file_bytes,
    maxContentBytes: limits.max_content_bytes,
    maxPathBytes: limits.max_path_bytes,
    maxPathTotalBytes: limits.max_path_total_bytes,
  };
}

const packageLayoutSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("package"), name: z.string(), source_dir: z.string() }),
  z.object({ status: z.literal("invalid"), message: z.string() }),
]);

/** Where a package keeps its modules: `<sourceDir>/<name>.gcl` and under `<sourceDir>/<name>/`. */
export interface PackageLayout {
  name: string;
  /** Relative to the package root, `/`-separated. */
  sourceDir: string;
}

/** The layout a manifest declares, as the loader reads it, or why it is invalid. */
export async function packageLayout(
  manifest: string,
): Promise<{ layout: PackageLayout } | { error: string }> {
  const layout = packageLayoutSchema.parse((await loadEngine()).packageLayout(manifest));
  return layout.status === "package"
    ? { layout: { name: layout.name, sourceDir: layout.source_dir } }
    : { error: layout.message };
}

/**
 * Load the WASM bundle; it is loaded once, and a failed load is tried again
 * on the next call. A failure says what the reader can do about it.
 */
export const loadEngine: () => Promise<Engine> = sharedUntilRejected(async () => {
  const base = import.meta.env.BASE_URL;
  const moduleUrl = new URL(`${base}pkg/graphcal_wasm.js`, location.origin).href;
  const wasmUrl = new URL(`${base}pkg/graphcal_wasm_bg.wasm`, location.origin).href;
  const unreachable = "could not load the Graphcal engine; check the connection and try again";
  let engine: Engine;
  let response: Response;
  try {
    engine = (await import(/* @vite-ignore */ moduleUrl)) as Engine;
    response = await fetch(wasmUrl, { signal: AbortSignal.timeout(30_000) });
  } catch {
    throw new Error(unreachable);
  }
  if (!response.ok) {
    throw new Error(
      `could not download the Graphcal engine (HTTP ${response.status}); try again later`,
    );
  }
  await engine.default({ module_or_path: await response.arrayBuffer() });
  return engine;
});

/** Why a project could not be prepared, in one line. */
function prepareFailure(failure: PrepareFailure): string {
  if (failure.status === "rejected") return failure.error.message;
  const first = failure.diagnostics[0];
  if (!first) return "compile error";
  const where = first.path ? ` (${first.path}:${first.range?.start.line ?? "?"})` : "";
  return `${first.message}${where}`;
}

/**
 * What the engine projects of an entry file: its own view, or that of one
 * include instance of its evaluation (by the instance's opaque id).
 */
export type EngineTarget = { kind: "module" } | { kind: "instance"; instance: string };

/** Prepare a project from in-memory sources and project `target` of its file `entry`. */
export async function projectView(
  entry: string,
  files: ProjectFile[],
  target: EngineTarget = { kind: "module" },
): Promise<ViewProjection> {
  const engine = await loadEngine();
  let prepared: Prepared;
  try {
    prepared = engine.prepareViewProject({ entry, files });
  } catch (error) {
    const failure = prepareFailureSchema.safeParse(error);
    if (failure.success) return { status: "invalid", message: prepareFailure(failure.data) };
    throw error instanceof Error ? error : new Error(String(error));
  }
  try {
    return projectionOf(prepared.evaluateView([], target));
  } finally {
    prepared.free();
  }
}

// How far a project file got in the engine, from its answer to
// `evaluateView`: statically valid or not, and if so evaluated, waiting for
// values a library's includers give it, or failed while evaluating. Pure:
// engine.ts asks the engine, the hosts decide with this what to keep.
import { z } from "zod";
import { viewModelSchema, type ViewModel } from "./model.ts";

/** `evaluateView`: the entry file's view, with values or why it has none. */
const viewOutcomeSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("evaluated"), view: viewModelSchema }),
  /** A required param has no value: the structure alone. */
  z.object({ status: z.literal("needs_values"), view: viewModelSchema, message: z.string() }),
  z.object({
    status: z.literal("binding_errors"),
    view: viewModelSchema,
    errors: z.array(z.object({ name: z.string(), message: z.string() })),
  }),
  z.object({ status: z.literal("eval_error"), view: viewModelSchema, message: z.string() }),
  /** Prepared without the view (a playground preparation). */
  z.object({ status: z.literal("not_requested") }),
  /** The checked program could not be projected (a compiler invariant failed). */
  z.object({ status: z.literal("projection_failed"), message: z.string() }),
]);

/**
 * A project file's view, by how far the project got. Only a project that
 * does not check is `invalid`; one that checks has its view, valued when it
 * evaluated, else its structure and why it has no values.
 */
export type ViewProjection =
  | { status: "invalid"; message: string }
  | { status: "evaluated"; view: ViewModel }
  /** A required param has no value: a library whose includers bind it. It can be given values. */
  | { status: "needs_values"; view: ViewModel; message: string }
  /** Evaluating it failed. */
  | { status: "eval_failed"; view: ViewModel; message: string };

/** The view of `entry` from the engine's answer to `evaluateView`. */
export function projectionOf(raw: unknown): ViewProjection {
  const outcome = viewOutcomeSchema.parse(raw);
  switch (outcome.status) {
    case "evaluated":
      return { status: "evaluated", view: outcome.view };
    case "needs_values":
      return { status: "needs_values", view: outcome.view, message: outcome.message };
    case "binding_errors":
      return {
        status: "eval_failed",
        view: outcome.view,
        message: outcome.errors.map((e) => `${e.name}: ${e.message}`).join("; "),
      };
    case "eval_error":
      return { status: "eval_failed", view: outcome.view, message: outcome.message };
    case "projection_failed":
      return { status: "invalid", message: outcome.message };
    case "not_requested":
      return { status: "invalid", message: "the project was prepared without its view" };
  }
}

/** What to say about a projection that has no values: a library, or a failure. */
export function projectionNote(file: string, projection: ViewProjection): string | null {
  switch (projection.status) {
    case "evaluated":
      return null;
    case "needs_values":
      return `${file} has no values of its own: ${projection.message}`;
    case "eval_failed":
      return `${file} did not evaluate: ${projection.message}`;
    case "invalid":
      return `${file}: ${projection.message}`;
  }
}

/** The models an edit leaves, or why it is refused: only a project that no longer checks is. */
export function acceptEdit(
  projections: readonly { file: string; projection: ViewProjection }[],
): { ok: true; models: ViewModel[]; note: string | null } | { ok: false; message: string } {
  const models: ViewModel[] = [];
  let note: string | null = null;
  for (const { file, projection } of projections) {
    if (projection.status === "invalid") return { ok: false, message: projection.message };
    models.push(projection.view);
    note ??= projectionNote(file, projection);
  }
  return { ok: true, models, note };
}

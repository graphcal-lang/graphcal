import { outcomeSchema } from "./protocol";

export const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;

/** Run inside the cancellable worker, before cloning results to the UI thread. */
export function boundedOutcome(raw: unknown) {
  const outcome = outcomeSchema.parse(raw);
  if (new TextEncoder().encode(JSON.stringify(outcome)).byteLength > MAX_OUTPUT_BYTES) {
    throw new Error(
      "Results exceed the 8 MiB browser display limit. Reduce the calculation or use the CLI; no values were truncated.",
    );
  }
  return outcome;
}

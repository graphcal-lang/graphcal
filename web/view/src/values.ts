import type { Value } from "./model.ts";

/** Resolve the entry of an indexed value at the given labels, one per axis. */
export function valueAt(value: Value | null, labels: readonly string[]): Value | null {
  let current: Value | null = value;
  for (const label of labels) {
    if (current === null || current.kind !== "indexed") return null;
    const entry = current.entries.find((e) => e.key === label);
    current = entry ? entry.value : null;
  }
  return current;
}

export function displayOf(value: Value | null): string {
  return value === null ? "—" : value.display;
}

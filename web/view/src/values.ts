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

/**
 * A magnitude rounded for a cell: four significant digits, no grouping. Below
 * 1e-3 those digits need more than six decimals, and from 1e6 the zeros only
 * pad them, so the value takes an exponent instead: a nonzero value never
 * reads as 0, and a large one stays short.
 */
export function shortMagnitude(magnitude: string): string {
  const x = Number(magnitude);
  if (!Number.isFinite(x)) return magnitude;
  if (x === 0) return "0";
  const rounded = Number(x.toPrecision(4));
  if (Math.abs(rounded) < 1e-3 || Math.abs(rounded) >= 1e6) return rounded.toExponential();
  return rounded.toLocaleString("en-US", { maximumFractionDigits: 6, useGrouping: false });
}

/** An expression on one line: runs of whitespace become one space. */
export function oneLine(expr: string): string {
  return expr.replace(/\s+/g, " ").trim();
}

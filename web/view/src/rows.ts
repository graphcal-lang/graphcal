// Row visibility and row-label picking for an axis: pure helpers shared by the
// renderer (which draws visible rows and "n hidden" gaps) and the shell (which
// edits the sets from clicks and the context menu).

/** Hidden row labels per axis name. */
export type Hidden = Map<string, Set<string>>;

export type RowRun =
  | { kind: "row"; label: string; /** Index among visible rows, for striping. */ visible: number }
  | { kind: "gap"; labels: string[] };

/** Visible rows in order, with each run of hidden rows collapsed into one gap. */
export function rowRuns(
  labels: readonly string[],
  hidden: ReadonlySet<string> | undefined,
): RowRun[] {
  const runs: RowRun[] = [];
  let visible = 0;
  for (const label of labels) {
    if (hidden?.has(label)) {
      const last = runs.at(-1);
      if (last?.kind === "gap") last.labels.push(label);
      else runs.push({ kind: "gap", labels: [label] });
    } else {
      runs.push({ kind: "row", label, visible });
      visible++;
    }
  }
  return runs;
}

/** Row labels picked on one axis; `anchor` is where a shift-range starts. */
export interface Picked {
  axis: string;
  labels: Set<string>;
  anchor: string;
}

export interface PickModifiers {
  /** Extend from the anchor to the clicked label (shift). */
  range: boolean;
  /** Add or remove the clicked label alone (cmd / ctrl). */
  toggle: boolean;
}

/** Spreadsheet-style picking: click, shift-click for a range, cmd-click to toggle. */
export function pick(
  current: Picked | null,
  axis: string,
  order: readonly string[],
  label: string,
  modifiers: PickModifiers,
): Picked {
  const same = current?.axis === axis ? current : null;
  if (modifiers.range && same) {
    const a = order.indexOf(same.anchor);
    const b = order.indexOf(label);
    const [lo, hi] = a < b ? [a, b] : [b, a];
    const labels = modifiers.toggle ? new Set(same.labels) : new Set<string>();
    for (const l of order.slice(lo, hi + 1)) labels.add(l);
    return { axis, labels, anchor: same.anchor };
  }
  if (modifiers.toggle && same) {
    const labels = new Set(same.labels);
    if (labels.has(label)) labels.delete(label);
    else labels.add(label);
    return { axis, labels, anchor: label };
  }
  return { axis, labels: new Set([label]), anchor: label };
}

export function hiddenOf(hidden: Hidden, axis: string): Set<string> {
  const set = hidden.get(axis) ?? new Set<string>();
  hidden.set(axis, set);
  return set;
}

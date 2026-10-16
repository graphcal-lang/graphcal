// Planning the sheet's edits: what a cell or an expression typed by the
// reader becomes in the source. Pure: the page hands the plan to its host.
import type { SourceEdit } from "./host.ts";
import type { Range, ViewNode } from "./model.ts";
import { EditError, replaceRange } from "./source.ts";
import { errorMessage } from "./errors.ts";
import { oneLine, valueAt } from "./values.ts";

/** An edit to hand to the host, or why there is none. */
export type EditPlan = { ok: true; edit: SourceEdit } | { ok: false; message: string };

function refused(message: string): EditPlan {
  return { ok: false, message };
}

/**
 * Turn typed text into a literal for a cell of the node. A quantity needs a
 * floating-point literal (`0` is an integer; `0.0` is not), so a bare number is
 * given a decimal point and the unit the cell is shown in; anything else (an
 * `Int`, a `Bool`, a unit typed out) is passed through as written.
 */
export function literalFor(node: ViewNode, unit: string | null, text: string): string {
  const trimmed = text.trim();
  if (node.element !== "quantity") return trimmed;
  const match = /^(-?)(\d[\d_]*)?(?:\.(\d[\d_]*))?([eE][-+]?\d+)?$/.exec(trimmed);
  if (!match || (match[2] === undefined && match[3] === undefined)) return trimmed;
  const [, sign, whole = "0", fraction = "0", exponent = ""] = match;
  const number = `${sign}${whole}.${fraction}${exponent}`;
  return unit ? `${number} ${unit}` : number;
}

/** The unit a cell of the node is shown in: its own value's, else the column's. */
export function cellUnit(node: ViewNode, keys: readonly string[]): string | null {
  const value = valueAt(node.value, keys);
  return value?.kind === "quantity" ? value.unit : node.unit;
}

/** The expression as edited in the formula bar: the source text on one line, when its file is at hand. */
export function editableExpr(node: ViewNode, source: string | null): string | null {
  if (node.expr === null || node.source === null || node.source.file === null) return null;
  if (source === null) return null;
  return oneLine(node.expr);
}

/** Replace the whole expression of a node (formula bar); `source` is its file's text. */
export function planExprEdit(node: ViewNode, source: string | null, next: string): EditPlan {
  const file = node.source?.file ?? null;
  if (editableExpr(node, source) === null || file === null || source === null) {
    return refused("this expression cannot be edited");
  }
  // Only the compiler's range says where the expression is; without one
  // (a column of a multi-declaration table) nothing is replaced.
  const range = node.source?.expr_range ?? null;
  if (range === null) {
    return refused("this expression is shared with other declarations; edit its cells");
  }
  try {
    return { ok: true, edit: { file, next: replaceRange(source, range, next) } };
  } catch (error) {
    return refused(error instanceof EditError ? error.message : errorMessage(error));
  }
}

/**
 * The range a cell of the node is written at: one the compiler reported for
 * the cell (a scalar's is its expression). A cell with none is read-only: it
 * is computed, or its table is not written entry by entry.
 */
export function cellRange(node: ViewNode, keys: readonly string[]): Range | null {
  const source = node.source;
  if (source === null) return null;
  if (keys.length === 0) return source.expr_range;
  const cell = source.cells.find(
    (c) => c.keys.length === keys.length && c.keys.every((key, i) => key === keys[i]),
  );
  return cell?.range ?? null;
}

/** Replace the literal of one cell (entry keys `keys`) with what was typed. */
export function planCellEdit(
  node: ViewNode,
  keys: readonly string[],
  source: string | null,
  text: string,
): EditPlan {
  const file = node.source?.file ?? null;
  const range = cellRange(node, keys);
  if (file === null || source === null || range === null) {
    return refused("this value cannot be edited");
  }
  const literal = literalFor(node, cellUnit(node, keys), text);
  try {
    return { ok: true, edit: { file, next: replaceRange(source, range, literal) } };
  } catch (error) {
    return refused(error instanceof EditError ? error.message : errorMessage(error));
  }
}

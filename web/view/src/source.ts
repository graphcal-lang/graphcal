// Textual edits to Graphcal sources. Pure string functions, no DOM, no Vite.

export class EditError extends Error {}

function lines(source: string): string[] {
  return source.split("\n");
}

/**
 * Replace the expression text of a declaration. `line` is the 1-based line
 * of the declaration; the old expression is the view's `expr`, which is a
 * verbatim fragment of the source at or after that line.
 */
export function replaceExpr(source: string, line: number, from: string, to: string): string {
  const rows = lines(source);
  const start = rows.slice(0, Math.max(0, line - 1)).join("\n").length;
  const at = source.indexOf(from, line > 1 ? start + 1 : 0);
  if (at < 0 || from.length === 0) {
    throw new EditError("the expression is not a verbatim fragment of the source");
  }
  return source.slice(0, at) + to + source.slice(at + from.length);
}

/** A 0-based line and UTF-16 column, as the Language Server Protocol counts. */
export interface Position {
  line: number;
  character: number;
}

/** A half-open editor range: 0-based lines and UTF-16 columns. */
export interface Range {
  start: Position;
  end: Position;
}

function offsetOf(source: string, position: Position): number {
  const rows = lines(source);
  if (position.line >= rows.length) return source.length;
  const start = rows.slice(0, position.line).join("\n").length + (position.line > 0 ? 1 : 0);
  return Math.min(start + position.character, source.length);
}

/** Replace the text between two positions, as the compiler reports an expression's range. */
export function replaceRange(source: string, range: Range, to: string): string {
  const from = offsetOf(source, range.start);
  const until = offsetOf(source, range.end);
  if (until < from) throw new EditError("the expression range is inverted");
  return source.slice(0, from) + to + source.slice(until);
}

/**
 * The 0-based rows strictly inside the table literal that `range` spans: the
 * rows between its `table[…] {` line and its closing line. Only these rows are
 * searched, so an edit never reaches past the declaration's own expression.
 */
function tableRows(rows: string[], range: Range): [number, number] {
  const head = (rows[range.start.line] ?? "").slice(range.start.character);
  const axes = /^\s*table\s*\[([^\]]*)\]/.exec(head);
  if (!axes) throw new EditError("this value is not a table literal in the source");
  // A multi-declaration table's columns belong to several declarations.
  if ((axes[1] ?? "").includes("(")) {
    throw new EditError("a table shared by several declarations cannot be edited cell by cell");
  }
  if (range.end.line <= range.start.line) {
    throw new EditError("a table literal on one line cannot be edited cell by cell");
  }
  return [range.start.line + 1, range.end.line];
}

/** Split a table row body `a, b, c` keeping each item's surrounding spaces. */
function splitItems(body: string): string[] {
  return body.split(",");
}

/**
 * Replace one entry of a table literal: `row: value;` for one axis, or the
 * `sub` column of `row: v1, v2, …;` under a `: sub1, sub2, …;` header.
 * `range` is the compiler's range of the declaration's expression.
 */
export function replaceTableCell(
  source: string,
  range: Range,
  row: string,
  sub: string | null,
  to: string,
): string {
  const rows = lines(source);
  const [start, end] = tableRows(rows, range);
  const rowRe = new RegExp(`^(\\s*${row}\\s*:\\s*)(.*?)(\\s*;.*)$`);
  let header: string[] | null = null;
  for (let i = start; i < end; i += 1) {
    const text = rows[i] ?? "";
    const head = /^\s*:\s*(.*?)\s*;/.exec(text);
    if (head) {
      header = splitItems(head[1] ?? "").map((s) => s.trim());
      continue;
    }
    const m = rowRe.exec(text);
    if (!m) continue;
    const [, before, body, after] = m;
    let next: string;
    if (sub === null) {
      next = to;
    } else {
      if (!header) throw new EditError("the table has no column header line");
      const col = header.indexOf(sub);
      if (col < 0) throw new EditError(`column ${sub} is not in the table header`);
      const items = splitItems(body ?? "");
      if (col >= items.length) throw new EditError(`row ${row} has no column ${sub}`);
      const item = items[col] ?? "";
      const lead = /^\s*/.exec(item)?.[0] ?? "";
      items[col] = `${lead}${to}`;
      next = items.join(",");
    }
    rows[i] = `${before ?? ""}${next}${after ?? ""}`;
    return rows.join("\n");
  }
  throw new EditError(`row ${row} is not in the table literal`);
}

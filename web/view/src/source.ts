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

function offsetOf(source: string, position: Position): number {
  const rows = lines(source);
  if (position.line >= rows.length) return source.length;
  const start = rows.slice(0, position.line).join("\n").length + (position.line > 0 ? 1 : 0);
  return Math.min(start + position.character, source.length);
}

/** Replace the text between two positions, as the compiler reports an expression's range. */
export function replaceRange(
  source: string,
  range: { start: Position; end: Position },
  to: string,
): string {
  const from = offsetOf(source, range.start);
  const until = offsetOf(source, range.end);
  if (until < from) throw new EditError("the expression range is inverted");
  return source.slice(0, from) + to + source.slice(until);
}

/** The `[start, end]` 0-based row range of the `table … { … };` starting at `line`. */
function tableBlock(rows: string[], line: number): [number, number] {
  const start = line - 1;
  if (!/\btable\s*\[/.test(rows[start] ?? "")) {
    throw new EditError("this value is not a table literal in the source");
  }
  for (let i = start; i < rows.length; i += 1) {
    if (/^\s*}\s*;?\s*$/.test(rows[i] ?? "")) return [start, i];
  }
  throw new EditError("the table literal has no closing brace");
}

/** Split a table row body `a, b, c` keeping each item's surrounding spaces. */
function splitItems(body: string): string[] {
  return body.split(",");
}

/**
 * Replace one entry of a table literal: `row: value;` for one axis, or the
 * `sub` column of `row: v1, v2, …;` under a `: sub1, sub2, …;` header.
 */
export function replaceTableCell(
  source: string,
  line: number,
  row: string,
  sub: string | null,
  to: string,
): string {
  const rows = lines(source);
  const [start, end] = tableBlock(rows, line);
  const rowRe = new RegExp(`^(\\s*${row}\\s*:\\s*)(.*?)(\\s*;.*)$`);
  let header: string[] | null = null;
  for (let i = start + 1; i < end; i += 1) {
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

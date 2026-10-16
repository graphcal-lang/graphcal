// Textual edits to Graphcal sources: replacing a range the compiler reported.
// Pure string functions, no DOM, no Vite.
import type { Position, Range } from "./model.ts";

export class EditError extends Error {}

/**
 * The UTF-16 offset of a 0-based line and column, as the Language Server
 * Protocol counts them; lines end at `\n` (a `\r` before it is part of the
 * line). A position past the end of its line, or past the last line, is not
 * in the text.
 */
export function offsetOf(source: string, position: Position): number {
  let start = 0;
  for (let line = 0; line < position.line; line += 1) {
    const end = source.indexOf("\n", start);
    if (end < 0) throw new EditError("the range is outside the file");
    start = end + 1;
  }
  const end = source.indexOf("\n", start);
  const length = (end < 0 ? source.length : end) - start;
  if (position.character > length) throw new EditError("the range is outside the file");
  return start + position.character;
}

/** Replace the text between two positions, as the compiler reports an expression's range. */
export function replaceRange(source: string, range: Range, to: string): string {
  const from = offsetOf(source, range.start);
  const until = offsetOf(source, range.end);
  if (until < from) throw new EditError("the range is inverted");
  return source.slice(0, from) + to + source.slice(until);
}

/** The smallest replacement turning `before` into `after`: shared prefix and suffix trimmed. */
export function minimalEdit(
  before: string,
  after: string,
): { start: number; end: number; text: string } {
  let start = 0;
  const max = Math.min(before.length, after.length);
  while (start < max && before[start] === after[start]) start += 1;
  let endBefore = before.length;
  let endAfter = after.length;
  while (endBefore > start && endAfter > start && before[endBefore - 1] === after[endAfter - 1]) {
    endBefore -= 1;
    endAfter -= 1;
  }
  return { start, end: endBefore, text: after.slice(start, endAfter) };
}

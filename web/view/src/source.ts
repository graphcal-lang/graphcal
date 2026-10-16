// Positions in Graphcal sources, as the compiler reports ranges, and the
// smallest change between two texts. Pure string functions, no DOM, no Vite.
import type { Position } from "./model.ts";

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

/** The line and UTF-16 column of an offset, as `offsetOf` counts them. */
export function positionOf(source: string, offset: number): Position {
  const before = source.slice(0, offset);
  const lineStart = before.lastIndexOf("\n") + 1;
  let line = 0;
  for (let at = before.indexOf("\n"); at >= 0; at = before.indexOf("\n", at + 1)) line += 1;
  return { line, character: offset - lineStart };
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

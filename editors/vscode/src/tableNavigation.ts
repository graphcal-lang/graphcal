// Pure logic for table cell navigation.
// No VS Code API dependency — operates on plain text and offsets.

export interface TableRange {
  /** Offset of the opening `{` of the table body. */
  bodyStart: number;
  /** Offset of the closing `}` of the table body. */
  bodyEnd: number;
}

/**
 * Find the enclosing `table[...] { ... }` block for the given offset.
 * Returns the range of the table body (between `{` and `}`), or null
 * if the cursor is not inside a table body.
 */
export function findEnclosingTable(
  text: string,
  offset: number,
): TableRange | null {
  // Scan backward from offset to find `table[` keyword.
  // We limit the backward scan to avoid scanning the entire file.
  const maxBackwardScan = 20_000; // ~500 lines of 40 chars
  const scanStart = Math.max(0, offset - maxBackwardScan);
  const prefix = text.slice(scanStart, offset);

  // Find the last `table[` before the cursor.
  const tablePattern = /table\s*\[/g;
  let lastMatch: RegExpExecArray | null = null;
  let m: RegExpExecArray | null;
  while ((m = tablePattern.exec(prefix)) !== null) {
    lastMatch = m;
  }
  if (!lastMatch) {
    return null;
  }

  const tableKeywordOffset = scanStart + lastMatch.index;

  // From the `table[` position, find the matching `]` then the opening `{`.
  let i = tableKeywordOffset + lastMatch[0].length; // just past `[`
  let bracketDepth = 1;
  // Find matching `]`
  while (i < text.length && bracketDepth > 0) {
    if (text[i] === "[") bracketDepth++;
    else if (text[i] === "]") bracketDepth--;
    i++;
  }
  // Now find the opening `{`
  while (i < text.length && text[i] !== "{") {
    if (!isWhitespace(text[i])) {
      // Unexpected character between `]` and `{`
      return null;
    }
    i++;
  }
  if (i >= text.length || text[i] !== "{") {
    return null;
  }
  const bodyStart = i;

  // Find matching `}` tracking brace depth.
  let braceDepth = 1;
  i++;
  while (i < text.length && braceDepth > 0) {
    const ch = text[i];
    if (ch === "/" && i + 1 < text.length && text[i + 1] === "/") {
      // Skip line comment
      i = skipToEndOfLine(text, i);
      continue;
    }
    if (ch === '"') {
      i = skipString(text, i);
      continue;
    }
    if (ch === "{") braceDepth++;
    else if (ch === "}") braceDepth--;
    i++;
  }
  if (braceDepth !== 0) {
    return null;
  }
  const bodyEnd = i - 1; // offset of the `}`

  // Check that cursor is inside the body.
  if (offset <= bodyStart || offset >= bodyEnd) {
    return null;
  }

  return { bodyStart, bodyEnd };
}

/**
 * Find the start offset of the next cell value from the given offset.
 * Returns null if at the end of the table.
 *
 * "Next cell" means: scan forward for the next `,` or `;` separator at
 * bracket-depth 0, then skip to the start of the next value content.
 */
export function findNextCell(
  text: string,
  offset: number,
  table: TableRange,
): number | null {
  let i = offset;
  let depth = 0; // bracket depth relative to table body

  while (i < table.bodyEnd) {
    const ch = text[i];

    // Skip line comments
    if (ch === "/" && i + 1 < table.bodyEnd && text[i + 1] === "/") {
      i = skipToEndOfLine(text, i);
      continue;
    }
    // Skip string literals
    if (ch === '"') {
      i = skipString(text, i);
      continue;
    }

    if (ch === "(" || ch === "[" || ch === "{") {
      depth++;
      i++;
      continue;
    }
    if (ch === ")" || ch === "]" || ch === "}") {
      if (depth === 0) {
        // We've hit the closing `}` of the table body
        return null;
      }
      depth--;
      i++;
      continue;
    }

    if (depth === 0 && ch === ",") {
      // Next cell is after this comma
      return skipToNextValue(text, i + 1, table.bodyEnd);
    }

    if (depth === 0 && ch === ":" && text[i + 1] !== ":") {
      // Row label separator (single `:`, not `::`) — jump to first cell value
      return skipToNextValue(text, i + 1, table.bodyEnd);
    }

    if (depth === 0 && ch === ";") {
      // End of row — jump to the next row's first cell value
      return skipToNextRowValue(text, i + 1, table.bodyEnd);
    }

    i++;
  }

  return null;
}

/**
 * Find the start offset of the previous cell value from the given offset.
 * Returns null if at the beginning of the table.
 */
export function findPreviousCell(
  text: string,
  offset: number,
  table: TableRange,
): number | null {
  // Strategy: collect all cell start positions within the table,
  // then find the one before the current cell.
  const cells = collectCellStarts(text, table);
  if (cells.length === 0) {
    return null;
  }

  // Find which cell the cursor is currently in or after.
  // We want the cell start that is strictly before the current one.
  // First, find the index of the current cell (the last cell start <= offset).
  let currentIdx = -1;
  for (let idx = 0; idx < cells.length; idx++) {
    if (cells[idx] <= offset) {
      currentIdx = idx;
    } else {
      break;
    }
  }

  // If the cursor is exactly at a cell start, go to the previous cell.
  // If the cursor is inside a cell (past its start), go to that cell's start.
  if (currentIdx < 0) {
    return null; // cursor is before the first cell
  }

  if (cells[currentIdx] === offset) {
    // Cursor is at the start of this cell — go to previous
    return currentIdx > 0 ? cells[currentIdx - 1] : null;
  }

  // Cursor is inside the cell — go to this cell's start
  return cells[currentIdx];
}

/**
 * Quick check: is the cursor inside a table body?
 * Used for the context key — called on every cursor move, must be fast.
 */
export function isInsideTable(text: string, offset: number): boolean {
  return findEnclosingTable(text, offset) !== null;
}

// --- Internal helpers ---

/**
 * Collect the start offsets of all cell values in a table.
 * A "cell value" starts after `:` (row label), after `,` (next column),
 * or at the start of a header column name.
 */
function collectCellStarts(text: string, table: TableRange): number[] {
  const cells: number[] = [];
  let i = table.bodyStart + 1; // skip `{`
  let depth = 0;

  // Skip initial whitespace to find the first content
  i = skipWhitespaceAndNewlines(text, i, table.bodyEnd);

  // Determine if this is a slice section header `[...]`
  // or a header row or a 1D data row.
  // We process the whole body sequentially.
  while (i < table.bodyEnd) {
    const ch = text[i];

    // Skip line comments
    if (ch === "/" && i + 1 < table.bodyEnd && text[i + 1] === "/") {
      i = skipToEndOfLine(text, i);
      i = skipWhitespaceAndNewlines(text, i, table.bodyEnd);
      continue;
    }

    // Skip string literals
    if (ch === '"') {
      i = skipString(text, i);
      continue;
    }

    if (ch === "(" || ch === "[" || ch === "{") {
      depth++;
      i++;
      continue;
    }
    if (ch === ")" || ch === "]" || ch === "}") {
      if (depth === 0) break; // end of table body
      depth--;
      i++;
      continue;
    }

    if (depth === 0 && ch === ":" && text[i + 1] !== ":") {
      // After row label (single `:`, not `::`) — first cell of this row
      const cellStart = skipToNextValue(text, i + 1, table.bodyEnd);
      if (cellStart !== null) {
        cells.push(cellStart);
      }
      i++;
      continue;
    }

    if (depth === 0 && ch === ",") {
      // After comma — next cell in this row
      const cellStart = skipToNextValue(text, i + 1, table.bodyEnd);
      if (cellStart !== null) {
        cells.push(cellStart);
      }
      i++;
      continue;
    }

    if (depth === 0 && ch === ";") {
      // End of row
      i++;
      continue;
    }

    // If we're at the start of a line and this looks like a header column
    // (identifier at depth 0, not followed by `:` before `,` or `;`)
    // This handles the first column in a header row.
    // We detect this by checking if we're at the start of non-whitespace
    // content after a newline (or at body start).
    if (depth === 0 && isStartOfLineContent(text, i, table.bodyStart)) {
      // Check if this is a slice section header `[...]`
      // Those are handled by bracket depth tracking above.
      // Check if this looks like a row label (identifier followed by `:`)
      const colonPos = findNextSeparator(text, i, table.bodyEnd);
      if (colonPos !== null && text[colonPos] === ":") {
        // This is a row label — the cell starts after `:`
        // Don't add the label as a cell, it will be added when we hit `:`
      } else {
        // This is a header column or the first value — add it
        cells.push(i);
      }
    }

    i++;
  }

  return cells;
}

/**
 * Check if offset `i` is at the start of line content (first non-whitespace
 * character of a line, or first content after the table body opening).
 */
function isStartOfLineContent(
  text: string,
  i: number,
  bodyStart: number,
): boolean {
  if (i <= bodyStart + 1) return true;
  // Walk backward to see if only whitespace precedes on this line
  let j = i - 1;
  while (j > bodyStart) {
    if (text[j] === "\n") return true;
    if (!isWhitespace(text[j])) return false;
    j--;
  }
  return true;
}

/**
 * From offset `i`, find the next `,`, `;`, or `:` at bracket depth 0.
 * Returns the offset of the separator, or null if not found before bodyEnd.
 */
function findNextSeparator(
  text: string,
  start: number,
  bodyEnd: number,
): number | null {
  let depth = 0;
  for (let i = start; i < bodyEnd; i++) {
    const ch = text[i];
    if (ch === "/" && i + 1 < bodyEnd && text[i + 1] === "/") {
      const eol = skipToEndOfLine(text, i);
      i = eol - 1;
      continue;
    }
    if (ch === '"') {
      i = skipString(text, i) - 1;
      continue;
    }
    if (ch === "(" || ch === "[" || ch === "{") depth++;
    else if (ch === ")" || ch === "]" || ch === "}") {
      if (depth === 0) return null;
      depth--;
    } else if (depth === 0 && (ch === "," || ch === ";" || (ch === ":" && text[i + 1] !== ":"))) {
      return i;
    }
  }
  return null;
}

/**
 * Skip whitespace (not newlines) after a separator to find the start of
 * the next value. Returns the offset of the first non-whitespace character,
 * or null if we hit the end of the table body.
 */
function skipToNextValue(
  text: string,
  start: number,
  bodyEnd: number,
): number | null {
  let i = start;
  while (i < bodyEnd) {
    const ch = text[i];
    if (ch === " " || ch === "\t") {
      i++;
      continue;
    }
    if (ch === "/" && i + 1 < bodyEnd && text[i + 1] === "/") {
      i = skipToEndOfLine(text, i);
      continue;
    }
    if (ch === "}" || ch === ";") {
      return null;
    }
    return i;
  }
  return null;
}

/**
 * After a `;`, skip to the first cell value of the next row.
 * Handles: whitespace/newlines, comment lines, slice section headers `[...]`,
 * header rows (identifier columns), and data rows (label: value).
 */
function skipToNextRowValue(
  text: string,
  start: number,
  bodyEnd: number,
): number | null {
  let i = skipWhitespaceAndNewlines(text, start, bodyEnd);
  if (i >= bodyEnd) return null;

  // Skip comment lines
  while (
    i < bodyEnd &&
    text[i] === "/" &&
    i + 1 < bodyEnd &&
    text[i + 1] === "/"
  ) {
    i = skipToEndOfLine(text, i);
    i = skipWhitespaceAndNewlines(text, i, bodyEnd);
  }
  if (i >= bodyEnd) return null;

  // Check for slice section header `[...]`
  if (text[i] === "[") {
    // Skip past the `[...]` header
    let depth = 1;
    i++;
    while (i < bodyEnd && depth > 0) {
      if (text[i] === "[") depth++;
      else if (text[i] === "]") depth--;
      i++;
    }
    // Skip whitespace/newlines after slice header
    i = skipWhitespaceAndNewlines(text, i, bodyEnd);
    if (i >= bodyEnd) return null;
  }

  // Check for closing `}`
  if (text[i] === "}") return null;

  // We're at the start of the next row's content.
  // Determine if this is a data row (has `Label:` prefix) or a header row.
  // For a data row, skip past the label and `:` to reach the first cell value.
  const sep = findNextSeparator(text, i, bodyEnd);
  if (sep !== null && text[sep] === ":") {
    // Data row — skip past `Label:` and whitespace
    return skipToNextValue(text, sep + 1, bodyEnd);
  }

  // Header row or 1D row — the content here is the first column/value
  return i;
}

function skipWhitespaceAndNewlines(
  text: string,
  start: number,
  end: number,
): number {
  let i = start;
  while (i < end && (text[i] === " " || text[i] === "\t" || text[i] === "\n" || text[i] === "\r")) {
    i++;
  }
  return i;
}

function skipToEndOfLine(text: string, start: number): number {
  let i = start;
  while (i < text.length && text[i] !== "\n") {
    i++;
  }
  if (i < text.length) i++; // skip past `\n`
  return i;
}

function skipString(text: string, start: number): number {
  // start is at the opening `"`
  let i = start + 1;
  while (i < text.length && text[i] !== '"') {
    if (text[i] === "\\") i++; // skip escaped char
    i++;
  }
  if (i < text.length) i++; // skip closing `"`
  return i;
}

function isWhitespace(ch: string): boolean {
  return ch === " " || ch === "\t" || ch === "\n" || ch === "\r";
}

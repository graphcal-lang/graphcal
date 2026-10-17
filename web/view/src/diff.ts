// A unified diff of the what-if edits, to carry them elsewhere: `git apply`
// reads it, and the VS Code extension applies it from the clipboard. Pure
// text functions; the hosts supply the before and after of each file.

export interface FileChange {
  /** Repository path of the file. */
  path: string;
  before: string;
  after: string;
}

interface Op {
  kind: " " | "-" | "+";
  text: string;
}

/**
 * Lines of a text, each with its terminating newline. Only a last line can lack
 * one, so `b` and `b\n` differ: a patch must carry an added or dropped final newline.
 */
function linesOf(text: string): string[] {
  return text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/** The edit script between two line arrays: common prefix and suffix kept, the middle by LCS. */
function editScript(a: readonly string[], b: readonly string[]): Op[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  // Longest common subsequence by dynamic programming; the middles are small.
  const n = midA.length;
  const m = midB.length;
  const table: Uint32Array[] = [];
  for (let i = 0; i <= n; i += 1) table.push(new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i -= 1) {
    const row = table[i] as Uint32Array;
    const next = table[i + 1] as Uint32Array;
    for (let j = m - 1; j >= 0; j -= 1) {
      row[j] =
        midA[i] === midB[j]
          ? (next[j + 1] as number) + 1
          : Math.max(next[j] as number, row[j + 1] as number);
    }
  }
  const ops: Op[] = a.slice(0, start).map((text) => ({ kind: " ", text }));
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && midA[i] === midB[j]) {
      ops.push({ kind: " ", text: midA[i] as string });
      i += 1;
      j += 1;
    } else if (
      i < n &&
      (j >= m ||
        ((table[i + 1] as Uint32Array)[j] as number) >=
          ((table[i] as Uint32Array)[j + 1] as number))
    ) {
      // Removals before additions, as git prints them.
      ops.push({ kind: "-", text: midA[i] as string });
      i += 1;
    } else {
      ops.push({ kind: "+", text: midB[j] as string });
      j += 1;
    }
  }
  for (const text of a.slice(endA)) ops.push({ kind: " ", text });
  return ops;
}

const CONTEXT = 3;

/** The unified diff of one file, with `git diff` headers; empty when nothing changed. */
export function unifiedDiff(change: FileChange): string {
  if (change.before === change.after) return "";
  const a = linesOf(change.before);
  const b = linesOf(change.after);
  const ops = editScript(a, b);
  // Group the changes into hunks: changed ops with up to CONTEXT lines around them.
  const changed = ops.map((op) => op.kind !== " ");
  const hunks: { from: number; to: number }[] = [];
  let i = 0;
  while (i < ops.length) {
    if (!changed[i]) {
      i += 1;
      continue;
    }
    const from = Math.max(0, i - CONTEXT);
    let to = i;
    let last = i;
    while (to < ops.length && to - last <= 2 * CONTEXT) {
      if (changed[to]) last = to;
      to += 1;
    }
    to = Math.min(ops.length, last + CONTEXT + 1);
    hunks.push({ from, to });
    i = to;
  }
  const out: string[] = [
    `diff --git a/${change.path} b/${change.path}`,
    `--- a/${change.path}`,
    `+++ b/${change.path}`,
  ];
  let oldLine = 1;
  let newLine = 1;
  let at = 0;
  for (const hunk of hunks) {
    for (; at < hunk.from; at += 1) {
      const op = ops[at] as Op;
      if (op.kind !== "+") oldLine += 1;
      if (op.kind !== "-") newLine += 1;
    }
    const slice = ops.slice(hunk.from, hunk.to);
    const oldCount = slice.filter((op) => op.kind !== "+").length;
    const newCount = slice.filter((op) => op.kind !== "-").length;
    // An empty side starts before its first line, as `@@ -0,0 +1 @@` does.
    const startOf = (line: number, count: number): number => (count === 0 ? line - 1 : line);
    out.push(
      `@@ -${startOf(oldLine, oldCount)},${oldCount} +${startOf(newLine, newCount)},${newCount} @@`,
    );
    for (const op of slice) {
      const ended = op.text.endsWith("\n");
      out.push(`${op.kind}${ended ? op.text.slice(0, -1) : op.text}`);
      // The marker follows the line it is about: a file's last line, without a newline.
      if (!ended) out.push("\\ No newline at end of file");
      if (op.kind !== "+") oldLine += 1;
      if (op.kind !== "-") newLine += 1;
    }
    at = hunk.to;
  }
  return `${out.join("\n")}\n`;
}

/**
 * The files whose text differs from `before` in `after`, at the paths
 * `pathOf` gives them. A file only in `after` is not one the edits reach.
 */
export function changedFiles(
  before: ReadonlyMap<string, string>,
  after: ReadonlyMap<string, string>,
  pathOf: (file: string) => string,
): FileChange[] {
  const changes: FileChange[] = [];
  for (const [file, was] of before) {
    const now = after.get(file);
    if (now === undefined || now === was) continue;
    changes.push({ path: pathOf(file), before: was, after: now });
  }
  return changes;
}

/** A patch to copy, and how many files it changes. */
export interface Patch {
  text: string;
  files: number;
}

/** The diffs of every changed file under one heading, as a patch to copy. */
export function patchOf(changes: readonly FileChange[], heading: string): Patch {
  const diffs = changes.map(unifiedDiff).filter((d) => d !== "");
  if (diffs.length === 0) return { text: "", files: 0 };
  return { text: `# ${heading}\n\n${diffs.join("")}`, files: diffs.length };
}

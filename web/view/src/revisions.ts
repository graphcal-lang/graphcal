// A file's text by revision. A host numbers the states of its sources; a
// model, and every range in it, refers to the revision it was projected
// from. An edit names that revision and is placed on the latest text when its
// turn comes: through the changes made since, while they did not touch its
// range. Pure: the hosts keep one history per file.
import type { Range } from "./model.ts";
import { EditError, offsetOf } from "./source.ts";

/** Replace `range` of a file, as it read at `revision`, with `text`. */
export interface SourceEdit {
  file: string;
  revision: number;
  range: Range;
  text: string;
}

/** A file's text at a revision. */
export interface Revised {
  text: string;
  revision: number;
}

/** A replacement by UTF-16 offsets. */
export interface TextChange {
  from: number;
  to: number;
  insert: string;
}

/** A change and the revision it made; `before` is the text it was made on. */
interface Step {
  revision: number;
  before: string;
  change: TextChange;
}

export interface FileHistory {
  text: string;
  /** The revision `text` is at. */
  revision: number;
  /** The oldest revision an edit can be placed from; older text is not known. */
  since: number;
  /** The changes made since `since`, oldest first. */
  steps: readonly Step[];
}

/** How many changes a history keeps to place late edits through. */
const KEPT_STEPS = 64;

export function historyOf(text: string, revision: number): FileHistory {
  return { text, revision, since: revision, steps: [] };
}

/**
 * The file is `text` at `revision`, as a host received it. The same text
 * keeps what the history knows; another text starts over from it.
 */
export function received(history: FileHistory, text: string, revision: number): FileHistory {
  if (revision < history.revision) return history;
  return text === history.text ? { ...history, revision } : historyOf(text, revision);
}

export function applyChange(text: string, change: TextChange): string {
  return text.slice(0, change.from) + change.insert + text.slice(change.to);
}

/** The history after `change`, placed on the latest text, made `revision`. */
export function recorded(history: FileHistory, change: TextChange, revision: number): FileHistory {
  const steps = [...history.steps, { revision, before: history.text, change }];
  const dropped = steps.length > KEPT_STEPS ? steps.splice(0, steps.length - KEPT_STEPS) : [];
  return {
    text: applyChange(history.text, change),
    revision,
    since: dropped.at(-1)?.revision ?? history.since,
    steps,
  };
}

export type Placement = { ok: true; change: TextChange } | { ok: false; message: string };

/**
 * Where `edit` goes in the latest text: its range is read on the text of its
 * revision, then moved past every change made since. A change that touched
 * the range refuses the edit, as does a revision older than the history.
 */
export function place(history: FileHistory, edit: Omit<SourceEdit, "file">): Placement {
  // A revision after the file's last change reads the file as it is now.
  if (edit.revision < history.since) {
    return { ok: false, message: "the file changed since this was shown; edit again" };
  }
  const later = history.steps.filter((step) => step.revision > edit.revision);
  const base = later[0]?.before ?? history.text;
  let from: number;
  let to: number;
  try {
    from = offsetOf(base, edit.range.start);
    to = offsetOf(base, edit.range.end);
  } catch (error) {
    if (error instanceof EditError) return { ok: false, message: error.message };
    throw error;
  }
  if (to < from) return { ok: false, message: "the range is inverted" };
  for (const { change } of later) {
    // Wholly before the change: in place. Wholly after it: moved by its length.
    if (to <= change.from) continue;
    if (from >= change.to) {
      const delta = change.insert.length - (change.to - change.from);
      from += delta;
      to += delta;
      continue;
    }
    return { ok: false, message: "another edit changed the same text; edit again" };
  }
  return { ok: true, change: { from, to, insert: edit.text } };
}

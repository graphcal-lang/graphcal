// Bringing the host's change of a file into the code pane without losing
// what the reader typed: both started from the same text, the host's change
// is mapped over the reader's. Pure (CodeMirror's change sets need no DOM).
import { ChangeSet } from "@codemirror/state";
import { minimalEdit } from "./source.ts";

function changeFrom(base: string, to: string): ChangeSet {
  const { start, end, text } = minimalEdit(base, to);
  // `\n` alone separates lines, so a `\r` stays part of its line, as the compiler counts.
  return ChangeSet.of({ from: start, to: end, insert: text }, base.length, "\n");
}

/**
 * The change to make to `ours` so it also holds the change from `base` to
 * `theirs`. Where both changed the same place, the reader's text comes first.
 */
export function rebaseChange(base: string, ours: string, theirs: string): ChangeSet {
  return changeFrom(base, theirs).map(changeFrom(base, ours));
}

/**
 * What the code pane does when the host's text of its file is `text`: its
 * document `doc` was based on `synced`. Nothing when the host has nothing
 * new (the reader's typing stays); else the host's change, under the typing.
 */
export function adoptionOf(synced: string | null, doc: string, text: string): ChangeSet | null {
  if (doc === text || (synced !== null && synced === text)) return null;
  return rebaseChange(synced ?? doc, doc, text);
}

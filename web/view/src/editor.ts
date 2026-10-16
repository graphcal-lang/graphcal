// The code pane: a CodeMirror editor over one file of the shown project.
// Typing is a what-if edit like any other: the text goes to the host, which
// re-projects the open sheets; nothing is saved. The sheet's values come
// back as inlays at the end of each declaring line, and the selection is
// shared: a node picked in the sheet marks its line here, the caret on a
// declaring line picks that node in the sheet.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching } from "@codemirror/language";
import { unifiedMergeView } from "@codemirror/merge";
import {
  Compartment,
  EditorState,
  StateEffect,
  StateField,
  type Transaction,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  WidgetType,
} from "@codemirror/view";
import { graphcalHighlighting, graphcalLanguage } from "./graphcalLanguage.ts";
import type { Inlay } from "./inlays.ts";
import { adoptionOf } from "./rebase.ts";

class InlayWidget extends WidgetType {
  private readonly text: string;
  private readonly selected: boolean;
  constructor(text: string, selected: boolean) {
    super();
    this.text = text;
    this.selected = selected;
  }
  override eq(other: InlayWidget): boolean {
    return other.text === this.text && other.selected === this.selected;
  }
  toDOM(): HTMLElement {
    const el = document.createElement("span");
    el.className = this.selected ? "cm-inlay selected" : "cm-inlay";
    el.textContent = this.text;
    return el;
  }
  override ignoreEvent(): boolean {
    return true;
  }
}

const setInlays = StateEffect.define<readonly Inlay[]>();
const setMark = StateEffect.define<number | null>();

function inlayDecorations(state: EditorState, inlays: readonly Inlay[]): DecorationSet {
  const marks = [];
  for (const inlay of inlays) {
    if (inlay.line < 1 || inlay.line > state.doc.lines) continue;
    const line = state.doc.line(inlay.line);
    marks.push(
      Decoration.widget({ widget: new InlayWidget(inlay.text, inlay.selected), side: 1 }).range(
        line.to,
      ),
    );
  }
  return Decoration.set(marks, true);
}

/** Values at line ends; they move with edits and are replaced when the sheet re-evaluates. */
const inlayField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(decorations, tr) {
    let next = decorations.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setInlays)) next = inlayDecorations(tr.state, effect.value);
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

const markDecoration = Decoration.line({ class: "cm-node-line" });

/** The line of the node selected in the sheet. */
const markField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(decorations, tr) {
    let next = decorations.map(tr.changes);
    for (const effect of tr.effects) {
      if (!effect.is(setMark)) continue;
      const line = effect.value;
      next =
        line === null || line < 1 || line > tr.state.doc.lines
          ? Decoration.none
          : Decoration.set([markDecoration.range(tr.state.doc.line(line).from)]);
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

export interface EditorHooks {
  /**
   * The text changed by typing; called after a short pause. Once the host has
   * applied it, and before it hands out models of the result, `applied` is
   * called with the host's text of the file.
   */
  onChange(file: string, text: string, applied: (hostText: string) => void): void;
  /** The caret moved to another line by the user. */
  onCaretLine(line: number): void;
}

/** How long typing may pause before the text is evaluated. */
const CHANGE_DELAY_MS = 350;

export class SourceEditor {
  readonly view: EditorView;
  private file: string | null = null;
  /** Set while the editor's own text is replaced from the host, so it is not echoed back. */
  private silent = false;
  private caretLine = 0;
  private pending: ReturnType<typeof setTimeout> | null = null;
  /**
   * The host's text the document is based on: last received from the host,
   * or last sent and applied. The document differs from it while typing waits
   * to be sent or the host refused it, and is then never replaced.
   */
  private synced: string | null = null;
  /** The base text of a comparison, shown as a unified diff under the editor's own text. */
  private readonly merge = new Compartment();
  private original: string | null = null;
  private readonly hooks: EditorHooks;

  constructor(parent: HTMLElement, hooks: EditorHooks) {
    this.hooks = hooks;
    this.view = new EditorView({ parent, state: this.stateFor("") });
  }

  private stateFor(text: string): EditorState {
    return EditorState.create({
      doc: text,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        drawSelection(),
        history(),
        bracketMatching(),
        graphcalLanguage,
        graphcalHighlighting,
        inlayField,
        markField,
        this.merge.of([]),
        EditorState.lineSeparator.of("\n"),
        EditorView.contentAttributes.of({ "aria-label": "Graphcal source", spellcheck: "false" }),
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && !this.silent) this.scheduleChange();
          if (update.selectionSet || update.docChanged) {
            const line = update.state.doc.lineAt(update.state.selection.main.head).number;
            if (line !== this.caretLine) {
              this.caretLine = line;
              if (update.transactions.some((tr: Transaction) => tr.isUserEvent("select"))) {
                this.hooks.onCaretLine(line);
              }
            }
          }
        }),
      ],
    });
  }

  private scheduleChange(): void {
    if (this.pending !== null) clearTimeout(this.pending);
    this.pending = setTimeout(() => this.send(), CHANGE_DELAY_MS);
  }

  private send(): void {
    this.pending = null;
    const file = this.file;
    if (file === null) return;
    const sent = this.view.state.doc.toString();
    this.hooks.onChange(file, sent, (hostText) => {
      if (this.file !== file) return;
      // The host has what was sent; it may hold more (another edit came first).
      this.synced = sent;
      this.adopt(hostText);
    });
  }

  /** Send what is typed now, without waiting for the pause. */
  flush(): void {
    if (this.pending === null) return;
    clearTimeout(this.pending);
    this.send();
  }

  get currentFile(): string | null {
    return this.file;
  }

  /**
   * Show the host's `text` of `file`. Another file replaces the whole state
   * (its history too), after what was typed in the last one is sent; the same
   * file takes the host's change in place, under what the reader typed, so
   * the caret, the typing and the undo history survive a re-projection.
   */
  show(file: string, text: string): void {
    if (file !== this.file) {
      this.flush();
      this.file = file;
      this.synced = text;
      this.silent = true;
      try {
        this.view.setState(this.stateFor(text));
      } finally {
        this.silent = false;
      }
      this.caretLine = 1;
      this.original = null;
      return;
    }
    this.adopt(text);
  }

  /** The host's text of the shown file is now `text`. */
  private adopt(text: string): void {
    const synced = this.synced;
    this.synced = text;
    const change = adoptionOf(synced, this.view.state.doc.toString(), text);
    if (change === null) return;
    this.silent = true;
    try {
      this.view.dispatch({ changes: change });
    } finally {
      this.silent = false;
    }
  }

  /** Compare the text with `original` (deleted lines in red, added in green); null ends the comparison. */
  setOriginal(original: string | null): void {
    if (original === this.original) return;
    this.original = original;
    this.view.dispatch({
      effects: this.merge.reconfigure(
        original === null
          ? []
          : unifiedMergeView({
              original,
              mergeControls: false,
              highlightChanges: true,
              gutter: true,
              syntaxHighlightDeletions: true,
            }),
      ),
    });
  }

  setInlays(inlays: readonly Inlay[]): void {
    this.view.dispatch({ effects: setInlays.of(inlays) });
  }

  /** Mark the line of the sheet's selected node and bring it into view. */
  markLine(line: number | null): void {
    const effects: StateEffect<unknown>[] = [setMark.of(line)];
    if (line !== null && line >= 1 && line <= this.view.state.doc.lines) {
      const at = this.view.state.doc.line(line).from;
      effects.push(EditorView.scrollIntoView(at, { y: "nearest", yMargin: 40 }));
    }
    this.view.dispatch({ effects });
  }

  focus(): void {
    this.view.focus();
  }
}

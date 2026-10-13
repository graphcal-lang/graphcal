// The code pane: a CodeMirror editor over one file of the shown project.
// Typing is a what-if edit like any other: the text goes to the host, which
// re-projects the open sheets; nothing is saved. The sheet's values come
// back as inlays at the end of each declaring line, and the selection is
// shared: a node picked in the sheet marks its line here, the caret on a
// declaring line picks that node in the sheet.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching } from "@codemirror/language";
import { EditorState, StateEffect, StateField, type Transaction } from "@codemirror/state";
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
import { minimalEdit } from "./vscodeHost.ts";

/** A value shown at the end of a line: the line is 1-based, as the model counts. */
export interface Inlay {
  line: number;
  text: string;
  /** Whether this is the line of the node selected in the sheet. */
  selected: boolean;
}

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
  /** The text changed by typing; called after a short pause. */
  onChange(file: string, text: string): void;
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
    this.pending = setTimeout(() => {
      this.pending = null;
      if (this.file !== null) this.hooks.onChange(this.file, this.view.state.doc.toString());
    }, CHANGE_DELAY_MS);
  }

  /** Send what is typed now, without waiting for the pause. */
  flush(): void {
    if (this.pending === null) return;
    clearTimeout(this.pending);
    this.pending = null;
    if (this.file !== null) this.hooks.onChange(this.file, this.view.state.doc.toString());
  }

  get currentFile(): string | null {
    return this.file;
  }

  /**
   * Show `text` as the content of `file`. Another file replaces the whole
   * state (its history too); the same file is patched in place so the caret
   * and the undo history survive a re-projection.
   */
  show(file: string, text: string): void {
    if (this.pending !== null) {
      clearTimeout(this.pending);
      this.pending = null;
    }
    this.silent = true;
    try {
      if (file !== this.file) {
        this.file = file;
        this.view.setState(this.stateFor(text));
        this.caretLine = 1;
        return;
      }
      const before = this.view.state.doc.toString();
      if (before === text) return;
      const edit = minimalEdit(before, text);
      this.view.dispatch({ changes: { from: edit.start, to: edit.end, insert: edit.text } });
    } finally {
      this.silent = false;
    }
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

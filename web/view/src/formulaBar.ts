// The formula bar: `subject = expr = value` for the selected cell, coloured
// like the editor, with the expression editable in place; and the note slot
// for errors and progress, mirrored in the code pane's head (the bar is
// hidden in the code-only layout).
import { spansOf, subjectTokens, tokenize, type Token } from "./formula.ts";
import type { NoteKind } from "./host.ts";
import { bindInput } from "./inlineInput.ts";
import { span } from "./dom.ts";

/** What the bar shows for a selected cell. */
export interface FormulaLine {
  /** The node as the module spells it, and the reader's names of the cell's entries. */
  label: string;
  entries: string[];
  /** The expression on one line, when the node has one. */
  expr: string | null;
  /** Whether the expression can be edited here. */
  editable: boolean;
  value: string;
  /** What the comparison says about the cell, when it changed. */
  change: string | null;
}

export class FormulaBar {
  private readonly el: HTMLElement;
  private readonly codeNote: HTMLElement;

  constructor(el: HTMLElement, codeNote: HTMLElement) {
    this.el = el;
    this.codeNote = codeNote;
  }

  /** Whether an expression is being edited in the bar. */
  get editing(): boolean {
    return this.el.classList.contains("editing");
  }

  /** Show `line`, or nothing when no cell is selected; a note stays. */
  render(line: FormulaLine | null): void {
    const content = document.createElement("span");
    content.className = "formula-line";
    // A note (an error, progress) outlives the selection it was shown with.
    const note = this.el.querySelector(".formula-note");
    this.el.classList.toggle("active", line !== null);
    this.el.classList.remove("editing");
    if (line !== null) {
      const eq = (): Token => ({ text: " = ", cls: "operator" });
      content.append(...spansOf(subjectTokens(line.label, line.entries)));
      if (line.expr !== null) {
        const expr = document.createElement("span");
        expr.className = "formula-expr";
        expr.classList.toggle("editable", line.editable);
        const coloured = document.createElement("span");
        coloured.className = "formula-coloured";
        coloured.append(...spansOf(tokenize(line.expr)));
        expr.append(coloured);
        content.append(...spansOf([eq()]), expr);
      }
      content.append(...spansOf([eq(), ...tokenize(line.value)]));
      if (line.change !== null) content.append(span("formula-was", line.change));
    }
    this.el.replaceChildren(content);
    if (note) this.el.append(note);
  }

  /** A note in the bar and the code pane's head; null clears it. */
  note(message: string | null, kind: NoteKind = "error"): void {
    this.el.querySelector(".formula-note")?.remove();
    this.codeNote.textContent = message ?? "";
    this.codeNote.className = `code-note ${message === null ? "" : kind}`;
    if (message === null) return;
    const note = document.createElement("span");
    note.className = `formula-note ${kind}`;
    note.textContent = message;
    this.el.append(note);
  }

  /**
   * Edit the shown expression in place, starting from `expr`. `commit`
   * answers whether the edit was taken; `cancel` (Escape, or nothing changed)
   * shows the bar again.
   */
  startEdit(
    expr: string,
    label: string,
    commit: (text: string) => Promise<boolean>,
    cancel: () => void,
  ): void {
    if (this.editing) return;
    const exprEl = this.el.querySelector<HTMLElement>(".formula-expr");
    const coloured = exprEl?.querySelector<HTMLElement>(".formula-coloured");
    if (!exprEl || !coloured) return;
    const input = document.createElement("input");
    input.className = "formula-input";
    input.spellcheck = false;
    input.value = expr;
    input.setAttribute("aria-label", `expression of ${label}`);
    input.addEventListener("input", () => {
      coloured.replaceChildren(...spansOf(tokenize(input.value.length > 0 ? input.value : " ")));
    });
    bindInput(input, expr, commit, cancel);
    exprEl.append(input);
    this.el.classList.add("editing");
    this.note(null);
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }

  onClick(listener: () => void): void {
    this.el.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".formula-note")) return;
      listener();
    });
  }
}

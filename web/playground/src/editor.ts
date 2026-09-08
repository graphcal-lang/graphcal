import { basicSetup } from "codemirror";
import { EditorState, type Text } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { lintGutter, setDiagnostics } from "@codemirror/lint";
import type { Diagnostic, SourceRange } from "./protocol";

export function offsetAt(document: Text, position: SourceRange["start"]): number {
  const line = document.line(Math.min(document.lines, position.line + 1));
  return Math.min(line.to, line.from + position.character);
}

export class SourceEditor {
  readonly view: EditorView;
  private readonly change: (source: string) => void;
  private readonly run: () => void;
  constructor(
    parent: HTMLElement,
    source: string,
    change: (source: string) => void,
    run: () => void,
  ) {
    this.change = change;
    this.run = run;
    this.view = new EditorView({ parent, state: this.state(source) });
    // Give the independently scrolling region keyboard access (including Safari).
    this.view.scrollDOM.tabIndex = 0;
    this.view.scrollDOM.setAttribute("aria-label", "Source scrolling region");
  }
  private state(source: string) {
    return EditorState.create({
      doc: source,
      extensions: [
        basicSetup,
        lintGutter(),
        EditorState.lineSeparator.of("\n"),
        EditorView.contentAttributes.of({
          "aria-label": "Graphcal source editor",
          spellcheck: "false",
        }),
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              this.run();
              return true;
            },
          },
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) this.change(update.state.doc.toString());
        }),
        EditorView.theme({
          "&": { height: "100%" },
          ".cm-scroller": { overflow: "auto", fontFamily: "ui-monospace, monospace" },
        }),
      ],
    });
  }
  load(source: string) {
    this.view.setState(this.state(source));
  }
  diagnostics(diagnostics: readonly Diagnostic[]) {
    this.view.dispatch(
      setDiagnostics(
        this.view.state,
        diagnostics.flatMap((diagnostic) =>
          diagnostic.labels.map((label) => ({
            from: offsetAt(this.view.state.doc, label.range.start),
            to: Math.max(
              offsetAt(this.view.state.doc, label.range.start),
              offsetAt(this.view.state.doc, label.range.end),
            ),
            severity: diagnostic.severity,
            message: label.message ?? diagnostic.message,
          })),
        ),
      ),
    );
  }
  focus(range: SourceRange) {
    const from = offsetAt(this.view.state.doc, range.start);
    const to = Math.max(from, offsetAt(this.view.state.doc, range.end));
    this.view.dispatch({ selection: { anchor: from, head: to }, scrollIntoView: true });
    this.view.focus();
  }
  destroy() {
    this.view.destroy();
  }
}

// Syntax colouring for the code pane, the same as the formula bar's: the
// expression tokens of formula.ts (which mirror the TextMate scopes of
// vscode-graphcal), plus the declaration-level words a whole file has. Every
// token class becomes a `tok-*` class on the text, so one stylesheet colours
// the bar and the editor alike, in VS Code's Dark+ colours.
import { StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { Tag, tagHighlighter } from "@lezer/highlight";
import { tokenize, type Token, type TokenClass } from "./formula.ts";

const CLASSES: readonly TokenClass[] = [
  "comment",
  "string",
  "keyword",
  "builtin",
  "function",
  "number",
  "unit",
  "namespace",
  "variable",
  "variable-punct",
  "constant",
  "language",
  "type",
  "accessor",
  "declaration",
  "operator",
  "punctuation",
  "plain",
];

const tagOf = Object.fromEntries(CLASSES.map((cls) => [cls, Tag.define()])) as Record<
  TokenClass,
  Tag
>;

/**
 * The style name a token class is reported to CodeMirror under. The stream
 * parser resolves legacy names such as `variable`, `type` and `builtin` to
 * its own tags before looking at the token table, so the names are prefixed.
 */
const styleOf = (cls: TokenClass): string => `gc-${cls}`;
const tokenTable = Object.fromEntries(CLASSES.map((cls) => [styleOf(cls), tagOf[cls]]));

/** keyword.declaration.* and keyword.other.pub in the grammar: blue in Dark+. */
const DECLARATION = new Set([
  "pub",
  "node",
  "param",
  "const",
  "assert",
  "index",
  "type",
  "dim",
  "unit",
  "base",
  "import",
  "include",
  "dag",
  "fn",
  "plot",
  "figure",
  "layer",
]);

const IDENT = /^[a-zA-Z][a-zA-Z0-9_]*$/;

function isWord(token: Token): boolean {
  return (
    (token.cls === "plain" || token.cls === "variable" || token.cls === "type") &&
    IDENT.test(token.text)
  );
}

/**
 * The tokens of one line of a file: expression tokens, with the declaration
 * words coloured as keywords and the name each declares as the grammar scopes
 * it (`node x` a variable, `const node X` a constant, `index M` / `type T` a
 * type, `fn f` a function, `import a.b::{…}` a namespace path).
 */
export function lineTokens(line: string): Token[] {
  const tokens = tokenize(line);
  const nextWord = (from: number): number => {
    let j = from;
    while (j < tokens.length && (tokens[j] as Token).text.trim() === "") j += 1;
    return j;
  };
  let constant = false;
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i] as Token;
    if (!isWord(token) || !DECLARATION.has(token.text)) continue;
    token.cls = "declaration";
    const j = nextWord(i + 1);
    const name = tokens[j] ?? { text: "", cls: "plain" };
    switch (token.text) {
      case "const":
        constant = true;
        break;
      case "node":
      case "param":
      case "assert":
      case "plot":
      case "figure":
      case "layer":
      case "dag":
        if (isWord(name) && !DECLARATION.has(name.text)) {
          name.cls = constant && token.text === "node" ? "constant" : "variable";
        }
        break;
      case "index":
      case "type":
      case "dim":
      case "unit":
        if (isWord(name) && !DECLARATION.has(name.text)) name.cls = "type";
        break;
      case "fn":
        if (isWord(name)) name.cls = "function";
        break;
      case "import":
      case "include":
        // The module path up to `::` or `(`; the members and alias after it are variables.
        {
          let path = true;
          for (let k = j; k < tokens.length; k += 1) {
            const t = tokens[k] as Token;
            if (t.text === ";") break;
            if (t.text === "::" || t.text === "(") path = false;
            // `eps(` in `include power_project.eps()` is a module, not a call.
            if ((isWord(t) || t.cls === "function") && t.text !== "as") {
              t.cls = path ? "namespace" : "variable";
            }
          }
        }
        break;
      default:
        break;
    }
    if (token.text !== "const") constant = false;
  }
  return tokens;
}

interface LineState {
  line: string;
  tokens: Token[];
  /** Index of the next token to hand out. */
  index: number;
  /** Where the next token starts in the line. */
  pos: number;
}

/** The whole line is tokenised when the stream reaches it; tokens are then handed out one by one. */
export const graphcalLanguage = StreamLanguage.define<LineState>({
  name: "Graphcal",
  startState: () => ({ line: "", tokens: [], index: 0, pos: 0 }),
  copyState: (state) => ({ ...state }),
  token(stream, state) {
    if (stream.string !== state.line || stream.pos !== state.pos) {
      state.line = stream.string;
      state.tokens = lineTokens(stream.string);
      state.index = 0;
      state.pos = 0;
      while (state.index < state.tokens.length && state.pos < stream.pos) {
        state.pos += (state.tokens[state.index] as Token).text.length;
        state.index += 1;
      }
    }
    const token = state.tokens[state.index];
    if (token === undefined) {
      stream.skipToEnd();
      return null;
    }
    state.index += 1;
    state.pos += token.text.length;
    stream.pos = state.pos;
    return token.cls === "plain" || token.text.trim() === "" ? null : styleOf(token.cls);
  },
  tokenTable,
});

/** Colours every token class as `tok-<class>`, like `spansOf` in the formula bar. */
export const graphcalHighlighter = tagHighlighter(
  CLASSES.map((cls) => ({ tag: tagOf[cls], class: `tok-${cls}` })),
);
export const graphcalHighlighting = syntaxHighlighting(graphcalHighlighter);

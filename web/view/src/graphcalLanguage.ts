import { StreamLanguage } from "@codemirror/language";

// Reserved words from grammar.ebnf, §1. Contextual words (including built-in
// type names, plugin, and fn) deliberately remain ordinary identifiers here.
const keywords = new Set([
  "param",
  "node",
  "const",
  "if",
  "else",
  "base",
  "dim",
  "unit",
  "type",
  "index",
  "for",
  "import",
  "include",
  "dag",
  "match",
  "as",
  "assert",
  "plot",
  "figure",
  "layer",
  "table",
  "pub",
]);

// Lexical colors only: no name resolution, casing heuristics, or parser state.
// Token names are CodeMirror highlighting tags, not compiler classifications.
export const graphcalLanguage = StreamLanguage.define({
  name: "Graphcal",
  token(
    stream,
  ): "comment" | "string" | "number" | "bool" | "keyword" | "operator" | "punctuation" | null {
    if (stream.eatSpace()) return null;
    if (stream.match(/^\/\/[^\r\n]*/)) return "comment";
    // Graphcal strings have no escapes and cannot cross a physical line.
    // Color an unfinished string only to the end of its line while editing.
    if (stream.match(/^"[^"\r\n]*"?/)) return "string";
    if (stream.match(/^[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9][0-9_]*)?/)) return "number";
    if (stream.match(/^[a-zA-Z][a-zA-Z0-9_]*/)) {
      const word = stream.current();
      if (word === "true" || word === "false") return "bool";
      return keywords.has(word) ? "keyword" : null;
    }
    if (stream.match(/^(\+\/-|->|=>|~=|==|!=|<=|>=|&&|\|\||[+\-*/^%=<>!@])/)) return "operator";
    if (stream.match(/^(::|[(){}[\];,:.#|_])/)) return "punctuation";
    // Unknown input must still advance; validity is the compiler's concern.
    stream.next();
    return null;
  },
});

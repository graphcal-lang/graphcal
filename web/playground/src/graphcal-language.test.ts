import { describe, expect, it } from "vite-plus/test";
import { EditorState } from "@codemirror/state";
import { ensureSyntaxTree } from "@codemirror/language";
import { classHighlighter, highlightTree } from "@lezer/highlight";
import { graphcalLanguage } from "./graphcal-language";
import { MAX_SOURCE_BYTES } from "./document";

function highlights(source: string) {
  const spans: { text: string; style: string; from: number; to: number }[] = [];
  highlightTree(graphcalLanguage.parser.parse(source), classHighlighter, (from, to, style) => {
    spans.push({ text: source.slice(from, to), style, from, to });
  });
  return spans;
}

function tokens(source: string) {
  return highlights(source).map(({ text, style }) => [text, style]);
}

describe("Graphcal lexical highlighting", () => {
  it.each([
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
  ])("highlights reserved word %s", (word) => {
    expect(tokens(word)).toEqual([[word, "tok-keyword"]]);
    expect(tokens(`${word}_value`)).toEqual([]);
  });

  it.each(["0", "42", "1_000.0_1", "2e3", "2E-3", "2.0e+3"])("highlights number %s", (number) => {
    expect(tokens(number)).toEqual([[number, "tok-number"]]);
  });

  it("colors literals and lexical delimiters, not unit or declaration roles", () => {
    expect(tokens("node value: Length = 1.2e-3 m; // node")).toEqual([
      ["node", "tok-keyword"],
      [":", "tok-punctuation"],
      ["=", "tok-operator"],
      ["1.2e-3", "tok-number"],
      [";", "tok-punctuation"],
      ["// node", "tok-comment"],
    ]);
    expect(tokens("true false True false_value")).toEqual([
      ["true", "tok-bool"],
      ["false", "tok-bool"],
    ]);
  });

  it("leaves contextual words and all identifier casings neutral", () => {
    expect(
      tokens(
        "scan unfold range linspace step points Fin key fin_key floor_key ceil_key nearest_key mark encode plots point line bar area rect tick min max Dim Index Nat Type Dimensionless Bool Int Datetime Complex Key bind plugin fn PI CamelCase lower_snake_case NODE",
      ),
    ).toEqual([]);
  });

  it.each([
    "+",
    "-",
    "*",
    "/",
    "^",
    "%",
    "=",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "&&",
    "||",
    "!",
    "->",
    "=>",
    "~=",
    "+/-",
    "@",
  ])("recognizes operator %s", (operator) => {
    expect(tokens(operator)).toEqual([[operator, "tok-operator"]]);
  });

  it.each(["(", ")", "{", "}", "[", "]", ";", ",", ":", "::", ".", "#", "|", "_"])(
    "recognizes punctuation %s",
    (punctuation) => {
      expect(tokens(punctuation)).toEqual([[punctuation, "tok-punctuation"]]);
    },
  );

  it("respects comment/string boundaries, including literal backslashes", () => {
    expect(tokens('"// node 🙂" // "string"')).toEqual([
      ['"// node 🙂"', "tok-string"],
      ['// "string"', "tok-comment"],
    ]);
    expect(tokens('"backslash\\" node')).toEqual([
      ['"backslash\\"', "tok-string"],
      ["node", "tok-keyword"],
    ]);
  });

  it("recovers from unfinished strings at newlines, preserving UTF-16 ranges", () => {
    const text = '"🙂 unfinished\r\nnode';
    expect(highlights(text)).toEqual([
      { text: '"🙂 unfinished', style: "tok-string", from: 0, to: 14 },
      { text: "node", style: "tok-keyword", from: 16, to: 20 },
    ]);
    expect(tokens("// 🙂\r\nnode")).toEqual([
      ["// 🙂", "tok-comment"],
      ["node", "tok-keyword"],
    ]);
    expect(tokens("`🙂? node (\n42")).toEqual([
      ["node", "tok-keyword"],
      ["(", "tok-punctuation"],
      ["42", "tok-number"],
    ]);
    expect(tokens("")).toEqual([]);
  });

  it("tokenizes a document near the source limit", () => {
    const line = "node value: Int = 42; // example\n";
    const count = Math.floor(MAX_SOURCE_BYTES / line.length);
    const result = highlights(line.repeat(count));
    expect(result).toHaveLength(count * 6);
    expect(result.at(-1)).toEqual({
      text: "// example",
      style: "tok-comment",
      from: count * line.length - 11,
      to: count * line.length - 1,
    });
  });

  it("updates highlighting through CodeMirror source replacement transactions", () => {
    const original = '"unfinished\nnode x = 1;';
    let state = EditorState.create({ doc: original, extensions: [graphcalLanguage] });
    for (const insert of ["// replaced\nnode x = 2;", original]) {
      state = state.update({ changes: { from: 0, to: state.doc.length, insert } }).state;
      const tree = ensureSyntaxTree(state, state.doc.length, 1000);
      expect(tree).not.toBeNull();
      const actual: [string, string][] = [];
      highlightTree(tree!, classHighlighter, (from, to, style) => {
        actual.push([state.doc.sliceString(from, to), style]);
      });
      expect(actual).toEqual(tokens(insert));
    }
  });
});

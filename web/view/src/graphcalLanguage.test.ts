import { describe, expect, test } from "vite-plus/test";
import { highlightTree } from "@lezer/highlight";
import { graphcalHighlighter, graphcalLanguage, lineTokens } from "./graphcalLanguage.ts";

/** `text:class` for every token that is not whitespace. */
function classes(line: string): string[] {
  return lineTokens(line)
    .filter((t) => t.text.trim() !== "")
    .map((t) => `${t.text}:${t.cls}`);
}

describe("lineTokens", () => {
  test("colours declaration words and the names they declare as the grammar does", () => {
    expect(classes("pub node p_total: Power[Mode] = sum(@p_mode);")).toEqual([
      "pub:declaration",
      "node:declaration",
      "p_total:variable",
      ": :punctuation",
      "Power:type",
      "[:punctuation",
      "Mode:type",
      "]:punctuation",
      "=:operator",
      "sum:function",
      "(:punctuation",
      "@:variable-punct",
      "p_mode:variable",
      "):punctuation",
      ";:punctuation",
    ]);
    expect(classes("const node g0_eff: Dimensionless = 0.98;").slice(0, 3)).toEqual([
      "const:declaration",
      "node:declaration",
      "g0_eff:constant",
    ]);
    expect(classes("param margin: Dimensionless = 0.2;").slice(0, 2)).toEqual([
      "param:declaration",
      "margin:variable",
    ]);
    expect(classes("assert within_budget = @p < @b;").slice(0, 2)).toEqual([
      "assert:declaration",
      "within_budget:variable",
    ]);
    expect(classes("pub index mode = { Safe, Nominal };").slice(0, 3)).toEqual([
      "pub:declaration",
      "index:declaration",
      "mode:type",
    ]);
  });

  test("colours an import path as a namespace and its members as variables", () => {
    expect(classes("import rocket_project.constants::{ g0 };")).toEqual([
      "import:declaration",
      "rocket_project:namespace",
      ".:punctuation",
      "constants:namespace",
      ":::accessor",
      "{:punctuation",
      "g0:variable",
      "}:punctuation",
      ";:punctuation",
    ]);
    expect(classes("include power_project.eps() as eps;")).toEqual([
      "include:declaration",
      "power_project:namespace",
      ".:punctuation",
      "eps:namespace",
      "(:punctuation",
      "):punctuation",
      "as:plain",
      "eps:variable",
      ";:punctuation",
    ]);
  });

  test("leaves expression lines to the formula tokenizer", () => {
    expect(classes("  @p_bus[c] * @duty[c, m] * (1.0 + @margin)")).toContain("@:variable-punct");
    expect(classes("// a comment")).toEqual(["// a comment:comment"]);
  });

  test("the editor's highlighter puts the same tok-* classes on the text", () => {
    const source = "param margin: Dimensionless = 0.2;\nconst node g: Power = 1.0 W;";
    const spans: string[] = [];
    highlightTree(graphcalLanguage.parser.parse(source), graphcalHighlighter, (from, to, cls) => {
      spans.push(`${source.slice(from, to)}:${cls}`);
    });
    expect(spans).toEqual([
      "param:tok-declaration",
      "margin:tok-variable",
      ": :tok-punctuation",
      "Dimensionless:tok-type",
      "=:tok-operator",
      "0.2:tok-number",
      ";:tok-punctuation",
      "const:tok-declaration",
      "node:tok-declaration",
      "g:tok-constant",
      ": :tok-punctuation",
      "Power:tok-type",
      "=:tok-operator",
      "1.0:tok-number",
      "W:tok-unit",
      ";:tok-punctuation",
    ]);
  });
});

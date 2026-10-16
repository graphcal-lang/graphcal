import { Text } from "@codemirror/state";
import { describe, expect, test } from "vite-plus/test";
import { adoptionOf, rebaseChange, typedEdit } from "./rebase.ts";

function rebased(base: string, ours: string, theirs: string): string {
  return rebaseChange(base, ours, theirs)
    .apply(Text.of(ours.split("\n")))
    .sliceString(0, undefined, "\n");
}

describe("rebaseChange", () => {
  const base = "param a: Power = 1.0 W;\nparam b: Power = 2.0 W;\n";

  test("keeps the reader's typing and takes the host's change elsewhere", () => {
    const ours = base.replace("param a", "param alpha");
    const theirs = base.replace("2.0 W", "5.0 W");
    expect(rebased(base, ours, theirs)).toBe(
      "param alpha: Power = 1.0 W;\nparam b: Power = 5.0 W;\n",
    );
  });

  test("a host change the reader has not touched is taken whole", () => {
    const theirs = base.replace("1.0 W", "3.0 W");
    expect(rebased(base, base, theirs)).toBe(theirs);
  });

  test("nothing from the host leaves the reader's text as it is", () => {
    const ours = `${base}node c: Power = @a + @b;\n`;
    expect(rebased(base, ours, base)).toBe(ours);
  });

  test("keeps carriage returns inside their lines", () => {
    const crlf = "param a: Int = 1;\r\nparam b: Int = 2;\r\n";
    const ours = crlf.replace("a: Int", "aa: Int");
    const theirs = crlf.replace("= 2", "= 7");
    expect(rebased(crlf, ours, theirs)).toBe("param aa: Int = 1;\r\nparam b: Int = 7;\r\n");
  });
});

describe("adoptionOf", () => {
  const synced = "param a: Int = 1;\n";
  const typed = "param a: Int = 1;\nnode b: Int = @a +\n";

  test("the host's same text never replaces what was typed", () => {
    expect(adoptionOf(synced, typed, synced)).toBeNull();
  });

  test("a host change comes in under the typing", () => {
    const change = adoptionOf(synced, typed, "param a: Int = 4;\n");
    expect(change?.apply(Text.of(typed.split("\n"))).sliceString(0, undefined, "\n")).toBe(
      "param a: Int = 4;\nnode b: Int = @a +\n",
    );
  });

  test("a document already holding the host's text is left alone", () => {
    expect(adoptionOf(synced, typed, typed)).toBeNull();
  });
});

describe("typedEdit", () => {
  test("is the typed change, on the revision the document is based on", () => {
    const synced = { text: "param a: Int = 1;\nparam b: Int = 2;\n", revision: 3 };
    expect(typedEdit("src/m.gcl", synced, synced.text.replace("= 2", "= 25"))).toEqual({
      file: "src/m.gcl",
      revision: 3,
      range: { start: { line: 1, character: 16 }, end: { line: 1, character: 16 } },
      text: "5",
    });
  });

  test("nothing typed is no edit", () => {
    expect(typedEdit("src/m.gcl", { text: "x", revision: 1 }, "x")).toBeNull();
  });
});

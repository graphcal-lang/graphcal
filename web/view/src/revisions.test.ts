import { describe, expect, test } from "vite-plus/test";
import type { Range } from "./model.ts";
import {
  applyChange,
  historyOf,
  place,
  received,
  recorded,
  type FileHistory,
  type SourceEdit,
} from "./revisions.ts";

function on(line: number, from: number, to: number): Range {
  return { start: { line, character: from }, end: { line, character: to } };
}

/** Place `edit` and record it as `revision`, as a host does on the edit's turn. */
function apply(history: FileHistory, edit: Omit<SourceEdit, "file">, revision: number) {
  const placed = place(history, edit);
  if (!placed.ok) throw new Error(placed.message);
  return recorded(history, placed.change, revision);
}

const source = "param a: Power = 1.0 W;\nparam b: Power = 2.0 W;\n";

describe("placing an edit on the latest text", () => {
  test("two edits made on the same model both land (#2067)", () => {
    // B and C are both built from the model of revision 1.
    const b = { revision: 1, range: on(0, 17, 22), text: "10.0 W" };
    const c = { revision: 1, range: on(1, 17, 22), text: "20.0 W" };
    const afterB = apply(historyOf(source, 1), b, 2);
    const afterC = apply(afterB, c, 3);
    expect(afterC.text).toBe("param a: Power = 10.0 W;\nparam b: Power = 20.0 W;\n");
    expect(afterC.revision).toBe(3);
  });

  test("an edit after a change on its own line is moved along", () => {
    const history = apply(
      historyOf(source, 1),
      { revision: 1, range: on(0, 6, 7), text: "alpha" },
      2,
    );
    const placed = place(history, { revision: 1, range: on(0, 17, 22), text: "3.0 W" });
    expect(placed.ok && applyChange(history.text, placed.change)).toBe(
      "param alpha: Power = 3.0 W;\nparam b: Power = 2.0 W;\n",
    );
  });

  test("an edit whose text another edit changed is refused", () => {
    const history = apply(
      historyOf(source, 1),
      { revision: 1, range: on(0, 17, 22), text: "5.0 W" },
      2,
    );
    expect(place(history, { revision: 1, range: on(0, 17, 22), text: "6.0 W" })).toEqual({
      ok: false,
      message: "another edit changed the same text; edit again",
    });
  });

  test("an edit on the latest revision is placed as it is", () => {
    expect(place(historyOf(source, 4), { revision: 4, range: on(1, 17, 22), text: "x" })).toEqual({
      ok: true,
      change: { from: 41, to: 46, insert: "x" },
    });
  });

  test("an edit from before what the history knows is refused", () => {
    const history = received(historyOf(source, 1), "// edited elsewhere\n", 5);
    expect(place(history, { revision: 1, range: on(0, 0, 1), text: "x" })).toMatchObject({
      ok: false,
    });
  });

  test("a range outside its revision's text is refused", () => {
    expect(place(historyOf(source, 1), { revision: 1, range: on(5, 0, 1), text: "x" })).toEqual({
      ok: false,
      message: "the range is outside the file",
    });
  });
});

describe("received", () => {
  test("the same text at a newer revision keeps the changes to place through", () => {
    const history = apply(historyOf(source, 1), { revision: 1, range: on(0, 6, 7), text: "aa" }, 2);
    const again = received(history, history.text, 7);
    expect(again.revision).toBe(7);
    const placed = place(again, { revision: 1, range: on(1, 6, 7), text: "bb" });
    expect(placed.ok && applyChange(again.text, placed.change)).toBe(
      "param aa: Power = 1.0 W;\nparam bb: Power = 2.0 W;\n",
    );
  });

  test("an older copy arriving late is ignored", () => {
    const history = historyOf(source, 5);
    expect(received(history, "older", 3)).toBe(history);
  });
});

describe("a CRLF file", () => {
  test("is edited at the compiler's range, with each \\r part of its line", () => {
    const crlf = "param a: Power[M] = table[M] {\r\n    x: 1.0 W;\r\n};\r\n";
    const placed = place(historyOf(crlf, 1), { revision: 1, range: on(1, 7, 12), text: "9.0 W" });
    expect(placed.ok && applyChange(crlf, placed.change)).toBe(crlf.replace("1.0 W", "9.0 W"));
  });

  test("an inverted range is refused", () => {
    const range = { start: { line: 0, character: 2 }, end: { line: 0, character: 1 } };
    expect(place(historyOf("abc", 1), { revision: 1, range, text: "x" })).toEqual({
      ok: false,
      message: "the range is inverted",
    });
  });
});

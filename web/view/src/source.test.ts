import { describe, expect, test } from "vite-plus/test";
import { EditError, offsetOf, replaceRange } from "./source.ts";

describe("offsetOf", () => {
  test("counts lines at \\n, with a \\r part of its line", () => {
    const source = "a = 1;\r\nb = 2;\r\n";
    expect(offsetOf(source, { line: 1, character: 4 })).toBe(12);
    expect(offsetOf(source, { line: 0, character: 7 })).toBe(7);
    expect(offsetOf(source, { line: 2, character: 0 })).toBe(source.length);
  });

  test("a position outside the text is refused", () => {
    expect(() => offsetOf("a\nb", { line: 0, character: 2 })).toThrow(EditError);
    expect(() => offsetOf("a\nb", { line: 2, character: 0 })).toThrow(EditError);
  });
});

describe("replaceRange", () => {
  test("replaces the compiler's range in a CRLF file", () => {
    const source = "param a: Power[M] = table[M] {\r\n    x: 1.0 W;\r\n};\r\n";
    const range = { start: { line: 1, character: 7 }, end: { line: 1, character: 12 } };
    expect(replaceRange(source, range, "9.0 W")).toBe(source.replace("1.0 W", "9.0 W"));
  });

  test("an inverted range is refused", () => {
    const range = { start: { line: 0, character: 2 }, end: { line: 0, character: 1 } };
    expect(() => replaceRange("abc", range, "x")).toThrow("the range is inverted");
  });
});

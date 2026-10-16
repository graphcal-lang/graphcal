import { describe, expect, test } from "vite-plus/test";
import { EditError, minimalEdit, offsetOf, positionOf } from "./source.ts";

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

describe("minimalEdit", () => {
  test("trims the shared prefix and suffix", () => {
    expect(minimalEdit("param n = 1;\n", "param n = 2;\n")).toEqual({
      start: 10,
      end: 11,
      text: "2",
    });
  });
});

describe("positionOf", () => {
  test("is the inverse of offsetOf", () => {
    const source = "a = 1;\r\nbb = 2;\n\nc";
    for (let offset = 0; offset <= source.length; offset += 1) {
      expect(offsetOf(source, positionOf(source, offset))).toBe(offset);
    }
    expect(positionOf(source, 8)).toEqual({ line: 1, character: 0 });
  });
});

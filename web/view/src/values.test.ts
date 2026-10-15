import { describe, expect, test } from "vite-plus/test";
import { shortMagnitude } from "./values.ts";

describe("shortMagnitude", () => {
  test.each([
    ["0", "0"],
    ["-0", "0"],
    ["1", "1"],
    ["123456", "123500"],
    ["999999", "1e+6"],
    ["123456789", "1.235e+8"],
    ["-2.5", "-2.5"],
    ["0.001", "0.001"],
    ["0.0012346", "0.001235"],
    ["0.123456", "0.1235"],
    ["0.000999", "9.99e-4"],
    ["0.000001", "1e-6"],
    ["1.2346e-6", "1.235e-6"],
    ["4.7e-9", "4.7e-9"],
    ["-3e-8", "-3e-8"],
    ["1e308", "1e+308"],
    ["not a number", "not a number"],
  ])("%s → %s", (magnitude, shown) => {
    expect(shortMagnitude(magnitude)).toBe(shown);
  });

  test("a nonzero value never reads as zero", () => {
    for (let exponent = -320; exponent <= 0; exponent += 1) {
      for (const sign of [1, -1]) {
        const shown = shortMagnitude(String(sign * 1.5 * 10 ** exponent));
        expect(Number(shown), shown).not.toBe(0);
      }
    }
  });
});

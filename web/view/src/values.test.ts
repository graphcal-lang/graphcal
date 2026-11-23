import { describe, expect, test } from "vite-plus/test";
import type { Value } from "./model.ts";
import { displayAt, shortMagnitude, statusOf } from "./values.ts";

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

describe("statusOf", () => {
  const mass: Value = {
    kind: "quantity",
    display: "2 kg",
    magnitude: "2",
    si_value: 2,
    unit: "kg",
  };

  test("a todo hole", () => {
    const node = { value: null, unavailable: { kind: "todo" as const } };
    expect(statusOf(node)).toEqual({
      kind: "todo",
      label: "TODO",
      note: "TODO: this formula is unfinished",
    });
    expect(displayAt(node, [])).toBe("TODO");
  });

  test("a blocked node names every unfinished formula it waits on", () => {
    const node = {
      value: null,
      unavailable: { kind: "blocked" as const, unfinished: ["cell_voltage", "eps::loss"] },
    };
    expect(statusOf(node)?.note).toBe("BLOCKED: waits on the unfinished cell_voltage, eps::loss");
    expect(displayAt(node, ["Safe"])).toBe("BLOCKED");
  });

  test("a value, a failure, and a model from an older engine have no status", () => {
    expect(statusOf({ value: mass, unavailable: null })).toBeNull();
    expect(statusOf({ value: null, unavailable: null })).toBeNull();
    expect(statusOf({ value: null })).toBeNull();
    expect(displayAt({ value: mass }, [])).toBe("2 kg");
    expect(displayAt({ value: null }, [])).toBe("—");
  });
});

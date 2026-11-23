import { describe, expect, test } from "vite-plus/test";
import { tipPlace } from "./tip.ts";

describe("tipPlace", () => {
  const viewport = { width: 800, height: 600 };
  const tip = { width: 200, height: 20 };

  test("under the anchor, at its left edge", () => {
    expect(tipPlace({ left: 100, top: 50, bottom: 70 }, tip, viewport)).toEqual({
      left: 100,
      top: 76,
    });
  });

  test("above the anchor when there is no room under it", () => {
    expect(tipPlace({ left: 100, top: 570, bottom: 590 }, tip, viewport).top).toBe(544);
  });

  test("kept inside the right and left edges", () => {
    expect(tipPlace({ left: 700, top: 50, bottom: 70 }, tip, viewport).left).toBe(594);
    expect(tipPlace({ left: -40, top: 50, bottom: 70 }, tip, viewport).left).toBe(6);
  });
});

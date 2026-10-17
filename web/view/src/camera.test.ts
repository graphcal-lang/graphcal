import { describe, expect, test } from "vite-plus/test";
import { centredOf, clampScale, overviewOf, zoomAround } from "./camera.ts";

describe("camera", () => {
  test("clamps the scale", () => {
    expect(clampScale(0)).toBe(0.1);
    expect(clampScale(10)).toBe(4);
    expect(clampScale(1.5)).toBe(1.5);
  });

  test("zooms around a point that stays put", () => {
    const next = zoomAround({ x: 10, y: 20, scale: 1 }, 110, 220, 2);
    expect(next).toEqual({ x: -90, y: -180, scale: 2 });
    // The stage point under (110, 220) is (100, 200) before and after.
    expect((110 - next.x) / next.scale).toBe(100);
    expect((220 - next.y) / next.scale).toBe(200);
  });

  test("overview scales down to fit, centred, and never up", () => {
    expect(overviewOf({ width: 500, height: 500 }, { width: 1000, height: 250 })).toEqual({
      scale: 0.5,
      x: 0,
      y: 187.5,
    });
    expect(overviewOf({ width: 500, height: 500 }, { width: 100, height: 100 })).toEqual({
      scale: 1,
      x: 200,
      y: 200,
    });
  });

  test("an empty or hidden canvas or stage keeps the full size, never NaN", () => {
    expect(overviewOf({ width: 0, height: 0 }, { width: 0, height: 0 })).toEqual({
      scale: 1,
      x: 0,
      y: 0,
    });
    expect(overviewOf({ width: 500, height: 500 }, { width: 0, height: 0 }).scale).toBe(1);
    expect(overviewOf({ width: 0, height: 0 }, { width: 300, height: 300 }).scale).toBe(1);
  });

  test("a scale that is not a number is clamped to full size", () => {
    expect(clampScale(Number.NaN)).toBe(1);
  });

  test("a stage larger than the canvas shows its middle", () => {
    expect(centredOf({ width: 500, height: 500 }, { width: 900, height: 300 })).toEqual({
      scale: 1,
      x: -200,
      y: 0,
    });
  });
});

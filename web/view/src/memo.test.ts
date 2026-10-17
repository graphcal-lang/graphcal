import { describe, expect, test } from "vite-plus/test";
import { sharedUntilRejected } from "./memo.ts";

describe("sharedUntilRejected", () => {
  test("runs once while it succeeds", async () => {
    let runs = 0;
    const load = sharedUntilRejected(() => Promise.resolve(++runs));
    expect(await load()).toBe(1);
    expect(await load()).toBe(1);
    expect(runs).toBe(1);
  });

  test("runs again after a failure instead of keeping it", async () => {
    let runs = 0;
    const load = sharedUntilRejected(() => {
      runs += 1;
      return runs === 1 ? Promise.reject(new Error("offline")) : Promise.resolve(runs);
    });
    await expect(load()).rejects.toThrow("offline");
    expect(await load()).toBe(2);
    expect(await load()).toBe(2);
  });
});

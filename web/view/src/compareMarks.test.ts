import { describe, expect, test } from "vite-plus/test";
import { cellTip } from "./compareMarks.ts";
import { statusOf } from "./values.ts";

describe("cellTip", () => {
  test("a changed value says what it was", () => {
    expect(cellTip(null, "5 W")).toBe("was 5 W");
  });

  test("a cell that became blocked keeps the reason, then what it was", () => {
    const status = statusOf({
      value: null,
      unavailable: { kind: "blocked", unfinished: ["cell_voltage"] },
    });
    expect(cellTip(status, "5 W")).toBe("BLOCKED: waits on the unfinished cell_voltage\nwas 5 W");
  });
});

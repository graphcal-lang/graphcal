import { expect, it } from "vite-plus/test";
import { boundedOutcome, MAX_OUTPUT_BYTES } from "./output-budget";
import { denyPlotResource } from "./figures";

it("bounds rendered transport before it reaches the UI without truncating values", () => {
  const result = { status: "rejected", error: { message: "Short diagnostic" } };
  expect(boundedOutcome(result)).toEqual(result);
  expect(() =>
    boundedOutcome({ status: "rejected", error: { message: "🙂".repeat(MAX_OUTPUT_BYTES / 4) } }),
  ).toThrow("8 MiB");
});
it("denies plot data, image and link resource loading", async () => {
  await expect(denyPlotResource()).rejects.toThrow("External plot resources");
});

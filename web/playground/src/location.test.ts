import { expect, it } from "vite-plus/test";
import { documentLocation } from "./location";

it("gives shared fragments precedence, including malformed payloads", () => {
  expect(
    documentLocation(new URL("https://graphcal.org/playground/?example=rocket#broken")),
  ).toEqual({ kind: "shared", fragment: "#broken" });
  expect(documentLocation(new URL("https://graphcal.org/playground/?example=hello"))).toEqual({
    kind: "example",
    id: "hello",
  });
  expect(documentLocation(new URL("https://graphcal.org/playground/"))).toEqual({
    kind: "example",
    id: "rocket",
  });
  expect(() =>
    documentLocation(new URL("https://graphcal.org/playground/?example=a&example=b")),
  ).toThrow();
});

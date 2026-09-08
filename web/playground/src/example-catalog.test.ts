import { expect, it } from "vite-plus/test";
import { selectExample } from "./example-catalog";

it("only selects catalog entries, never constructs resource paths", () => {
  const example = {
    id: "rocket",
    title: "Rocket",
    description: "Units",
    filename: "rocket.gcl",
    source: "",
  };
  expect(selectExample([example], "rocket")).toBe(example);
  for (const id of ["../rocket", "https://evil.example/a", "unknown", ""]) {
    expect(() => selectExample([example], id)).toThrow("Unknown example");
  }
});

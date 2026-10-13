import { describe, expect, test } from "vite-plus/test";
import { describeRoute, formatRoute, parseRoute, routeFromText } from "./route.ts";

describe("parseRoute", () => {
  test("owner, repo, ref and path", () => {
    expect(parseRoute("#graphcal-lang/graphcal@main/tests/fixtures/valid/rocket.gcl")).toEqual({
      owner: "graphcal-lang",
      repo: "graphcal",
      ref: "main",
      path: "tests/fixtures/valid/rocket.gcl",
    });
  });

  test("the ref is optional and may be percent-encoded", () => {
    expect(parseRoute("#o/r/src/a.gcl")?.ref).toBeNull();
    expect(parseRoute("#o/r@feat%2Fx/src/a.gcl")?.ref).toBe("feat/x");
    expect(parseRoute("#o/r@0123abc/a.gcl")?.ref).toBe("0123abc");
  });

  test("rejects what is not a file address", () => {
    expect(parseRoute("")).toBeNull();
    expect(parseRoute("#")).toBeNull();
    expect(parseRoute("#power_max")).toBeNull();
    expect(parseRoute("#o/r")).toBeNull();
    expect(parseRoute("#o/r/a.toml")).toBeNull();
    expect(parseRoute("#o/r@/a.gcl")).toBeNull();
    expect(parseRoute("#o/r/../a.gcl")).toBeNull();
    expect(parseRoute("#o/r//a.gcl")).toBeNull();
    expect(parseRoute("#o/bad repo/a.gcl")).toBeNull();
  });
});

test("formatRoute round-trips, encoding a slashed ref", () => {
  const route = { owner: "o", repo: "r", ref: "feat/x", path: "src/a.gcl" };
  expect(formatRoute(route)).toBe("#o/r@feat%2Fx/src/a.gcl");
  expect(parseRoute(formatRoute(route))).toEqual(route);
  expect(formatRoute({ ...route, ref: null })).toBe("#o/r/src/a.gcl");
});

test("describeRoute names the resolved ref", () => {
  const route = { owner: "o", repo: "r", ref: null, path: "a.gcl" };
  expect(describeRoute(route)).toBe("r @ default branch");
  expect(describeRoute(route, "main")).toBe("r @ main");
});

describe("routeFromText", () => {
  test("reads a GitHub file URL, a fragment, and the short form", () => {
    const expected = { owner: "o", repo: "r", ref: "feat/x", path: "src/a.gcl" };
    expect(routeFromText("https://github.com/o/r/blob/feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("#o/r@feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("o/r@feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("  o/r/src/a.gcl ")).toEqual({ ...expected, ref: null });
    expect(routeFromText("https://github.com/o/r")).toBeNull();
    expect(routeFromText("")).toBeNull();
  });
});

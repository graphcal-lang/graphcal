import { describe, expect, test } from "vite-plus/test";
import {
  describeRoute,
  formatRoute,
  parseRoute,
  routeFromText,
  sameRef,
  withHead,
  type Route,
} from "./route.ts";

describe("parseRoute", () => {
  test("owner, repo, ref and path", () => {
    const compare: Route = {
      owner: "o",
      repo: "r",
      at: { kind: "compare", base: "main", head: "feat/x" },
      path: "src/a.gcl",
    };
    expect(parseRoute("#o/r@main...feat%2Fx/src/a.gcl")).toEqual(compare);
    expect(parseRoute("#o/r@main.../src/a.gcl")).toBeNull();
    expect(parseRoute("#o/r@...main/src/a.gcl")).toBeNull();
    expect(formatRoute(compare)).toBe("#o/r@main...feat%2Fx/src/a.gcl");
    expect(describeRoute(compare)).toBe("r @ main…feat/x");
    expect(parseRoute("#graphcal-lang/graphcal@main/tests/fixtures/valid/rocket.gcl")).toEqual({
      owner: "graphcal-lang",
      repo: "graphcal",
      at: { kind: "ref", ref: "main" },
      path: "tests/fixtures/valid/rocket.gcl",
    });
  });

  test("the ref is optional and may be percent-encoded", () => {
    expect(parseRoute("#o/r/src/a.gcl")?.at).toEqual({ kind: "default" });
    expect(parseRoute("#o/r@feat%2Fx/src/a.gcl")?.at).toEqual({ kind: "ref", ref: "feat/x" });
    expect(parseRoute("#o/r@0123abc/a.gcl")?.at).toEqual({ kind: "ref", ref: "0123abc" });
  });

  test("the path is percent-decoded", () => {
    expect(parseRoute("#o/r/my%20dir/%E9%9B%BB%E5%8A%9B.gcl")?.path).toBe("my dir/電力.gcl");
  });

  test("malformed percent-encoding is not an address, not an exception", () => {
    expect(parseRoute("#o/r@%/a.gcl")).toBeNull();
    expect(parseRoute("#o/r@main...%E9/a.gcl")).toBeNull();
    expect(parseRoute("#o/r/%zz.gcl")).toBeNull();
  });

  test("dot segments are checked after decoding", () => {
    expect(parseRoute("#o/r/%2E%2E/a.gcl")).toBeNull();
    expect(parseRoute("#o/r/%2E/a.gcl")).toBeNull();
    expect(parseRoute("#o/r/a%2Fb.gcl")).toBeNull();
  });

  test("a repository name may start with a dot", () => {
    expect(parseRoute("#o/.github/a.gcl")?.repo).toBe(".github");
    expect(parseRoute("#o/./a.gcl")).toBeNull();
    expect(parseRoute("#o/../a.gcl")).toBeNull();
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

describe("formatRoute", () => {
  test("round-trips, encoding a slashed ref", () => {
    const route: Route = {
      owner: "o",
      repo: "r",
      at: { kind: "ref", ref: "feat/x" },
      path: "src/a.gcl",
    };
    expect(formatRoute(route)).toBe("#o/r@feat%2Fx/src/a.gcl");
    expect(parseRoute(formatRoute(route))).toEqual(route);
    expect(formatRoute({ ...route, at: { kind: "default" } })).toBe("#o/r/src/a.gcl");
  });

  test("encodes each path segment, so what it writes reads back", () => {
    const route: Route = {
      owner: "o",
      repo: ".github",
      at: { kind: "default" },
      path: "my dir/50% #1/電力.gcl",
    };
    const hash = formatRoute(route);
    expect(hash).toBe("#o/.github/my%20dir/50%25%20%231/%E9%9B%BB%E5%8A%9B.gcl");
    expect(parseRoute(hash)).toEqual(route);
  });
});

test("describeRoute names the resolved ref", () => {
  const route: Route = { owner: "o", repo: "r", at: { kind: "default" }, path: "a.gcl" };
  expect(describeRoute(route)).toBe("r @ default branch");
  expect(describeRoute(route, "main")).toBe("r @ main");
});

test("withHead keeps the base of a comparison; sameRef compares both sides", () => {
  const compare = { kind: "compare", base: "main", head: "x" } as const;
  expect(withHead(compare, "y")).toEqual({ kind: "compare", base: "main", head: "y" });
  expect(withHead({ kind: "default" }, "y")).toEqual({ kind: "ref", ref: "y" });
  expect(sameRef(compare, { kind: "ref", ref: "x" })).toBe(false);
  expect(sameRef(compare, { ...compare })).toBe(true);
  expect(sameRef({ kind: "default" }, { kind: "default" })).toBe(true);
});

describe("routeFromText", () => {
  test("reads a GitHub file URL, a fragment, and the short form", () => {
    const expected: Route = {
      owner: "o",
      repo: "r",
      at: { kind: "ref", ref: "feat/x" },
      path: "src/a.gcl",
    };
    expect(routeFromText("https://github.com/o/r/blob/feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("#o/r@feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("o/r@feat%2Fx/src/a.gcl")).toEqual(expected);
    expect(routeFromText("  o/r/src/a.gcl ")).toEqual({ ...expected, at: { kind: "default" } });
    expect(routeFromText("https://github.com/o/r")).toBeNull();
    expect(routeFromText("")).toBeNull();
  });

  test("drops a GitHub URL's line anchor and query", () => {
    const expected: Route = {
      owner: "o",
      repo: ".github",
      at: { kind: "ref", ref: "main" },
      path: "my dir/a.gcl",
    };
    expect(routeFromText("https://github.com/o/.github/blob/main/my%20dir/a.gcl#L10")).toEqual(
      expected,
    );
    expect(
      routeFromText("https://github.com/o/.github/blob/main/my%20dir/a.gcl?plain=1#L10-L12"),
    ).toEqual(expected);
  });
});

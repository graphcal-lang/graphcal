import { describe, expect, test } from "vite-plus/test";
import { compareOf } from "./compareBase.ts";
import type { ViewModel } from "./model.ts";

/** A model of `file`; only its file matters to the lookup. */
function modelOf(file: string): ViewModel {
  return { module: { id: file, file }, target: { kind: "module" } } as unknown as ViewModel;
}

describe("compareOf", () => {
  const evaluated = modelOf("src/p/a.gcl");
  const compare = compareOf("main", {
    kind: "read",
    changedFiles: new Set(["src/p/a.gcl"]),
    sources: new Map([
      ["src/p/a.gcl", "param a: Power = 1.0 W"],
      ["src/p/broken.gcl", "param"],
    ]),
    models: new Map([
      ["src/p/a.gcl", { kind: "present", value: evaluated }],
      ["src/p/broken.gcl", { kind: "failed", message: "expected a name" }],
    ]),
  });

  test("tells a new file from one that did not evaluate at the base", () => {
    expect(compare.model(modelOf("src/p/a.gcl"))).toEqual({ kind: "present", value: evaluated });
    expect(compare.model(modelOf("src/p/new.gcl"))).toEqual({ kind: "absent" });
    expect(compare.model(modelOf("src/p/broken.gcl"))).toEqual({
      kind: "failed",
      message: "expected a name",
    });
    expect(compare.source("src/p/a.gcl")).toEqual({
      kind: "present",
      value: "param a: Power = 1.0 W",
    });
    expect(compare.source("src/p/new.gcl")).toEqual({ kind: "absent" });
  });

  test("an include instance is not compared with its module alone", () => {
    const instance = {
      module: { id: "a", file: "src/p/a.gcl" },
      target: { kind: "instance", root: { id: "main" }, instance: { id: "i" } },
    } as unknown as ViewModel;
    expect(compare.model(instance).kind).toBe("failed");
  });

  test("a base that could not be read fails everything, rather than making it new", () => {
    const failed = compareOf("nope", {
      kind: "failed",
      message: "o/r has no branch, tag or commit nope",
    });
    const why = { kind: "failed", message: "o/r has no branch, tag or commit nope" };
    expect(failed.ref).toBe("nope");
    expect(failed.model(modelOf("src/p/a.gcl"))).toEqual(why);
    expect(failed.source("src/p/a.gcl")).toEqual(why);
    expect(failed.changedFiles.size).toBe(0);
  });
});

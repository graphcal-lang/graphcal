import { afterEach, beforeEach, describe, expect, test, vi } from "vite-plus/test";
import type { Commit, GitHub, Tree } from "./github.ts";
import { GitHubHost } from "./githubHost.ts";
import type { ViewModel } from "./model.ts";
import { parseRoute, type Route } from "./route.ts";
import { moduleTarget } from "./target.ts";
import { viewModel } from "./testModels.ts";

vi.mock("./engine.ts", () => ({
  loadEngine: async () => ({}),
  projectLimits: async () => ({
    maxFiles: 100,
    maxFileBytes: 1 << 20,
    maxContentBytes: 1 << 20,
    maxPathBytes: 1024,
    maxPathTotalBytes: 1 << 16,
  }),
  packageLayout: async () => ({ error: "no packages in these tests" }),
  projectView: async (file: string) => ({ status: "evaluated", view: modelOf(file) }),
}));

function modelOf(file: string): ViewModel {
  return viewModel([], { module: { id: `r.${file}`, file, package: "r" } });
}

/** The files of the fake repository at each ref; a ref not listed does not resolve. */
const repository: Record<string, Record<string, string>> = {
  main: { "a.gcl": "param n = 1;\n", "notes.txt": "" },
  old: { "a.gcl": "param n = 0;\n" },
  // The entry is gone at `feat`: the load fails after the ref resolved.
  feat: { "b.gcl": "param m = 1;\n" },
};

const github = {
  repo: async () => ({ defaultBranch: "main" }),
  commit: async (_owner: string, _repo: string, ref: string): Promise<Commit> => {
    if (!(ref in repository)) throw new Error(`no ref ${ref}`);
    return { sha: ref };
  },
  tree: async (_owner: string, _repo: string, commit: Commit): Promise<Tree> => ({
    entries: Object.keys(repository[commit.sha] ?? {}).map((path) => ({ path, size: null })),
  }),
  read: async (_owner: string, _repo: string, commit: Commit, path: string): Promise<string> =>
    repository[commit.sha]?.[path] ?? "",
  changed: async () => new Set(["a.gcl"]),
} as unknown as GitHub;

let followHash: () => void = () => {};
const location = { hash: "" };

beforeEach(() => {
  vi.stubGlobal("window", {
    addEventListener: (type: string, listener: () => void) => {
      if (type === "hashchange") followHash = listener;
    },
  });
  vi.stubGlobal("location", location);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Let the host's load run to its end. */
async function settled(): Promise<void> {
  for (let i = 0; i < 50; i += 1) await Promise.resolve();
}

function route(hash: string): Route {
  const parsed = parseRoute(hash);
  if (parsed === null) throw new Error(`not a route: ${hash}`);
  return parsed;
}

describe("GitHubHost", () => {
  test("a failed load leaves the files, the comparison and the patch's ref as read", async () => {
    // MDL-6: the ref, the files and the comparison were published before the
    // load succeeded, so a failed one mislabelled the copied patch.
    const host = new GitHubHost(route("#o/r@old...main/a.gcl"), Promise.resolve(github));
    await settled();
    expect(host.files).toEqual(["a.gcl", "notes.txt"]);
    const compare = host.compare;
    expect(compare?.ref).toBe("old");
    const model = host.model(moduleTarget("r.a.gcl"));
    if (model === null) throw new Error("the entry was not projected");
    const edit = await host.edit(
      model,
      {
        file: "a.gcl",
        revision: host.projectedAt(model) ?? 0,
        range: { start: { line: 0, character: 10 }, end: { line: 0, character: 11 } },
        text: "2",
      },
      [moduleTarget("r.a.gcl")],
    );
    expect(edit.ok).toBe(true);
    const before = host.patch();
    expect(before.text.split("\n")[0]).toBe("# graphcal what-if: o/r @ main");

    location.hash = "#o/r@feat/a.gcl";
    followHash();
    await settled();
    expect(host.files).toEqual(["a.gcl", "notes.txt"]);
    expect(host.compare).toBe(compare);
    expect(host.patch()).toEqual(before);
  });
});

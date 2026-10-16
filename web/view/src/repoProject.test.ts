import { describe, expect, test } from "vite-plus/test";
import type { ProjectLimits } from "./engine.ts";
import type { TreeEntry } from "./github.ts";
import {
  locateProject,
  planProject,
  ProjectPlanError,
  type ProjectLocation,
  type ProjectShape,
} from "./repoProject.ts";

/** The engine's limits as `crates/graphcal-wasm/src/project.rs` defines them. */
const LIMITS: ProjectLimits = {
  maxFiles: 16,
  maxFileBytes: 256 * 1024,
  maxContentBytes: 1024 * 1024,
  maxPathBytes: 1024,
  maxPathTotalBytes: 16 * 1024,
};

function tree(paths: Record<string, number>): TreeEntry[] {
  const entries: TreeEntry[] = [];
  for (const [path, size] of Object.entries(paths)) {
    entries.push({ path, type: "blob", sha: `sha-${path}`, size });
  }
  entries.push({ path: "src", type: "tree", sha: "sha-src", size: null });
  return entries;
}

/** The shape of a located project, its manifest declaring `name` (default `power_project`) in `src`. */
function shapeOf(location: ProjectLocation, name = "power_project"): ProjectShape {
  return location.kind === "package"
    ? { ...location, layout: { name, sourceDir: "src" } }
    : location;
}

function plan(entries: TreeEntry[], file: string, name?: string): string[] {
  return planProject(entries, shapeOf(locateProject(entries, file), name), LIMITS).files.map(
    (f) => f.relative,
  );
}

describe("locateProject", () => {
  const repo = tree({
    "graphcal.toml": 30,
    "src/power_project/power_max.gcl": 100,
    "examples/other/graphcal.toml": 20,
    "examples/other/src/other/main.gcl": 40,
    "a/b/one.gcl": 1,
  });

  test("the nearest manifest above the file is the root", () => {
    expect(locateProject(repo, "src/power_project/power_max.gcl")).toEqual({
      kind: "package",
      root: "",
      entry: "src/power_project/power_max.gcl",
      manifest: "graphcal.toml",
    });
    expect(locateProject(repo, "examples/other/src/other/main.gcl")).toEqual({
      kind: "package",
      root: "examples/other",
      entry: "src/other/main.gcl",
      manifest: "examples/other/graphcal.toml",
    });
  });

  test("rejects a missing file and a non-.gcl file", () => {
    expect(() => locateProject(repo, "src/missing.gcl")).toThrow(ProjectPlanError);
    expect(() => locateProject(repo, "graphcal.toml")).toThrow(/not a \.gcl file/);
  });
});

describe("planProject", () => {
  const repo = tree({
    "README.md": 10,
    "graphcal.toml": 30,
    "src/power_project.gcl": 10,
    "src/power_project/power_max.gcl": 100,
    "src/power_project/sub/eps.gcl": 80,
    "src/power_project/notes.txt": 5,
    "src/elsewhere/x.gcl": 5,
    "examples/other/graphcal.toml": 20,
    "examples/other/src/other/main.gcl": 40,
    "scratch/alone.gcl": 12,
  });

  test("a package brings its manifest and its namespace, not every file under the root", () => {
    expect(plan(repo, "src/power_project/power_max.gcl")).toEqual([
      "graphcal.toml",
      "src/power_project.gcl",
      "src/power_project/power_max.gcl",
      "src/power_project/sub/eps.gcl",
    ]);
  });

  test("an entry outside the namespace brings the files beside it", () => {
    expect(plan(repo, "scratch/alone.gcl")).toEqual([
      "graphcal.toml",
      "scratch/alone.gcl",
      "src/power_project.gcl",
      "src/power_project/power_max.gcl",
      "src/power_project/sub/eps.gcl",
    ]);
  });

  test("the source directory comes from the manifest", () => {
    const custom = tree({
      "graphcal.toml": 30,
      "gcl/lib/main.gcl": 1,
      "src/lib/stale.gcl": 1,
    });
    const shape: ProjectShape = {
      ...(locateProject(custom, "gcl/lib/main.gcl") as Extract<
        ProjectLocation,
        { kind: "package" }
      >),
      layout: { name: "lib", sourceDir: "gcl" },
    };
    expect(planProject(custom, shape, LIMITS).files.map((f) => f.path)).toEqual([
      "gcl/lib/main.gcl",
      "graphcal.toml",
    ]);
  });

  test("a file in a nested project gets that project", () => {
    const plan = planProject(
      repo,
      shapeOf(locateProject(repo, "examples/other/src/other/main.gcl"), "other"),
      LIMITS,
    );
    expect(plan.root).toBe("examples/other");
    expect(plan.entry).toBe("src/other/main.gcl");
    expect(plan.files).toEqual([
      { path: "examples/other/graphcal.toml", relative: "graphcal.toml" },
      { path: "examples/other/src/other/main.gcl", relative: "src/other/main.gcl" },
    ]);
  });

  test("without a manifest: the file's directory, not its subdirectories", () => {
    const entries = tree({
      "a/b/one.gcl": 1,
      "a/b/two.gcl": 1,
      "a/b/c/three.gcl": 1,
      "a/c.gcl": 1,
    });
    expect(plan(entries, "a/b/one.gcl")).toEqual(["one.gcl", "two.gcl"]);
  });

  test("the files beside the entry are left out when they do not fit", () => {
    // tests/fixtures/valid holds far more files than the engine takes.
    const many: Record<string, number> = {};
    for (let i = 0; i < 185; i++) many[`fixtures/f${String(i).padStart(3, "0")}.gcl`] = 1;
    expect(plan(tree(many), "fixtures/f007.gcl")).toEqual(["f007.gcl"]);
  });

  test("the file count includes the manifest and is checked before downloading", () => {
    const files: Record<string, number> = { "graphcal.toml": 1 };
    for (let i = 0; i < 16; i++) files[`src/p/m${i}.gcl`] = 1;
    expect(() => plan(tree(files), "src/p/m0.gcl", "p")).toThrow(
      /17 files \(the manifest included\); the browser engine takes up to 16/,
    );
    delete files["src/p/m15.gcl"];
    expect(plan(tree(files), "src/p/m0.gcl", "p")).toHaveLength(16);
  });

  test("the entry's path counts toward the total path length, as the engine counts it", () => {
    const limits = { ...LIMITS, maxPathTotalBytes: 2 * "one.gcl".length };
    const entries = tree({ "one.gcl": 1 });
    const shape = shapeOf(locateProject(entries, "one.gcl"));
    expect(() => planProject(entries, shape, { ...limits, maxPathTotalBytes: 13 })).toThrow(
      /too long/,
    );
    expect(planProject(entries, shape, limits).files).toHaveLength(1);
  });

  test("rejects oversize sources", () => {
    const big = tree({ "big.gcl": 300 * 1024 });
    expect(() => planProject(big, shapeOf(locateProject(big, "big.gcl")), LIMITS)).toThrow(
      /256 KiB/,
    );
    const files: Record<string, number> = { "graphcal.toml": 1 };
    for (let i = 0; i < 5; i++) files[`src/p/f${i}.gcl`] = 250 * 1024;
    expect(() => plan(tree(files), "src/p/f0.gcl", "p")).toThrow(/1024 KiB/);
  });
});

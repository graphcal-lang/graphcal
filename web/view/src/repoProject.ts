// Which files of a repository make up the project a `.gcl` file belongs to.
// The nearest `graphcal.toml` above the file is the project root (the file's
// own directory when there is none). A package's modules live in its
// namespace, `<source_dir>/<name>.gcl` and under `<source_dir>/<name>/`, as
// its manifest declares; only those, the manifest and the entry are needed.
// The other `.gcl` files beside the entry come along when they fit, so the
// explorer can open them without reading the project again. Everything is
// checked against the browser engine's limits before anything is downloaded.
import type { PackageLayout, ProjectLimits } from "./engine.ts";
import type { TreeEntry } from "./github.ts";

const MANIFEST = "graphcal.toml";

export class ProjectPlanError extends Error {}

/** Where the project around a file is, before its manifest is read. */
export type ProjectLocation =
  | {
      kind: "package";
      /** Repository directory of the project root: "" for the repository root. */
      root: string;
      /** The entry file, relative to the root. */
      entry: string;
      /** Repository path of the manifest. */
      manifest: string;
    }
  /** No manifest above the file: its directory is the root. */
  | { kind: "standalone"; root: string; entry: string };

/** A located project with its manifest read. */
export type ProjectShape =
  | (Extract<ProjectLocation, { kind: "package" }> & { layout: PackageLayout })
  | Extract<ProjectLocation, { kind: "standalone" }>;

export interface ProjectPlan {
  /** Repository directory of the project root: "" for the repository root. */
  root: string;
  /** The entry file, relative to the root. */
  entry: string;
  /** Every file to download: repository path and root-relative path, in path order. */
  files: { path: string; relative: string }[];
}

function dirname(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

/** `name` under the repository directory `dir` (`""` is the repository root). */
export function join(dir: string, name: string): string {
  return dir === "" ? name : `${dir}/${name}`;
}

function within(dir: string, path: string): boolean {
  return dir === "" || path.startsWith(`${dir}/`);
}

function relativeTo(root: string, path: string): string {
  return root === "" ? path : path.slice(root.length + 1);
}

function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

function blobSizes(entries: readonly TreeEntry[]): Map<string, number | null> {
  const blobs = new Map<string, number | null>();
  for (const entry of entries) if (entry.type === "blob") blobs.set(entry.path, entry.size);
  return blobs;
}

/** The project around `file`: its root, and its manifest if it has one. */
export function locateProject(entries: readonly TreeEntry[], file: string): ProjectLocation {
  const blobs = blobSizes(entries);
  if (!blobs.has(file)) throw new ProjectPlanError(`${file} is not in the repository at this ref`);
  if (!file.endsWith(".gcl")) throw new ProjectPlanError(`${file} is not a .gcl file`);
  for (let dir = dirname(file); ; dir = dirname(dir)) {
    const manifest = join(dir, MANIFEST);
    if (blobs.has(manifest)) {
      return { kind: "package", root: dir, entry: relativeTo(dir, file), manifest };
    }
    if (dir === "") break;
  }
  const root = dirname(file);
  return { kind: "standalone", root, entry: relativeTo(root, file) };
}

/** The sizes a request has used so far, as the engine counts them. */
interface Budget {
  files: number;
  contentBytes: number;
  /** The entry path and every file path. */
  pathBytes: number;
}

/** `budget` with one more file; throws when the engine would refuse the file. */
function addFile(
  budget: Budget,
  path: string,
  relative: string,
  size: number,
  limits: ProjectLimits,
): Budget {
  if (size > limits.maxFileBytes) {
    throw new ProjectPlanError(
      `${path} is ${kib(size)}; the browser engine takes files up to ${kib(limits.maxFileBytes)}`,
    );
  }
  const pathBytes = utf8Length(relative);
  if (pathBytes > limits.maxPathBytes) {
    throw new ProjectPlanError(`the path of ${path} is too long`);
  }
  return {
    files: budget.files + 1,
    contentBytes: budget.contentBytes + size,
    pathBytes: budget.pathBytes + pathBytes,
  };
}

/** Throws when the totals of `budget` exceed the engine's limits. */
function checkTotals(budget: Budget, limits: ProjectLimits): void {
  if (budget.files > limits.maxFiles) {
    throw new ProjectPlanError(
      `the project has ${budget.files} files (the manifest included); the browser engine takes up to ${limits.maxFiles}`,
    );
  }
  if (budget.contentBytes > limits.maxContentBytes) {
    throw new ProjectPlanError(
      `the project is ${kib(budget.contentBytes)} of source; the browser engine takes up to ${kib(limits.maxContentBytes)}`,
    );
  }
  if (budget.pathBytes > limits.maxPathTotalBytes) {
    throw new ProjectPlanError("the project's file paths are too long for the browser engine");
  }
}

/** The files to download for the project `shape`, within the engine's `limits`. */
export function planProject(
  entries: readonly TreeEntry[],
  shape: ProjectShape,
  limits: ProjectLimits,
): ProjectPlan {
  const blobs = blobSizes(entries);
  const { root } = shape;
  const entryPath = join(root, shape.entry);

  // Nested projects (a manifest below the root) are other projects.
  const nested = [...blobs.keys()]
    .filter((path) => path === MANIFEST || path.endsWith(`/${MANIFEST}`))
    .map(dirname)
    .filter((dir) => dir !== root && within(root, dir));
  const inProject = (path: string): boolean =>
    within(root, path) && !nested.some((dir) => within(dir, path));

  const required = new Set<string>([entryPath]);
  if (shape.kind === "package") {
    required.add(shape.manifest);
    const namespaceDir = join(join(root, shape.layout.sourceDir), shape.layout.name);
    const namespaceFile = `${namespaceDir}.gcl`;
    for (const path of blobs.keys()) {
      if (!path.endsWith(".gcl") || !inProject(path)) continue;
      if (path === namespaceFile || within(namespaceDir, path)) required.add(path);
    }
  }
  const entryDir = dirname(entryPath);
  const beside = [...blobs.keys()].filter(
    (path) => path.endsWith(".gcl") && dirname(path) === entryDir && !required.has(path),
  );

  const gather = (paths: Iterable<string>, start: Budget): Budget => {
    let budget = start;
    for (const path of paths) {
      budget = addFile(budget, path, relativeTo(root, path), blobs.get(path) ?? 0, limits);
    }
    checkTotals(budget, limits);
    return budget;
  };
  // The engine counts the entry's path besides the files' paths.
  const needed = gather(required, {
    files: 0,
    contentBytes: 0,
    pathBytes: utf8Length(shape.entry),
  });
  // The files beside the entry are a convenience: all of them when they fit, else none.
  let paths = [...required];
  try {
    gather(beside, needed);
    paths = [...required, ...beside];
  } catch (error) {
    if (!(error instanceof ProjectPlanError)) throw error;
  }

  const files = paths
    .map((path) => ({ path, relative: relativeTo(root, path) }))
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return { root, entry: shape.entry, files };
}

function kib(bytes: number): string {
  return `${Math.ceil(bytes / 1024)} KiB`;
}

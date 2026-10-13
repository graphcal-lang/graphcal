// Which files of a repository make up the project a `.gcl` file belongs to,
// by the loader's rule: the nearest `graphcal.toml` above the file is the
// project root (the file's own directory when there is none), and every
// `.gcl` under the root that is not inside a nested project is part of it.
// Sizes are checked against the browser engine's limits before anything is
// downloaded.
import type { TreeEntry } from "./github.ts";

/** Limits of `crates/graphcal-wasm/src/project.rs`. */
const MAX_FILE_BYTES = 256 * 1024;
const MAX_CONTENT_BYTES = 1024 * 1024;
const MAX_PATH_BYTES = 1024;
const MAX_PATH_TOTAL_BYTES = 16 * 1024;

const MANIFEST = "graphcal.toml";

export class ProjectPlanError extends Error {}

export interface ProjectPlan {
  /** Repository directory of the project root: "" for the repository root. */
  root: string;
  /** The entry file, relative to the root. */
  entry: string;
  /** Every file of the project: repository path and root-relative path. */
  files: { path: string; relative: string }[];
}

function dirname(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

function join(dir: string, name: string): string {
  return dir === "" ? name : `${dir}/${name}`;
}

function within(dir: string, path: string): boolean {
  return dir === "" || path.startsWith(`${dir}/`);
}

function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** The project around `file`, from the repository's tree. */
export function planProject(entries: readonly TreeEntry[], file: string): ProjectPlan {
  const blobs = new Map<string, number | null>();
  for (const entry of entries) if (entry.type === "blob") blobs.set(entry.path, entry.size);
  if (!blobs.has(file)) throw new ProjectPlanError(`${file} is not in the repository at this ref`);
  if (!file.endsWith(".gcl")) throw new ProjectPlanError(`${file} is not a .gcl file`);

  // The nearest manifest above the file decides the root.
  let root = dirname(file);
  for (let dir = dirname(file); ; dir = dirname(dir)) {
    if (blobs.has(join(dir, MANIFEST))) {
      root = dir;
      break;
    }
    if (dir === "") break;
  }

  // Nested projects (a manifest below the root) are other projects.
  const nested = [...blobs.keys()]
    .filter((path) => path.endsWith(`/${MANIFEST}`) || path === MANIFEST)
    .map(dirname)
    .filter((dir) => dir !== root && within(root, dir));
  const files: ProjectPlan["files"] = [];
  let contentBytes = 0;
  let pathBytes = 0;
  for (const [path, size] of blobs) {
    if (!within(root, path)) continue;
    if (nested.some((dir) => within(dir, path))) continue;
    const relative = root === "" ? path : path.slice(root.length + 1);
    if (!(relative.endsWith(".gcl") || relative === MANIFEST)) continue;
    const bytes = size ?? 0;
    if (bytes > MAX_FILE_BYTES) {
      throw new ProjectPlanError(
        `${path} is ${kib(bytes)}; the browser engine takes files up to ${kib(MAX_FILE_BYTES)}`,
      );
    }
    const relBytes = utf8Length(relative);
    if (relBytes > MAX_PATH_BYTES) throw new ProjectPlanError(`the path of ${path} is too long`);
    contentBytes += bytes;
    pathBytes += relBytes;
    files.push({ path, relative });
  }
  if (contentBytes > MAX_CONTENT_BYTES) {
    throw new ProjectPlanError(
      `the project is ${kib(contentBytes)} of source; the browser engine takes up to ${kib(MAX_CONTENT_BYTES)}`,
    );
  }
  if (pathBytes > MAX_PATH_TOTAL_BYTES) {
    throw new ProjectPlanError("the project has too many files for the browser engine");
  }
  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  const entry = root === "" ? file : file.slice(root.length + 1);
  return { root, entry, files };
}

function kib(bytes: number): string {
  return `${Math.ceil(bytes / 1024)} KiB`;
}

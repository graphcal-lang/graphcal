// The address of a file on GitHub, as the viewer's URL fragment carries it:
//
//   #owner/repo@ref/path/to/file.gcl
//   #owner/repo@base...head/path/to/file.gcl
//
// `@ref` is a branch, tag or commit; left out, the repository's default
// branch. `base...head` (as GitHub's compare pages write it) shows the file
// at `head` with what changed since `base` marked. Refs and path segments are
// percent-encoded (a ref with a slash is `feat%2Fx`, since the path follows
// it). The fragment never reaches a server.

/** Which ref of the repository an address reads. */
export type RouteRef =
  /** The repository's default branch. */
  | { kind: "default" }
  /** A branch, tag or commit. */
  | { kind: "ref"; ref: string }
  /** `head`, with what changed since `base` marked. */
  | { kind: "compare"; base: string; head: string };

export interface Route {
  owner: string;
  repo: string;
  at: RouteRef;
  /** Repository path of the `.gcl` file, decoded. */
  path: string;
}

/** The ref shown; null for the default branch. */
export function headRef(at: RouteRef): string | null {
  switch (at.kind) {
    case "default":
      return null;
    case "ref":
      return at.ref;
    case "compare":
      return at.head;
  }
}

/** The ref compared against, when the address is a comparison. */
export function baseRef(at: RouteRef): string | null {
  return at.kind === "compare" ? at.base : null;
}

/** Whether two addresses read the same refs. */
export function sameRef(a: RouteRef, b: RouteRef): boolean {
  return headRef(a) === headRef(b) && baseRef(a) === baseRef(b);
}

/** `at` showing `ref` instead, still compared against the same base. */
export function withHead(at: RouteRef, ref: string): RouteRef {
  return at.kind === "compare" ? { ...at, head: ref } : { kind: "ref", ref };
}

/** A GitHub user or organization name. */
const OWNER = /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/;
/** A GitHub repository name: `.github` is one, `.` and `..` are not. */
const REPO = /^[A-Za-z0-9._-]+$/;

function isRepoName(name: string): boolean {
  return REPO.test(name) && name !== "." && name !== "..";
}

/** `text` percent-decoded; null when it is not valid percent-encoding. */
function decode(text: string): string | null {
  try {
    return decodeURIComponent(text);
  } catch {
    return null;
  }
}

/** The decoded repository path, or null when a segment is empty, `.`, `..` or not decodable. */
function decodePath(text: string): string | null {
  const parts: string[] = [];
  for (const raw of text.split("/")) {
    const part = decode(raw);
    if (part === null || part === "" || part === "." || part === ".." || part.includes("/")) {
      return null;
    }
    parts.push(part);
  }
  return parts.join("/");
}

function decodeRef(text: string): string | null {
  const ref = decode(text);
  return ref === null || ref === "" ? null : ref;
}

function parseRefs(text: string | null): RouteRef | null {
  if (text === null) return { kind: "default" };
  if (!text.includes("...")) {
    const ref = decodeRef(text);
    return ref === null ? null : { kind: "ref", ref };
  }
  const [rawBase, rawHead] = splitOnce(text, "...");
  const base = decodeRef(rawBase);
  const head = rawHead === null ? null : decodeRef(rawHead);
  return base === null || head === null ? null : { kind: "compare", base, head };
}

/** The route in a fragment (`#owner/repo@ref/path`), or null when it is not one. */
export function parseRoute(hash: string): Route | null {
  const text = hash.startsWith("#") ? hash.slice(1) : hash;
  if (text.length === 0) return null;
  const [owner, rest] = splitOnce(text, "/");
  if (rest === null || !OWNER.test(owner)) return null;
  const [repoAndRef, rawPath] = splitOnce(rest, "/");
  if (rawPath === null) return null;
  const path = decodePath(rawPath);
  if (path === null || !path.endsWith(".gcl")) return null;
  const [repo, refs] = splitOnce(repoAndRef, "@");
  if (!isRepoName(repo)) return null;
  const at = parseRefs(refs);
  return at === null ? null : { owner, repo, at, path };
}

function formatRefs(at: RouteRef): string {
  switch (at.kind) {
    case "default":
      return "";
    case "ref":
      return `@${encodeURIComponent(at.ref)}`;
    case "compare":
      return `@${encodeURIComponent(at.base)}...${encodeURIComponent(at.head)}`;
  }
}

/** The fragment for a route, including the leading `#`. */
export function formatRoute(route: Route): string {
  const path = route.path.split("/").map(encodeURIComponent).join("/");
  return `#${route.owner}/${route.repo}${formatRefs(route.at)}/${path}`;
}

/** A GitHub file URL; its query (`?plain=1`) and line anchor (`#L10`) are not part of the address. */
const GITHUB_URL =
  /^(?:https?:\/\/)?(?:www\.)?github\.com\/([^/]+)\/([^/]+)\/(?:blob|tree|raw)\/([^/]+)\/([^?#]+)(?:\?[^#]*)?(?:#.*)?$/;

/**
 * The route in what a person pastes: a GitHub file URL
 * (`https://github.com/o/r/blob/ref/path.gcl`), the fragment form with or
 * without its `#`, or the short `owner/repo/path.gcl` (default branch).
 */
export function routeFromText(text: string): Route | null {
  const trimmed = text.trim();
  const url = GITHUB_URL.exec(trimmed);
  if (url) {
    const [, owner = "", repo = "", ref = "", path = ""] = url;
    // A branch with a slash is ambiguous in a GitHub URL; one written encoded is taken as is.
    return parseRoute(`${owner}/${repo.replace(/\.git$/, "")}@${ref}/${path}`);
  }
  return parseRoute(trimmed.startsWith("#") ? trimmed : `#${trimmed}`);
}

/** `repo @ ref`, or `repo @ base…head` for a comparison (the owner is in the address and the explorer's tooltip). */
export function describeRoute(
  route: Route,
  resolvedRef: string | null = headRef(route.at),
): string {
  const ref = resolvedRef ?? "default branch";
  const base = baseRef(route.at);
  return base === null ? `${route.repo} @ ${ref}` : `${route.repo} @ ${base}…${ref}`;
}

function splitOnce(text: string, separator: string): [string, string | null] {
  const at = text.indexOf(separator);
  return at < 0 ? [text, null] : [text.slice(0, at), text.slice(at + separator.length)];
}

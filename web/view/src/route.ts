// The address of a file on GitHub, as the viewer's URL fragment carries it:
//
//   #owner/repo@ref/path/to/file.gcl
//   #owner/repo@base...head/path/to/file.gcl
//
// `@ref` is a branch, tag or commit; left out, the repository's default
// branch. `base...head` (as GitHub's compare pages write it) shows the file
// at `head` with what changed since `base` marked. A ref with a slash in it
// is written percent-encoded (`feat%2Fx`), since the path follows it. The
// fragment never reaches a server.

export interface Route {
  owner: string;
  repo: string;
  /** The ref shown; null: the default branch. */
  ref: string | null;
  /** The ref compared against, when the address is a comparison. */
  base: string | null;
  /** Repository path of the `.gcl` file. */
  path: string;
}

const NAME = /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/;

/** The route in a fragment (`#owner/repo@ref/path`), or null when it is not one. */
export function parseRoute(hash: string): Route | null {
  const text = hash.startsWith("#") ? hash.slice(1) : hash;
  if (text.length === 0) return null;
  const [owner, rest] = splitOnce(text, "/");
  if (rest === null || !NAME.test(owner)) return null;
  const [repoAndRef, path] = splitOnce(rest, "/");
  if (path === null || path.length === 0 || !path.endsWith(".gcl")) return null;
  if (path.split("/").some((part) => part === "" || part === "." || part === "..")) return null;
  const [repo, refs] = splitOnce(repoAndRef, "@");
  if (!NAME.test(repo)) return null;
  if (refs === null) return { owner, repo, ref: null, base: null, path };
  const [base, head] = refs.includes("...") ? splitOnce(refs, "...") : [null, refs];
  if (head === null || head.length === 0 || base === "") return null;
  return {
    owner,
    repo,
    ref: decodeURIComponent(head),
    base: base === null ? null : decodeURIComponent(base),
    path,
  };
}

/** The fragment for a route, including the leading `#`. */
export function formatRoute(route: Route): string {
  const head = route.ref === null ? "" : encodeURIComponent(route.ref);
  const refs =
    route.base !== null && route.ref !== null
      ? `@${encodeURIComponent(route.base)}...${head}`
      : route.ref === null
        ? ""
        : `@${head}`;
  return `#${route.owner}/${route.repo}${refs}/${route.path}`;
}

/**
 * The route in what a person pastes: a GitHub file URL
 * (`https://github.com/o/r/blob/ref/path.gcl`), the fragment form with or
 * without its `#`, or the short `owner/repo/path.gcl` (default branch).
 */
export function routeFromText(text: string): Route | null {
  const trimmed = text.trim();
  const url =
    /^(?:https?:\/\/)?(?:www\.)?github\.com\/([^/]+)\/([^/]+)\/(?:blob|tree|raw)\/([^/]+)\/(.+)$/.exec(
      trimmed,
    );
  if (url) {
    const [, owner = "", repo = "", ref = "", path = ""] = url;
    // A branch with a slash is ambiguous in a GitHub URL; one written encoded is taken as is.
    return parseRoute(`${owner}/${repo.replace(/\.git$/, "")}@${ref}/${path}`);
  }
  return parseRoute(trimmed.startsWith("#") ? trimmed : `#${trimmed}`);
}

/** `repo @ ref`, or `repo @ base…head` for a comparison (the owner is in the address and the explorer's tooltip). */
export function describeRoute(route: Route, resolvedRef: string | null = route.ref): string {
  const ref = resolvedRef ?? "default branch";
  return route.base === null ? `${route.repo} @ ${ref}` : `${route.repo} @ ${route.base}…${ref}`;
}

function splitOnce(text: string, separator: string): [string, string | null] {
  const at = text.indexOf(separator);
  return at < 0 ? [text, null] : [text.slice(0, at), text.slice(at + separator.length)];
}

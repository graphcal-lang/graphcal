// The address of a file on GitHub, as the viewer's URL fragment carries it:
//
//   #owner/repo@ref/path/to/file.gcl
//
// `@ref` is a branch, tag or commit; left out, the repository's default
// branch. A ref with a slash in it is written percent-encoded (`feat%2Fx`),
// since the path follows it. The fragment never reaches a server.

export interface Route {
  owner: string;
  repo: string;
  /** null: the default branch. */
  ref: string | null;
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
  const [repo, ref] = splitOnce(repoAndRef, "@");
  if (!NAME.test(repo)) return null;
  if (ref !== null && ref.length === 0) return null;
  return { owner, repo, ref: ref === null ? null : decodeURIComponent(ref), path };
}

/** The fragment for a route, including the leading `#`. */
export function formatRoute(route: Route): string {
  const ref = route.ref === null ? "" : `@${encodeURIComponent(route.ref)}`;
  return `#${route.owner}/${route.repo}${ref}/${route.path}`;
}

/** `repo @ ref`, for the bar (the owner is in the address and the explorer's tooltip). */
export function describeRoute(route: Route, resolvedRef: string | null = route.ref): string {
  const ref = resolvedRef ?? "default branch";
  return `${route.repo} @ ${ref}`;
}

function splitOnce(text: string, separator: string): [string, string | null] {
  const at = text.indexOf(separator);
  return at < 0 ? [text, null] : [text.slice(0, at), text.slice(at + separator.length)];
}

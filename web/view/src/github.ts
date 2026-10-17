// The GitHub REST calls the viewer makes, over `fetch`: the repository, the
// commit a ref names, that commit's tree, and the raw text of a blob. A load
// resolves its ref to a commit once and reads everything by hash from there,
// so a push in between cannot mix two commits. Responses are cached by ETag
// so a reload asks "has it changed?" (a 304 is free of the rate limit); a
// blob, named by its content's hash, is fetched once. Failures are classified
// so the shell can tell "not found" from "sign in" from "wait".
import { z } from "zod";

const API = "https://api.github.com";
const API_VERSION = "2022-11-28";

export type GitHubFailure =
  /** 404: the repository, ref or path does not exist, or the viewer may not see it. */
  | "not_found"
  /** The repository has no branch, tag or commit of that name. */
  | "ref_not_found"
  /** 401: the token was rejected. */
  | "unauthorized"
  /** 403 / 429 with the rate limit used up. */
  | "rate_limited"
  /** 403 for another reason (SSO, blocked). */
  | "forbidden"
  /** The request never got a response. */
  | "network"
  | "other";

export class GitHubError extends Error {
  readonly kind: GitHubFailure;
  readonly status: number | null;
  /** When the rate limit resets, if that is what failed. */
  readonly resetAt: Date | null;
  constructor(kind: GitHubFailure, message: string, status: number | null, resetAt: Date | null) {
    super(message);
    this.kind = kind;
    this.status = status;
    this.resetAt = resetAt;
  }
}

const repoSchema = z.object({
  full_name: z.string(),
  default_branch: z.string(),
  private: z.boolean(),
});
export interface RepoInfo {
  fullName: string;
  defaultBranch: string;
  private: boolean;
}

const commitSchema = z.object({
  sha: z.string(),
  commit: z.object({ tree: z.object({ sha: z.string() }) }),
});
/** The commit a ref names, at the time it was asked. */
export interface Commit {
  sha: string;
  /** The commit's root tree. */
  treeSha: string;
}

const treeSchema = z.object({
  sha: z.string(),
  truncated: z.boolean(),
  tree: z.array(
    z.object({
      path: z.string(),
      type: z.enum(["blob", "tree", "commit"]),
      sha: z.string(),
      size: z.number().optional(),
    }),
  ),
});
export interface TreeEntry {
  path: string;
  type: "blob" | "tree" | "commit";
  /** The object's hash: two refs hold the same content when it agrees. */
  sha: string;
  /** Bytes, for blobs. */
  size: number | null;
}
export interface Tree {
  sha: string;
  /** GitHub stopped listing: the repository is too large for one request. */
  truncated: boolean;
  entries: TreeEntry[];
}

const namedSchema = z.array(z.object({ name: z.string() }));
const pullsSchema = z.array(
  z.object({
    number: z.number(),
    title: z.string(),
    head: z.object({
      ref: z.string(),
      sha: z.string(),
      repo: z.object({ full_name: z.string() }).nullable(),
    }),
  }),
);
export interface PullRef {
  number: number;
  title: string;
  /** The ref to read the pull request's head at: its branch when it lives in this repository, else its commit. */
  ref: string;
}
/** The lists of refs a repository offers. */
export type RefList = "branches" | "tags" | "pulls";
/** A list GitHub would not give (pull requests need their own token permission). */
export interface RefsFailure {
  list: RefList;
  /** The failure's class, when GitHub answered. */
  kind: GitHubFailure | null;
  message: string;
}
/** The refs a repository offers to switch to; a list that failed is empty and named in `failures`. */
export interface Refs {
  branches: string[];
  tags: string[];
  pulls: PullRef[];
  failures: RefsFailure[];
}

interface Cached {
  etag: string;
  body: string;
}

const RAW = "application/vnd.github.raw+json";
const JSON_ = "application/vnd.github+json";

export class GitHub {
  private readonly cache = new Map<string, Cached>();
  /** Blob texts by sha: a sha names one content, whichever repository holds it. */
  private readonly blobs = new Map<string, Promise<string>>();
  private readonly token: string | null;
  private readonly fetchFn: typeof fetch;
  constructor(token: string | null, fetchFn: typeof fetch = (input, init) => fetch(input, init)) {
    this.token = token;
    this.fetchFn = fetchFn;
  }

  async repo(owner: string, repo: string): Promise<RepoInfo> {
    const data = repoSchema.parse(JSON.parse(await this.get(`/repos/${owner}/${repo}`, JSON_)));
    return { fullName: data.full_name, defaultBranch: data.default_branch, private: data.private };
  }

  /** The commit `ref` (a branch, tag or commit) names now. */
  async commit(owner: string, repo: string, ref: string): Promise<Commit> {
    let body: string;
    try {
      body = await this.get(`/repos/${owner}/${repo}/commits/${encodeURIComponent(ref)}`, JSON_);
    } catch (error) {
      // GitHub answers 422 ("No commit found for SHA") for a name it does not know.
      if (error instanceof GitHubError && error.status === 422) {
        throw new GitHubError(
          "ref_not_found",
          `${owner}/${repo} has no branch, tag or commit ${ref}`,
          error.status,
          null,
        );
      }
      throw error;
    }
    const data = commitSchema.parse(JSON.parse(body));
    return { sha: data.sha, treeSha: data.commit.tree.sha };
  }

  /** Every path in the tree `treeSha` (a commit's `treeSha`), recursively. */
  async tree(owner: string, repo: string, treeSha: string): Promise<Tree> {
    const url = `/repos/${owner}/${repo}/git/trees/${encodeURIComponent(treeSha)}?recursive=1`;
    const data = treeSchema.parse(JSON.parse(await this.get(url, JSON_)));
    return {
      sha: data.sha,
      truncated: data.truncated,
      entries: data.tree.map((e) => ({
        path: e.path,
        type: e.type,
        sha: e.sha,
        size: e.size ?? null,
      })),
    };
  }

  /**
   * The text of the blob `sha` (a tree entry's `sha`). A blob's sha is the
   * hash of its content, so it is fetched once and shared by every tree
   * that holds it.
   */
  blob(owner: string, repo: string, sha: string): Promise<string> {
    const known = this.blobs.get(sha);
    if (known) return known;
    const text = this.get(`/repos/${owner}/${repo}/git/blobs/${encodeURIComponent(sha)}`, RAW);
    this.blobs.set(sha, text);
    text.catch(() => {
      if (this.blobs.get(sha) === text) this.blobs.delete(sha);
    });
    return text;
  }

  /**
   * Branches, tags and open pull requests (the first hundred of each). Each
   * list is read on its own: one that fails (pull requests, for a token
   * without that permission) leaves the others. Rejects when all three fail.
   */
  async refs(owner: string, repo: string): Promise<Refs> {
    const base = `/repos/${owner}/${repo}`;
    const fullName = `${owner}/${repo}`;
    const [branches, tags, pulls] = await Promise.allSettled([
      this.get(`${base}/branches?per_page=100`, JSON_).then((body) =>
        namedSchema.parse(JSON.parse(body)).map((b) => b.name),
      ),
      this.get(`${base}/tags?per_page=100`, JSON_).then((body) =>
        namedSchema.parse(JSON.parse(body)).map((t) => t.name),
      ),
      this.get(`${base}/pulls?state=open&per_page=100`, JSON_).then((body) =>
        pullsSchema.parse(JSON.parse(body)).map((p): PullRef => ({
          number: p.number,
          title: p.title,
          ref: p.head.repo?.full_name === fullName ? p.head.ref : p.head.sha,
        })),
      ),
    ]);
    if (
      branches.status === "rejected" &&
      tags.status === "rejected" &&
      pulls.status === "rejected"
    ) {
      throw branches.reason;
    }
    const failures: RefsFailure[] = [];
    const settled = <T>(list: RefList, result: PromiseSettledResult<T[]>): T[] => {
      if (result.status === "fulfilled") return result.value;
      const error: unknown = result.reason;
      failures.push({
        list,
        kind: error instanceof GitHubError ? error.kind : null,
        message: error instanceof Error ? error.message : String(error),
      });
      return [];
    };
    return {
      branches: settled("branches", branches),
      tags: settled("tags", tags),
      pulls: settled("pulls", pulls),
      failures,
    };
  }

  private async get(path: string, accept: string): Promise<string> {
    const url = `${API}${path}`;
    const headers: Record<string, string> = {
      Accept: accept,
      "X-GitHub-Api-Version": API_VERSION,
    };
    if (this.token !== null) headers.Authorization = `Bearer ${this.token}`;
    const key = `${accept} ${url}`;
    const cached = this.cache.get(key);
    if (cached) headers["If-None-Match"] = cached.etag;
    let response: Response;
    try {
      response = await this.fetchFn(url, { headers, signal: AbortSignal.timeout(30_000) });
    } catch (error) {
      const why = error instanceof Error ? error.message : String(error);
      throw new GitHubError("network", `GitHub did not answer (${why})`, null, null);
    }
    if (response.status === 304 && cached) return cached.body;
    if (!response.ok) throw failureOf(response, path);
    const body = await response.text();
    const etag = response.headers.get("ETag");
    if (etag !== null) this.cache.set(key, { etag, body });
    return body;
  }
}

function failureOf(response: Response, path: string): GitHubError {
  const { status } = response;
  const remaining = response.headers.get("X-RateLimit-Remaining");
  const reset = response.headers.get("X-RateLimit-Reset");
  if ((status === 403 || status === 429) && remaining === "0") {
    const resetAt = reset === null ? null : new Date(Number(reset) * 1000);
    const when = resetAt === null ? "" : ` until ${resetAt.toLocaleTimeString()}`;
    return new GitHubError("rate_limited", `GitHub rate limit reached${when}`, status, resetAt);
  }
  if (status === 404) return new GitHubError("not_found", `not found: ${path}`, status, null);
  if (status === 401)
    return new GitHubError("unauthorized", "GitHub rejected the token", status, null);
  if (status === 403) return new GitHubError("forbidden", `GitHub refused: ${path}`, status, null);
  return new GitHubError("other", `GitHub answered HTTP ${status} for ${path}`, status, null);
}

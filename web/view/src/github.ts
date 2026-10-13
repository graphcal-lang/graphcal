// The GitHub REST calls the viewer makes, over `fetch`: the repository, its
// tree at a ref, and the raw text of a file. Responses are cached by ETag so a
// reload asks "has it changed?" (a 304 is free of the rate limit). Failures
// are classified so the shell can tell "not found" from "sign in" from "wait".
import { z } from "zod";

const API = "https://api.github.com";
const API_VERSION = "2022-11-28";

export type GitHubFailure =
  /** 404: the repository, ref or path does not exist, or the viewer may not see it. */
  | "not_found"
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

const treeSchema = z.object({
  sha: z.string(),
  truncated: z.boolean(),
  tree: z.array(
    z.object({
      path: z.string(),
      type: z.enum(["blob", "tree", "commit"]),
      size: z.number().optional(),
    }),
  ),
});
export interface TreeEntry {
  path: string;
  type: "blob" | "tree" | "commit";
  /** Bytes, for blobs. */
  size: number | null;
}
export interface Tree {
  sha: string;
  /** GitHub stopped listing: the repository is too large for one request. */
  truncated: boolean;
  entries: TreeEntry[];
}

interface Cached {
  etag: string;
  body: string;
}

const RAW = "application/vnd.github.raw+json";
const JSON_ = "application/vnd.github+json";

export class GitHub {
  private readonly cache = new Map<string, Cached>();
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

  /** Every path in the repository at `ref` (a branch, tag or commit). */
  async tree(owner: string, repo: string, ref: string): Promise<Tree> {
    const url = `/repos/${owner}/${repo}/git/trees/${encodeURIComponent(ref)}?recursive=1`;
    const data = treeSchema.parse(JSON.parse(await this.get(url, JSON_)));
    return {
      sha: data.sha,
      truncated: data.truncated,
      entries: data.tree.map((e) => ({ path: e.path, type: e.type, size: e.size ?? null })),
    };
  }

  /** The text of a file at `ref`. */
  file(owner: string, repo: string, ref: string, path: string): Promise<string> {
    const encoded = path.split("/").map(encodeURIComponent).join("/");
    const url = `/repos/${owner}/${repo}/contents/${encoded}?ref=${encodeURIComponent(ref)}`;
    return this.get(url, RAW);
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

// GitHub, read through the Graphcal browser extension. The page cannot read
// github.com itself (a private repository needs the browser's sign-in, which
// no page gets), so each read is one message to the extension, which fetches
// github.com with the session cookies and answers with the data or why not.
// What is asked: the repository's default branch, the commit a ref names,
// every file path at a commit, a file's text at a commit, the paths that
// differ between two refs, and the refs to switch to. A load resolves its
// ref to a commit once and reads everything at that commit, so a push in
// between cannot mix two commits. A file is read once per commit.
import { z } from "zod";

export type GitHubFailure =
  /** The repository, ref or path does not exist, or the signed-in account cannot see it. */
  | "not_found"
  /** The repository has no branch, tag or commit of that name. */
  | "ref_not_found"
  /** The browser is not signed in to github.com. */
  | "signed_out"
  /** GitHub asked to slow down. */
  | "rate_limited"
  /** GitHub refused for another reason (SSO, blocked). */
  | "forbidden"
  /** GitHub, or the extension, did not answer. */
  | "network"
  /** The extension is not installed in this browser. */
  | "no_extension"
  | "other";

export class GitHubError extends Error {
  readonly kind: GitHubFailure;
  constructor(kind: GitHubFailure, message: string) {
    super(message);
    this.kind = kind;
  }
}

export interface RepoInfo {
  defaultBranch: string;
}

/** The commit a ref named, at the time it was asked. */
export interface Commit {
  sha: string;
}

export interface TreeEntry {
  /** Repository path of a file. */
  path: string;
  /** Bytes, when known; the extension's listing does not say. */
  size: number | null;
}
/** Every file at a commit. */
export interface Tree {
  entries: TreeEntry[];
}

export interface PullRef {
  number: number;
  title: string;
  /** The ref to read the pull request's head at. */
  ref: string;
}
/** The lists of refs a repository offers. */
export type RefList = "branches" | "tags" | "pulls";
/** A list that was not given. */
export interface RefsFailure {
  list: RefList;
  /** The failure's class, when it is one. */
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

/** One request to the extension, answered with whatever it sent back. Rejects when it is not there. */
export type Send = (request: Record<string, unknown>) => Promise<unknown>;

export const PROTOCOL_VERSION = 1;

const failureSchema = z.enum([
  "not_found",
  "ref_not_found",
  "signed_out",
  "rate_limited",
  "forbidden",
  "network",
  "other",
]);
const answerSchema = z.discriminatedUnion("ok", [
  z.object({ ok: z.literal(true), value: z.unknown() }),
  z.object({ ok: z.literal(false), kind: failureSchema, message: z.string() }),
]);

const repoSchema = z.object({ defaultBranch: z.string() });
const commitSchema = z.object({ sha: z.string().regex(/^[0-9a-f]{40}$/) });
const pathsSchema = z.object({ paths: z.array(z.string()) });
const textSchema = z.object({ text: z.string() });
const refsSchema = z.object({
  branches: z.array(z.string()),
  tags: z.array(z.string()),
  pulls: z.array(z.object({ number: z.number(), title: z.string(), ref: z.string() })),
  failures: z.array(
    z.object({
      list: z.enum(["branches", "tags", "pulls"]),
      kind: failureSchema.nullable(),
      message: z.string(),
    }),
  ),
});
const helloSchema = z.object({ version: z.string() });

export class GitHub {
  /** File texts by commit and path: a commit's file never changes. */
  private readonly texts = new Map<string, Map<string, Promise<string>>>();
  private readonly send: Send;
  constructor(send: Send) {
    this.send = send;
  }

  /** The extension's version: it is there. */
  async hello(): Promise<{ version: string }> {
    return this.call("hello", {}, helloSchema);
  }

  repo(owner: string, repo: string): Promise<RepoInfo> {
    return this.call("repo", { owner, repo }, repoSchema);
  }

  /** The commit `ref` (a branch, tag or commit) names now. */
  commit(owner: string, repo: string, ref: string): Promise<Commit> {
    return this.call("commit", { owner, repo, ref }, commitSchema);
  }

  /** Every file at `commit`. */
  async tree(owner: string, repo: string, commit: Commit): Promise<Tree> {
    const { paths } = await this.call("tree", { owner, repo, sha: commit.sha }, pathsSchema);
    return { entries: paths.map((path) => ({ path, size: null })) };
  }

  /** The text of the file `path` at `commit`, read once per commit. */
  read(owner: string, repo: string, commit: Commit, path: string): Promise<string> {
    let files = this.texts.get(commit.sha);
    if (!files) {
      files = new Map();
      this.texts.set(commit.sha, files);
    }
    const known = files.get(path);
    if (known) return known;
    const text = this.call("read", { owner, repo, sha: commit.sha, path }, textSchema).then(
      (answer) => answer.text,
    );
    files.set(path, text);
    text.catch(() => {
      if (files.get(path) === text) files.delete(path);
    });
    return text;
  }

  /** Repository paths whose content differs between `base` and `head` (what a pull request from `head` would change). */
  async changed(owner: string, repo: string, base: string, head: string): Promise<Set<string>> {
    const { paths } = await this.call("changed", { owner, repo, base, head }, pathsSchema);
    return new Set(paths);
  }

  /** The refs to switch to; a list the extension does not give is empty and named in `failures`. */
  refs(owner: string, repo: string): Promise<Refs> {
    return this.call("refs", { owner, repo }, refsSchema);
  }

  private async call<T>(
    method: string,
    fields: Record<string, unknown>,
    schema: z.ZodType<T>,
  ): Promise<T> {
    let raw: unknown;
    try {
      raw = await this.send({ v: PROTOCOL_VERSION, method, ...fields });
    } catch (error) {
      const why = error instanceof Error ? error.message : String(error);
      throw new GitHubError("network", `the Graphcal extension did not answer (${why})`);
    }
    const answer = answerSchema.safeParse(raw);
    if (!answer.success) {
      throw new GitHubError(
        "other",
        `the Graphcal extension answered ${method} in an unexpected form`,
      );
    }
    if (!answer.data.ok) throw new GitHubError(answer.data.kind, answer.data.message);
    const value = schema.safeParse(answer.data.value);
    if (!value.success) {
      throw new GitHubError(
        "other",
        `the Graphcal extension answered ${method} in an unexpected form`,
      );
    }
    return value.data;
  }
}

import { describe, expect, test, vi } from "vite-plus/test";
import { GitHub, GitHubError } from "./github.ts";

interface Call {
  url: string;
  headers: Record<string, string>;
}

/** A fetch that answers from a script of responses and records what it was asked. */
function fakeFetch(answers: ((call: Call) => Response)[]): { fetch: typeof fetch; calls: Call[] } {
  const calls: Call[] = [];
  const fetchFn: typeof fetch = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const call = { url, headers: (init?.headers ?? {}) as Record<string, string> };
    calls.push(call);
    const answer = answers.shift();
    if (!answer) throw new Error("unexpected request");
    return Promise.resolve(answer(call));
  }) as unknown as typeof fetch;
  return { fetch: fetchFn, calls };
}

function json(body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status: 200, headers });
}

describe("GitHub", () => {
  test("sends the token, the api version, and the raw accept for files", async () => {
    const { fetch, calls } = fakeFetch([() => new Response("param a: Power = 1.0 W")]);
    const github = new GitHub("tok", fetch);
    await expect(github.file("o", "r", "main", "src/a b.gcl")).resolves.toBe(
      "param a: Power = 1.0 W",
    );
    expect(calls[0]?.url).toBe("https://api.github.com/repos/o/r/contents/src/a%20b.gcl?ref=main");
    expect(calls[0]?.headers).toMatchObject({
      Authorization: "Bearer tok",
      Accept: "application/vnd.github.raw+json",
      "X-GitHub-Api-Version": "2022-11-28",
    });
  });

  test("no token means no Authorization header", async () => {
    const { fetch, calls } = fakeFetch([
      () => json({ full_name: "o/r", default_branch: "main", private: false }),
    ]);
    const info = await new GitHub(null, fetch).repo("o", "r");
    expect(info).toEqual({ fullName: "o/r", defaultBranch: "main", private: false });
    expect(calls[0]?.headers).not.toHaveProperty("Authorization");
  });

  test("the tree keeps path, type and size, and encodes the ref", async () => {
    const { fetch, calls } = fakeFetch([
      () =>
        json({
          sha: "abc",
          truncated: false,
          tree: [
            { path: "src", type: "tree" },
            { path: "src/a.gcl", type: "blob", size: 12 },
          ],
        }),
    ]);
    const tree = await new GitHub(null, fetch).tree("o", "r", "feat/x");
    expect(calls[0]?.url).toBe("https://api.github.com/repos/o/r/git/trees/feat%2Fx?recursive=1");
    expect(tree).toEqual({
      sha: "abc",
      truncated: false,
      entries: [
        { path: "src", type: "tree", size: null },
        { path: "src/a.gcl", type: "blob", size: 12 },
      ],
    });
  });

  test("a second request carries the ETag and a 304 answers from the cache", async () => {
    const { fetch, calls } = fakeFetch([
      () => new Response("v1", { headers: { ETag: '"e1"' } }),
      () => new Response(null, { status: 304 }),
    ]);
    const github = new GitHub(null, fetch);
    await expect(github.file("o", "r", "main", "a.gcl")).resolves.toBe("v1");
    await expect(github.file("o", "r", "main", "a.gcl")).resolves.toBe("v1");
    expect(calls[1]?.headers["If-None-Match"]).toBe('"e1"');
  });

  test("failures are classified", async () => {
    const kinds = async (response: Response): Promise<[string, string]> => {
      const { fetch } = fakeFetch([() => response]);
      try {
        await new GitHub(null, fetch).file("o", "r", "main", "a.gcl");
      } catch (error) {
        if (error instanceof GitHubError) return [error.kind, error.message];
        throw error;
      }
      return ["ok", ""];
    };
    expect((await kinds(new Response("", { status: 404 })))[0]).toBe("not_found");
    expect((await kinds(new Response("", { status: 401 })))[0]).toBe("unauthorized");
    expect((await kinds(new Response("", { status: 403 })))[0]).toBe("forbidden");
    expect((await kinds(new Response("", { status: 500 })))[0]).toBe("other");
    const limited = await kinds(
      new Response("", {
        status: 403,
        headers: { "X-RateLimit-Remaining": "0", "X-RateLimit-Reset": "1700000000" },
      }),
    );
    expect(limited[0]).toBe("rate_limited");
    expect(limited[1]).toMatch(/rate limit reached until/);
  });

  test("a fetch that throws is a network failure", async () => {
    const failing: typeof fetch = () => Promise.reject(new TypeError("Failed to fetch"));
    await expect(new GitHub(null, failing).repo("o", "r")).rejects.toMatchObject({
      kind: "network",
    });
  });

  test("lists branches, tags and pull requests, reading a fork's head by commit", async () => {
    const { fetch, calls } = fakeFetch([
      () => json([{ name: "main" }, { name: "feature/x" }]),
      () => json([{ name: "v0.5.2" }]),
      () =>
        json([
          {
            number: 42,
            title: "thermal",
            head: { ref: "thermal", sha: "a1", repo: { full_name: "o/r" } },
          },
          {
            number: 43,
            title: "fork",
            head: { ref: "fix", sha: "b2", repo: { full_name: "x/r" } },
          },
          { number: 44, title: "gone", head: { ref: "fix", sha: "c3", repo: null } },
        ]),
    ]);
    const github = new GitHub(null, fetch);
    await expect(github.refs("o", "r")).resolves.toEqual({
      branches: ["main", "feature/x"],
      tags: ["v0.5.2"],
      pulls: [
        { number: 42, title: "thermal", ref: "thermal" },
        { number: 43, title: "fork", ref: "b2" },
        { number: 44, title: "gone", ref: "c3" },
      ],
    });
    expect(calls.map((c) => c.url)).toEqual([
      "https://api.github.com/repos/o/r/branches?per_page=100",
      "https://api.github.com/repos/o/r/tags?per_page=100",
      "https://api.github.com/repos/o/r/pulls?state=open&per_page=100",
    ]);
  });
});

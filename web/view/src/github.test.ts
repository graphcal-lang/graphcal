import { describe, expect, test } from "vite-plus/test";
import { GitHub, GitHubError, PROTOCOL_VERSION, type Send } from "./github.ts";

interface Call {
  request: Record<string, unknown>;
}

/** A send that answers from a script of answers and records what it was asked. */
function fakeSend(answers: ((request: Record<string, unknown>) => unknown)[]): {
  send: Send;
  calls: Call[];
} {
  const calls: Call[] = [];
  const send: Send = (request) => {
    calls.push({ request });
    const answer = answers.shift();
    if (!answer) return Promise.reject(new Error("unexpected request"));
    return Promise.resolve(answer(request));
  };
  return { send, calls };
}

const ok = (value: unknown) => () => ({ ok: true, value });
const SHA = "0123456789abcdef0123456789abcdef01234567";

describe("GitHub", () => {
  test("each read is one versioned request to the extension", async () => {
    const { send, calls } = fakeSend([
      ok({ defaultBranch: "main" }),
      ok({ sha: SHA }),
      ok({ paths: ["graphcal.toml", "src/a.gcl"] }),
    ]);
    const github = new GitHub(send);
    await expect(github.repo("o", "r")).resolves.toEqual({ defaultBranch: "main" });
    const commit = await github.commit("o", "r", "feat/x");
    expect(commit).toEqual({ sha: SHA });
    await expect(github.tree("o", "r", commit)).resolves.toEqual({
      entries: [
        { path: "graphcal.toml", size: null },
        { path: "src/a.gcl", size: null },
      ],
    });
    expect(calls.map((c) => c.request)).toEqual([
      { v: PROTOCOL_VERSION, method: "repo", owner: "o", repo: "r" },
      { v: PROTOCOL_VERSION, method: "commit", owner: "o", repo: "r", ref: "feat/x" },
      { v: PROTOCOL_VERSION, method: "tree", owner: "o", repo: "r", sha: SHA },
    ]);
  });

  test("a file is read once per commit", async () => {
    const { send, calls } = fakeSend([ok({ text: "v1" }), ok({ text: "v2" })]);
    const github = new GitHub(send);
    const at = { sha: SHA };
    const [first, second] = await Promise.all([
      github.read("o", "r", at, "a.gcl"),
      github.read("o", "r", at, "a.gcl"),
    ]);
    expect([first, second, await github.read("o", "r", at, "a.gcl")]).toEqual(["v1", "v1", "v1"]);
    expect(calls).toHaveLength(1);
    const other = { sha: "0000000000000000000000000000000000000000" };
    await expect(github.read("o", "r", other, "a.gcl")).resolves.toBe("v2");
    expect(calls[1]?.request).toMatchObject({ method: "read", sha: other.sha, path: "a.gcl" });
  });

  test("a failed read is asked again", async () => {
    const { send, calls } = fakeSend([
      () => ({ ok: false, kind: "other", message: "GitHub answered HTTP 500" }),
      ok({ text: "v1" }),
    ]);
    const github = new GitHub(send);
    await expect(github.read("o", "r", { sha: SHA }, "a.gcl")).rejects.toMatchObject({
      kind: "other",
      message: "GitHub answered HTTP 500",
    });
    await expect(github.read("o", "r", { sha: SHA }, "a.gcl")).resolves.toBe("v1");
    expect(calls).toHaveLength(2);
  });

  test("what changed and the refs come through as given", async () => {
    const { send } = fakeSend([
      ok({ paths: ["b.gcl", "new.gcl"] }),
      ok({
        branches: ["main", "feat/x"],
        tags: [],
        pulls: [],
        failures: [{ list: "tags", kind: null, message: "the extension lists branches only" }],
      }),
    ]);
    const github = new GitHub(send);
    await expect(github.changed("o", "r", "main", "feat/x")).resolves.toEqual(
      new Set(["b.gcl", "new.gcl"]),
    );
    const refs = await github.refs("o", "r");
    expect(refs.branches).toEqual(["main", "feat/x"]);
    expect(refs.failures[0]?.list).toBe("tags");
  });

  test("the extension's failures keep their kind; a missing extension is a network failure", async () => {
    const { send } = fakeSend([
      () => ({ ok: false, kind: "signed_out", message: "sign in to github.com" }),
      () => ({ ok: false, kind: "ref_not_found", message: "o/r has no branch nope" }),
    ]);
    const github = new GitHub(send);
    await expect(github.repo("o", "r")).rejects.toMatchObject({
      kind: "signed_out",
      message: "sign in to github.com",
    });
    await expect(github.commit("o", "r", "nope")).rejects.toMatchObject({ kind: "ref_not_found" });
    const gone = new GitHub(() => Promise.reject(new Error("Receiving end does not exist")));
    await expect(gone.repo("o", "r")).rejects.toMatchObject({ kind: "network" });
  });

  test("an answer in an unexpected form is refused, not trusted", async () => {
    const { send } = fakeSend([ok({ sha: "main" }), () => "yes"]);
    const github = new GitHub(send);
    await expect(github.commit("o", "r", "main")).rejects.toMatchObject({ kind: "other" });
    await expect(github.repo("o", "r")).rejects.toBeInstanceOf(GitHubError);
  });
});

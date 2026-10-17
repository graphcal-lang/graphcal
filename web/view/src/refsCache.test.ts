import { expect, test } from "vite-plus/test";
import type { Refs } from "./github.ts";
import { RefsCache } from "./refsCache.ts";

const refs = (branch: string): Refs => ({ branches: [branch], tags: [], pulls: [], failures: [] });

test("the refs are read once per repository and token", async () => {
  const cache = new RefsCache();
  let reads = 0;
  const load = (branch: string) => (): Promise<Refs> => {
    reads += 1;
    return Promise.resolve(refs(branch));
  };
  const first = { owner: "o", repo: "a", token: null };
  await expect(cache.get(first, load("a"))).resolves.toEqual(refs("a"));
  await expect(cache.get({ ...first }, load("x"))).resolves.toEqual(refs("a"));
  expect(reads).toBe(1);
  // Another repository lists its own refs.
  await expect(cache.get({ ...first, repo: "b" }, load("b"))).resolves.toEqual(refs("b"));
  // A token may see what no token could.
  await expect(cache.get({ ...first, repo: "b", token: "t" }, load("t"))).resolves.toEqual(
    refs("t"),
  );
  expect(reads).toBe(3);
});

test("a failed read is asked again", async () => {
  const cache = new RefsCache();
  const key = { owner: "o", repo: "a", token: null };
  await expect(cache.get(key, () => Promise.reject(new Error("offline")))).rejects.toThrow(
    "offline",
  );
  await expect(cache.get(key, () => Promise.resolve(refs("a")))).resolves.toEqual(refs("a"));
});

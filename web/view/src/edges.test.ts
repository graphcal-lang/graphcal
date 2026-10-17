import { describe, expect, test } from "vite-plus/test";
import { EdgeMap } from "./edges.ts";

describe("EdgeMap", () => {
  test("keys an edge by both ends, without joining them", () => {
    const edges = new EdgeMap<boolean>();
    // Joined with "->", these two edges would share the key "a->b->c".
    edges.set("a->b", "c", true);
    edges.set("a", "b->c", false);
    expect(edges.get("a->b", "c")).toBe(true);
    expect(edges.get("a", "b->c")).toBe(false);
    expect(edges.has("c", "a->b")).toBe(false);
    expect(edges.size).toBe(2);
  });
});

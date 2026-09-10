import { describe, expect, it } from "vite-plus/test";
import { gzipSync } from "node:zlib";
import { MAX_SOURCE_BYTES, evaluationRequest, validateDocument } from "./document";
import { bindingsSchema } from "./bindings";
import { viewLocation, sameLocationDocument } from "./location";
import {
  decodeFragment,
  encodeFragment,
  MAX_ENVELOPE_BYTES,
  MAX_URL_LENGTH,
  shareUrl,
} from "./share-codec";

const document = { filename: "rocket.gcl", source: "// 🙂 % # &\r\nnode x: Length = 1.0 m;\n" };
function payload(bytes: string | Uint8Array) {
  return `#v=1&code=${gzipSync(bytes).toString("base64url")}`;
}

describe("single-file boundary", () => {
  it("preserves the semantic filename and creates exactly one file", () => {
    expect(evaluationRequest(document)).toEqual({
      entry: "rocket.gcl",
      files: [{ path: "rocket.gcl", content: document.source }],
    });
  });
  it.each([
    "../a.gcl",
    "a/b.gcl",
    "a\\b.gcl",
    "graphcal.toml",
    "1.gcl",
    "",
    "a".repeat(129) + ".gcl",
  ])("rejects unsafe basename %s", (filename) => {
    expect(() => validateDocument({ filename, source: "" })).toThrow();
  });
  it("checks Unicode byte limits and shape", () => {
    expect(() =>
      validateDocument({ ...document, source: "🙂".repeat(MAX_SOURCE_BYTES / 4 + 1) }),
    ).toThrow();
    expect(() => validateDocument({ ...document, files: [] })).toThrow();
    expect(() => validateDocument({ ...document, source: "\ud800" })).toThrow();
    expect(validateDocument({ ...document, source: "" }).source).toBe("");
  });
});

describe("sharing source and applied parameters", () => {
  it.each([document, { filename: "empty.gcl", source: "" }])(
    "round trips without changing source",
    async (value) => {
      expect(await decodeFragment(await encodeFragment(value))).toEqual({
        document: value,
        bindings: [],
      });
    },
  );
  it("decodes a gzip fixture produced independently of the browser codec", async () => {
    expect(await decodeFragment(payload(JSON.stringify(document)))).toEqual({
      document,
      bindings: [],
    });
  });
  it("keeps the published v1 decode format stable", async () => {
    expect(
      await decodeFragment(
        "#v=1&code=H4sIAAAAAAAAE6tWSsvMSc1LzE1VslLKTczM00tPzlHSUSrOLy1KBokp1QIABYuCeyMAAAA",
      ),
    ).toEqual({ document: { filename: "main.gcl", source: "" }, bindings: [] });
  });
  it("constructs a canonical source-only fragment URL", async () => {
    const url = new URL(await shareUrl(document, "https://graphcal.org/docs/?example=no"));
    expect(url.pathname).toBe("/playground/");
    expect(url.search).toBe("");
    expect(await decodeFragment(url.hash)).toEqual({ document, bindings: [] });
  });
  it("shares exact unit-bearing and i64 literals with the report view", async () => {
    const bindings = [
      { name: "speed", expr: "36.0 km/h" },
      { name: "count", expr: "9223372036854775807" },
    ];
    const url = new URL(await shareUrl(document, "https://graphcal.org", bindings, "report"));
    expect(url.search).toBe("?view=report");
    expect(await decodeFragment(url.hash)).toEqual({ document, bindings });
    const fixture = `#v=2&code=${gzipSync(JSON.stringify({ document, bindings })).toString("base64url")}`;
    expect(await decodeFragment(fixture)).toEqual({ document, bindings });
  });
  it("bounds binding metadata and rejects duplicates before Wasm", () => {
    expect(bindingsSchema.parse([{ name: "x", expr: "🙂".repeat(1024) }])).toHaveLength(1);
    for (const bindings of [
      [{ name: "x", expr: "🙂".repeat(1024) + "a" }],
      [
        { name: "x", expr: "1" },
        { name: "x", expr: "2" },
      ],
      [{ name: "x", expr: "\ud800" }],
      [{ name: "x", expr: "1", extra: true }],
      Array.from({ length: 257 }, (_, i) => ({ name: `x${i}`, expr: "1" })),
    ])
      expect(bindingsSchema.safeParse(bindings).success).toBe(false);
  });
  it("validates view independently of document identity", () => {
    const workspace = new URL("https://graphcal.org/playground/?example=rocket");
    const report = new URL(workspace);
    report.searchParams.set("view", "report");
    expect(viewLocation(workspace)).toBe("workspace");
    expect(viewLocation(report)).toBe("report");
    expect(sameLocationDocument(workspace, report)).toBe(true);
    for (const query of ["view=unknown", "view=report&view=report"])
      expect(() => viewLocation(new URL(`https://graphcal.org/playground/?${query}`))).toThrow();
  });
  it.each([
    "",
    "#v=2&code=a",
    "#v=1&v=1&code=a",
    "#v=1&code=a&x=b",
    "#v=1&code=%",
    "#v=1&code=aaa",
    "#" + "x".repeat(MAX_URL_LENGTH),
  ])("rejects malformed input %s", async (fragment) => {
    await expect(decodeFragment(fragment)).rejects.toThrow();
  });
  it("rejects invalid UTF-8, JSON, shapes and decompression bombs", async () => {
    for (const input of [
      new Uint8Array([255]),
      "{",
      JSON.stringify({ ...document, extra: 1 }),
      " ".repeat(MAX_ENVELOPE_BYTES + 1),
    ]) {
      await expect(decodeFragment(payload(input))).rejects.toThrow();
    }
  });
  it("rejects truncated gzip", async () => {
    await expect(decodeFragment(payload(JSON.stringify(document)).slice(0, -4))).rejects.toThrow();
  });
});

import { readFileSync } from "node:fs";
import { describe, expect, test } from "vite-plus/test";

/** The header rules of public/_headers (Cloudflare's format), by URL pattern. */
function rules(): Map<string, Map<string, string>> {
  const text = readFileSync(new URL("../public/_headers", import.meta.url), "utf8");
  const out = new Map<string, Map<string, string>>();
  let current: Map<string, string> | null = null;
  for (const line of text.split("\n")) {
    if (line.trim() === "" || line.startsWith("#")) continue;
    if (!line.startsWith(" ")) {
      current = new Map();
      out.set(line.trim(), current);
      continue;
    }
    const at = line.indexOf(":");
    if (current === null || at < 0) throw new Error(`not a header line: ${line}`);
    current.set(line.slice(0, at).trim().toLowerCase(), line.slice(at + 1).trim());
  }
  return out;
}

describe("_headers", () => {
  test("every response forbids framing, forms and foreign resources", () => {
    // MDL-7: a meta `frame-ancestors` is ignored; only a header protects the page.
    const all = rules().get("/*");
    expect(all).toBeDefined();
    const csp = new Map(
      (all?.get("content-security-policy") ?? "").split(";").map((directive) => {
        const [name = "", ...values] = directive.trim().split(/\s+/);
        return [name, values.join(" ")];
      }),
    );
    expect(csp.get("default-src")).toBe("'self'");
    expect(csp.get("connect-src")).toBe("'self'");
    expect(csp.get("object-src")).toBe("'none'");
    expect(csp.get("base-uri")).toBe("'none'");
    expect(csp.get("form-action")).toBe("'none'");
    expect(csp.get("frame-ancestors")).toBe("'none'");
    expect(all?.get("x-frame-options")).toBe("DENY");
    expect(all?.get("x-content-type-options")).toBe("nosniff");
  });

  test("lines stay within Cloudflare's limit", () => {
    const text = readFileSync(new URL("../public/_headers", import.meta.url), "utf8");
    for (const line of text.split("\n")) expect(line.length).toBeLessThanOrEqual(2000);
  });
});

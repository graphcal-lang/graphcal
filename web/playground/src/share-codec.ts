import { validateDocument, type SourceDocument } from "./document";
import { bindingsSchema, type Binding } from "./bindings";
import type { PlaygroundView } from "./location";

export interface SharedCalculation {
  document: SourceDocument;
  bindings: Binding[];
}

export const MAX_URL_LENGTH = 16 * 1024;
export const WARN_URL_LENGTH = 8 * 1024;
export const MAX_ENVELOPE_BYTES = 2 * 1024 * 1024;

/** Read with an incremental budget, including when the input is a gzip bomb. */
async function boundedBytes(
  stream: ReadableStream<Uint8Array>,
  maximum: number,
): Promise<Uint8Array> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  let expired = false;
  const timeout = setTimeout(() => {
    expired = true;
    void reader.cancel().catch(() => {});
  }, 5000);
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (expired) throw new Error("Snippet decoding timed out.");
      if (done) break;
      length += value.length;
      if (length > maximum)
        throw new Error(
          "Snippet exceeds the sharing size limit. Copy the source manually instead.",
        );
      chunks.push(value);
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    return bytes;
  } finally {
    clearTimeout(timeout);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function encodeFragment(
  document: SourceDocument,
  bindings: Binding[] = [],
): Promise<string> {
  if (typeof CompressionStream === "undefined")
    throw new Error("Sharing requires a browser with gzip Compression Streams support.");
  const json = JSON.stringify({
    document: validateDocument(document),
    bindings: bindingsSchema.parse(bindings),
  });
  const bytes = await boundedBytes(
    new Blob([json]).stream().pipeThrough(new CompressionStream("gzip")),
    MAX_URL_LENGTH,
  );
  const binary = Array.from(bytes, (byte) => String.fromCharCode(byte)).join("");
  return `#v=2&code=${btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")}`;
}

export async function decodeFragment(fragment: string): Promise<SharedCalculation> {
  if (fragment.length > MAX_URL_LENGTH) throw new Error("Shared URL exceeds the 16 KiB limit.");
  const fields = new URLSearchParams(fragment.replace(/^#/, ""));
  if (
    fields.size !== 2 ||
    fields.getAll("v").length !== 1 ||
    fields.getAll("code").length !== 1 ||
    (fields.get("v") !== "1" && fields.get("v") !== "2")
  ) {
    throw new Error(
      "Unsupported or malformed shared URL. Expected version 1 or 2 and one code payload.",
    );
  }
  const code = fields.get("code")!;
  if (!/^[A-Za-z0-9_-]+$/.test(code) || code.length % 4 === 1)
    throw new Error("Invalid base64url snippet.");
  if (typeof DecompressionStream === "undefined")
    throw new Error("Opening shared snippets requires gzip Decompression Streams support.");
  try {
    const binary = atob(code.replaceAll("-", "+").replaceAll("_", "/"));
    // Reject non-canonical trailing bits rather than accepting ambiguous encodings.
    if (btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "") !== code)
      throw new Error("Non-canonical base64url.");
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    const decoded = await boundedBytes(
      new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip")),
      MAX_ENVELOPE_BYTES,
    );
    const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(decoded));
    if (fields.get("v") === "1") return { document: validateDocument(value), bindings: [] };
    if (
      typeof value !== "object" ||
      value === null ||
      Object.keys(value).length !== 2 ||
      !("document" in value) ||
      !("bindings" in value)
    )
      throw new Error("Expected document and bindings.");
    return {
      document: validateDocument(value.document),
      bindings: bindingsSchema.parse(value.bindings),
    };
  } catch (error) {
    throw new Error(
      `Could not open shared snippet: ${error instanceof Error ? error.message : "invalid payload"}`,
    );
  }
}

export async function shareUrl(
  document: SourceDocument,
  origin: string,
  bindings: Binding[] = [],
  view: PlaygroundView = "workspace",
): Promise<string> {
  const url = new URL("/playground/", origin);
  if (view === "report") url.searchParams.set("view", view);
  url.hash = await encodeFragment(document, bindings);
  if (url.href.length > MAX_URL_LENGTH)
    throw new Error(
      "Shared URL exceeds 16 KiB. Copy the source manually instead; it has not been truncated.",
    );
  return url.href;
}

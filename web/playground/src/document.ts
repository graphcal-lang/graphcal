export const MAX_SOURCE_BYTES = 256 * 1024;
export interface SourceDocument {
  readonly filename: string;
  readonly source: string;
}

/** A basename is also the virtual package name; never silently rename it. */
export function validateDocument(value: unknown): SourceDocument {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Expected a filename and source.");
  }
  const fields = Object.keys(value);
  if (fields.length !== 2 || !fields.includes("filename") || !fields.includes("source")) {
    throw new Error("A snippet must contain exactly filename and source.");
  }
  const { filename, source } = value as Record<string, unknown>;
  if (
    typeof filename !== "string" ||
    !/^[a-zA-Z_][a-zA-Z0-9_]*\.gcl$/.test(filename) ||
    filename.length > 128
  ) {
    throw new Error(
      "Use a .gcl filename with an identifier stem (letters, digits, underscores; at most 128 characters).",
    );
  }
  if (typeof source !== "string" || !source.isWellFormed()) {
    throw new Error("Source must be valid Unicode text.");
  }
  if (new TextEncoder().encode(source).length > MAX_SOURCE_BYTES) {
    throw new Error("Source exceeds the 256 KiB browser limit. Your text has not been changed.");
  }
  return { filename, source };
}

export function sameDocument(left: SourceDocument, right: SourceDocument): boolean {
  return left.filename === right.filename && left.source === right.source;
}

export function evaluationRequest(document: SourceDocument) {
  const { filename, source } = validateDocument(document);
  return { entry: filename, files: [{ path: filename, content: source }] };
}

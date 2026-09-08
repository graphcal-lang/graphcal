import { z } from "zod";
import catalog from "../examples/catalog.json";
import { validateDocument } from "./document";
import { selectExample } from "./example-catalog";

export const examples = z
  .array(
    z.object({
      id: z.string().regex(/^[a-z][a-z0-9-]*$/),
      title: z.string(),
      description: z.string(),
      filename: z.string(),
    }),
  )
  .parse(catalog);
if (new Set(examples.map((example) => example.id)).size !== examples.length)
  throw new Error("Duplicate example ID");

export async function loadExample(id: string, signal: AbortSignal) {
  const example = selectExample(examples, id);
  const response = await fetch(`${import.meta.env.BASE_URL}examples/${example.id}.gcl`, { signal });
  if (!response.ok)
    throw new Error(
      `Example download failed (HTTP ${response.status}). Your document has not changed.`,
    );
  return validateDocument({ filename: example.filename, source: await response.text() });
}

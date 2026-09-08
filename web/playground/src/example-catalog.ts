export interface Example {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly filename: string;
  readonly source: string;
}

/** URL inputs select an existing entry, never a URL/path to fetch. */
export function selectExample(examples: readonly Example[], id: string): Example {
  const example = examples.find((entry) => entry.id === id);
  if (!example) throw new Error(`Unknown example: ${id.slice(0, 80)}`);
  return example;
}

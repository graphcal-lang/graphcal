export type DocumentLocation =
  | { kind: "shared"; fragment: string }
  | { kind: "example"; id: string };
export function documentLocation(url: URL): DocumentLocation {
  if (url.hash) return { kind: "shared", fragment: url.hash };
  if (url.searchParams.getAll("example").length > 1) throw new Error("Specify only one example.");
  return { kind: "example", id: url.searchParams.get("example") ?? "rocket" };
}

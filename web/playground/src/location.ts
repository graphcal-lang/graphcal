export type DocumentLocation =
  | { kind: "shared"; fragment: string }
  | { kind: "example"; id: string };
export type PlaygroundView = "workspace" | "report";
export function viewLocation(url: URL): PlaygroundView {
  const views = url.searchParams.getAll("view");
  if (views.length > 1 || (views.length === 1 && views[0] !== "workspace" && views[0] !== "report"))
    throw new Error("Specify one view: workspace or report.");
  return views[0] === "report" ? "report" : "workspace";
}
export function sameLocationDocument(left: URL, right: URL): boolean {
  const a = documentLocation(left);
  const b = documentLocation(right);
  return a.kind === "shared"
    ? b.kind === "shared" && a.fragment === b.fragment
    : b.kind === "example" && a.id === b.id;
}
export function documentLocation(url: URL): DocumentLocation {
  if (url.hash) return { kind: "shared", fragment: url.hash };
  if (url.searchParams.getAll("example").length > 1) throw new Error("Specify only one example.");
  return { kind: "example", id: url.searchParams.get("example") ?? "rocket" };
}

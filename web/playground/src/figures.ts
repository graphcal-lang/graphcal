type Embed = (
  target: HTMLElement,
  spec: Record<string, unknown>,
  options: Record<string, unknown>,
) => Promise<{ finalize: () => void }>;
declare global {
  var vegaEmbed: Embed | undefined;
}
let runtime: Promise<Embed> | undefined;
const scripts = new Map<string, Promise<void>>();
function script(name: string) {
  const existing = scripts.get(name);
  if (existing) return existing;
  const promise = new Promise<void>((resolve, reject) => {
    const node = document.createElement("script");
    node.src = `${import.meta.env.BASE_URL}vega/${name}`;
    const timer = setTimeout(() => failed(), 30_000);
    function failed() {
      clearTimeout(timer);
      node.remove();
      scripts.delete(name);
      reject(new Error("Plot library failed to load. Press Run to retry."));
    }
    node.addEventListener(
      "load",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
    node.addEventListener("error", failed, { once: true });
    document.head.append(node);
  });
  scripts.set(name, promise);
  return promise;
}
async function ensureRuntime(): Promise<Embed> {
  runtime ??= (async () => {
    for (const name of ["vega.min.js", "vega-lite.min.js", "vega-embed.min.js"]) await script(name);
    if (!globalThis.vegaEmbed) throw new Error("Plot library did not initialize.");
    return globalThis.vegaEmbed;
  })().catch((error: unknown) => {
    runtime = undefined;
    throw error;
  });
  return runtime;
}

export const denyPlotResource = () =>
  Promise.reject(new Error("External plot resources are unavailable in the playground."));
export async function renderFigure(
  target: HTMLElement,
  spec: Record<string, unknown>,
  current: () => boolean,
): Promise<(() => void) | undefined> {
  const embed = await ensureRuntime();
  if (!current()) return;
  // Ignore embed configuration in transported metadata. No source-selected
  // loader, image URL, hyperlink, or Vega editor export is permitted.
  const { usermeta: _metadata, ...safeSpec } = spec;
  const result = await embed(target, safeSpec, {
    actions: false,
    tooltip: false,
    defaultStyle: false,
    loader: {
      load: denyPlotResource,
      sanitize: denyPlotResource,
      http: denyPlotResource,
      file: denyPlotResource,
    },
  });
  if (!current()) {
    result.finalize();
    return;
  }
  return () => result.finalize();
}

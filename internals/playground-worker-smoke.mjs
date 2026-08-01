import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const siteRoot = new URL("../site/", import.meta.url);
const exampleRoot = new URL("assets/playground/examples/step-5/", siteRoot);
const descriptor = JSON.parse(readFileSync(new URL("project.json", exampleRoot), "utf8"));

let onMessage;
let posted;
globalThis.self = {
  addEventListener(type, handler) {
    if (type === "message") onMessage = handler;
  },
  postMessage(message) {
    posted = message;
  },
};
globalThis.fetch = async (input) => {
  const url = input instanceof URL ? input : new URL(input);
  return new Response(readFileSync(fileURLToPath(url)));
};

await import(new URL("javascripts/playground-worker.mjs", siteRoot));
if (typeof onMessage !== "function") {
  throw new Error("playground worker did not register a message handler");
}

await onMessage({
  data: {
    id: 1,
    request: {
      entry: descriptor.entry,
      files: descriptor.files.map((path) => ({
        path,
        content: readFileSync(new URL(path, exampleRoot), "utf8"),
      })),
    },
  },
});

const names = posted?.result?.evaluation?.values?.map(({ name }) => name) ?? [];
if (
  posted?.id !== 1
  || posted?.workerError
  || posted?.result?.status !== "evaluated"
  || posted.result.evaluation.has_errors
  || !names.includes("delta_v")
) {
  throw new Error(`playground worker smoke test failed: ${JSON.stringify(posted)}`);
}

console.log(`Playground worker smoke test passed (${names.length} values)`);

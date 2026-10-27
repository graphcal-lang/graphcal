import { afterEach, expect, test, vi } from "vite-plus/test";
import { connectExtension, EXTENSION_ID } from "./extension.ts";
import { GitHubError, PROTOCOL_VERSION } from "./github.ts";

/** Give the page a `chrome.runtime.sendMessage` that answers with `answer`. */
function stubExtension(answer: (extensionId: string, message: unknown) => Promise<unknown>) {
  const sendMessage = vi.fn(answer);
  vi.stubGlobal("chrome", { runtime: { sendMessage } });
  return sendMessage;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

async function failureOf(promise: Promise<unknown>): Promise<GitHubError> {
  const error: unknown = await promise.then(
    () => null,
    (e: unknown) => e,
  );
  if (!(error instanceof GitHubError))
    throw new Error(`expected a GitHubError, got ${String(error)}`);
  return error;
}

test("an extension that says hello is connected", async () => {
  const sendMessage = stubExtension(() =>
    Promise.resolve({ ok: true, value: { version: "0.1.0" } }),
  );
  await expect(connectExtension()).resolves.toBeDefined();
  expect(sendMessage).toHaveBeenCalledWith(EXTENSION_ID, { v: PROTOCOL_VERSION, method: "hello" });
});

test("a browser without the extension API has no extension", async () => {
  vi.stubGlobal("chrome", undefined);
  expect((await failureOf(connectExtension())).kind).toBe("no_extension");
});

test("a message nobody receives means the extension is not installed", async () => {
  stubExtension(() => Promise.reject(new Error("Could not establish connection")));
  expect((await failureOf(connectExtension())).kind).toBe("no_extension");
});

test("an extension that does not answer in time is taken as not installed", async () => {
  vi.useFakeTimers();
  stubExtension(() => new Promise(() => {}));
  const failure = failureOf(connectExtension());
  await vi.advanceTimersByTimeAsync(3000);
  expect((await failure).kind).toBe("no_extension");
});

test("an extension that answers in an unknown form is out of date, not missing", async () => {
  stubExtension(() => Promise.resolve({ hello: "from an older protocol" }));
  const error = await failureOf(connectExtension());
  expect(error.kind).toBe("other");
  expect(error.message).toMatch(/update the Graphcal extension/);
});

test("a connected hello leaves no timer behind", async () => {
  vi.useFakeTimers();
  stubExtension(() => Promise.resolve({ ok: true, value: { version: "0.1.0" } }));
  await connectExtension();
  expect(vi.getTimerCount()).toBe(0);
});

// What a web page sees of Chrome's extension API: `chrome.runtime.sendMessage`
// exists on a page whose origin an installed extension lists in its
// `externally_connectable`, and is absent otherwise (so is `chrome`, in
// other browsers).
interface ChromeRuntime {
  /** Send `message` to the extension `extensionId`; resolves with its answer, rejects when it is not there. */
  sendMessage(extensionId: string, message: unknown): Promise<unknown>;
}
declare const chrome: { runtime?: ChromeRuntime } | undefined;

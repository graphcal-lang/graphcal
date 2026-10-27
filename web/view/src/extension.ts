// The Graphcal browser extension, as the page reaches it: by its id, through
// `chrome.runtime.sendMessage`, which Chrome gives a page whose origin the
// extension lists. The id is fixed by the key in the extension's manifest,
// so a development build has the same id as the published one.
import { GitHub, GitHubError, type Send } from "./github.ts";

export const EXTENSION_ID = "fhfgclioheoakndeglaeacnlncpjoahe";
/** Where to get the extension. */
export const INSTALL_URL = `https://chromewebstore.google.com/detail/${EXTENSION_ID}`;

/** How long the extension gets to say hello before it is taken as not installed. */
const HELLO_TIMEOUT_MS = 3000;

/** The page's way to the extension, or null when this browser has none. */
function sender(): Send | null {
  if (typeof chrome === "undefined" || chrome?.runtime?.sendMessage === undefined) return null;
  const { runtime } = chrome;
  return (request) => runtime.sendMessage(EXTENSION_ID, request);
}

/**
 * GitHub through the installed extension. Rejects with a `no_extension`
 * error when the browser has no extension API, the extension is not
 * installed, or it does not answer in time. An extension that answers in a
 * form this page does not know is there but out of date, and rejects with
 * an `other` error that says to update it.
 */
export async function connectExtension(): Promise<GitHub> {
  const send = sender();
  if (send === null) {
    throw new GitHubError(
      "no_extension",
      "the Graphcal extension is not installed in this browser",
    );
  }
  const github = new GitHub(send);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new GitHubError("no_extension", "the Graphcal extension did not answer")),
      HELLO_TIMEOUT_MS,
    );
  });
  try {
    await Promise.race([github.hello(), timeout]);
  } catch (error) {
    if (!(error instanceof GitHubError)) throw error;
    switch (error.kind) {
      case "network":
        // `sendMessage` rejected: no extension with this id is listening.
        throw new GitHubError(
          "no_extension",
          "the Graphcal extension is not installed in this browser",
        );
      case "other":
        throw new GitHubError("other", `${error.message}; update the Graphcal extension`);
      default:
        throw error;
    }
  } finally {
    clearTimeout(timer);
  }
  return github;
}

// The bar's GitHub parts: where the sheet is read from, and the extension it
// is read through (a link to install it, when it is missing).
import { INSTALL_URL } from "./extension.ts";
import type { GitHubHost } from "./githubHost.ts";
import { describeRoute } from "./route.ts";

/** `owner/repo @ ref` on the left; the extension's state on the right. */
export function mountGitHubBar(host: GitHubHost, originEl: HTMLElement, slotEl: HTMLElement): void {
  const install = document.createElement("a");
  install.className = "bar-install";
  install.href = INSTALL_URL;
  install.target = "_blank";
  install.rel = "noopener";
  install.textContent = "Install the Graphcal extension";
  install.title = "the viewer reads GitHub through the extension, with your GitHub sign-in";
  slotEl.replaceChildren(install);
  originEl.hidden = false;

  host.onState((state) => {
    originEl.textContent = describeRoute(state.route, state.ref);
    originEl.title = `${state.route.owner}/${state.route.repo}`;
    originEl.classList.toggle("loading", state.loading);
    slotEl.hidden = !state.needsExtension;
  });
}

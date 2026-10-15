// The bar's GitHub parts: where the sheet is read from, and the token.
import type { GitHubHost } from "./githubHost.ts";
import { LoginDialog, type TokenStore } from "./login.ts";
import { describeRoute } from "./route.ts";

/** `owner/repo @ ref` on the left; `sign in` / `signed in` on the right. */
export function mountGitHubBar(
  host: GitHubHost,
  tokens: TokenStore,
  originEl: HTMLElement,
  slotEl: HTMLElement,
): void {
  const dialog = new LoginDialog(tokens);
  const login = document.createElement("button");
  login.type = "button";
  login.className = "bar-login";
  login.addEventListener("click", () => dialog.open());
  slotEl.replaceChildren(login);
  slotEl.hidden = false;
  originEl.hidden = false;

  const renderLogin = (wantsToken: boolean): void => {
    const signedIn = tokens.get() !== null;
    login.textContent = signedIn ? "GitHub · signed in" : "GitHub · sign in";
    login.title = signedIn
      ? "a token is stored in this browser; click to replace or forget it"
      : "paste a token to read private repositories";
    login.classList.toggle("wanted", wantsToken);
  };
  host.onState((state) => {
    originEl.textContent = describeRoute(state.route, state.ref);
    originEl.title = `${state.route.owner}/${state.route.repo}`;
    originEl.classList.toggle("loading", state.loading);
    renderLogin(state.wantsToken);
  });
  tokens.onChange(() => renderLogin(false));
}

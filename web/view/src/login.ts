// The GitHub token the viewer reads with. It lives in this browser's
// localStorage and goes to api.github.com only. Without one, public
// repositories are readable at GitHub's anonymous rate; with one, private
// repositories the token can read are too.

const KEY = "graphcal.view.github-token";

export class TokenStore {
  private readonly listeners: (() => void)[] = [];

  get(): string | null {
    try {
      const token = localStorage.getItem(KEY);
      return token === null || token.length === 0 ? null : token;
    } catch {
      return null;
    }
  }

  set(token: string): void {
    const trimmed = token.trim();
    if (trimmed.length === 0) {
      this.clear();
      return;
    }
    try {
      localStorage.setItem(KEY, trimmed);
    } catch {
      // Storage is blocked (private window, say): the token is lost on reload.
    }
    this.notify();
  }

  clear(): void {
    try {
      localStorage.removeItem(KEY);
    } catch {
      // Nothing to remove.
    }
    this.notify();
  }

  onChange(listener: () => void): void {
    this.listeners.push(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text) node.textContent = text;
  return node;
}

/** A dialog that takes a token, or forgets the stored one. */
export class LoginDialog {
  private readonly dialog = el("dialog", "login");
  private readonly input = el("input", "login-input");
  private readonly forget = el("button", "login-forget", "Forget token");

  private readonly tokens: TokenStore;

  constructor(tokens: TokenStore) {
    this.tokens = tokens;
    const { dialog, input, forget } = this;
    const form = el("form", "login-form");
    form.method = "dialog";
    form.append(
      el("h2", "login-title", "GitHub token"),
      el(
        "p",
        "login-text",
        "A fine-grained personal access token with Contents: Read-only on the repositories you want to view. It stays in this browser and is sent to api.github.com only.",
      ),
    );
    input.type = "password";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = "github_pat_…";
    input.setAttribute("aria-label", "GitHub token");
    form.append(input);
    const row = el("div", "login-row");
    const use = el("button", "login-use", "Use token");
    use.type = "submit";
    const cancel = el("button", "login-cancel", "Cancel");
    cancel.type = "button";
    cancel.addEventListener("click", () => dialog.close());
    forget.type = "button";
    forget.addEventListener("click", () => {
      tokens.clear();
      dialog.close();
    });
    row.append(use, cancel, forget);
    form.append(row);
    form.addEventListener("submit", () => {
      if (input.value.trim().length > 0) tokens.set(input.value);
    });
    dialog.append(form);
    dialog.addEventListener("close", () => {
      input.value = "";
    });
    document.body.append(dialog);
  }

  open(): void {
    this.forget.hidden = this.tokens.get() === null;
    this.dialog.showModal();
    this.input.focus();
  }
}

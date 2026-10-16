// An input put in place of what it edits (a cell, the formula bar's
// expression): Enter or blur commits, Escape cancels; the input stays while
// the host evaluates, and again when the edit is refused.

export function bindInput(
  input: HTMLInputElement,
  initial: string,
  commit: (text: string) => Promise<boolean>,
  cancel: () => void,
): void {
  let busy = false;
  let done = false;
  const finish = (): void => {
    done = true;
  };
  const submit = (): void => {
    if (busy || done) return;
    if (input.value === initial) {
      finish();
      cancel();
      return;
    }
    busy = true;
    void commit(input.value).then((ok) => {
      busy = false;
      if (ok) finish();
      else if (document.activeElement !== input && input.isConnected) input.focus();
    });
  };
  input.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      finish();
      cancel();
    }
  });
  input.addEventListener("blur", () => {
    if (!done && !busy) submit();
  });
  input.addEventListener("pointerdown", (event) => event.stopPropagation());
}

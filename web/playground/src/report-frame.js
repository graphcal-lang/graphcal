// Fixed chart policy applies even to the initial static report scripts.
(function () {
  const embed = window.vegaEmbed;
  const deny = () =>
    Promise.reject(new Error("External plot resources are unavailable in the playground."));
  window.vegaEmbed = (target, spec) => {
    const { usermeta: _metadata, ...safeSpec } = spec;
    return embed(target, safeSpec, {
      actions: false,
      tooltip: false,
      defaultStyle: false,
      loader: { load: deny, sanitize: deny, http: deny, file: deny },
    });
  };
})();

// The sandbox has an opaque origin. Only the configured parent can initialize it.
window.addEventListener("DOMContentLoaded", () => {
  // Fragment-only links in srcdoc otherwise resolve against the parent's URL,
  // navigating away from this opaque report frame rather than within it.
  document.querySelector(".report-nav")?.addEventListener("click", (event) => {
    const link = event.target.closest("a");
    if (!link) return;
    const target = document.getElementById(link.getAttribute("href").slice(1));
    if (!target) return;
    event.preventDefault();
    target.scrollIntoView();
    target.focus({ preventScroll: true });
  });
  const config = JSON.parse(document.getElementById("report-connection").textContent);
  const send = (message) =>
    parent.postMessage({ ...message, session: config.session }, config.origin);
  let callbacks;
  let initialized = false;
  window.addEventListener("message", (event) => {
    if (
      event.source !== parent ||
      event.origin !== config.origin ||
      event.data?.session !== config.session
    )
      return;
    const message = event.data;
    if (message.type === "initialize" && !initialized) {
      initialized = true;
      window.GraphcalReport.mount({
        baselineBindings: [],
        initial: { evaluation: message.evaluation, bindings: message.bindings },
        hostOwnsTimeout: true,
        onPending: () => send({ type: "pending" }),
        createTransport: (receiver) => {
          callbacks = receiver;
          queueMicrotask(() => receiver.onMessage({ type: "ready", ports: message.ports }));
          return { postMessage: send, terminate() {} };
        },
      });
    } else if (message.type === "result" && callbacks) {
      callbacks.onMessage(message);
    }
  });
  send({ type: "ready" });
});

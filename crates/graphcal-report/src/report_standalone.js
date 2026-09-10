// Standalone Graphcal report bootstrap. Reads the embedded engine payload and
// adapts its Web Worker to the transport contract owned by report_runtime.js.
(function () {
  "use strict";

  function payloadText(id) {
    var el = document.getElementById(id);
    return el ? el.textContent : null;
  }

  var projectText = payloadText("graphcal-project");
  var glueB64 = payloadText("graphcal-engine-glue");
  var wasmB64 = payloadText("graphcal-engine-wasm");
  if (!projectText || !glueB64 || !wasmB64 || !window.GraphcalReport) return;

  var baselineBindings;
  try {
    baselineBindings = JSON.parse(payloadText("graphcal-baseline") || "[]");
  } catch (error) {
    return;
  }

  function workerMain() {
    var prepared = null;
    function describeError(error) {
      if (error && error.message) return String(error.message);
      try {
        var text = JSON.stringify(error);
        if (text && text !== "{}") return text;
      } catch (ignored) {
        // fall through to String()
      }
      return String(error);
    }
    self.onmessage = function (event) {
      var msg = event.data;
      try {
        if (msg.type === "init") {
          var binary = atob(msg.wasm);
          var bytes = new Uint8Array(binary.length);
          for (var i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
          Promise.resolve(wasm_bindgen({ module_or_path: bytes }))
            .then(function () {
              prepared = wasm_bindgen.prepareReportBundle(msg.project);
              self.postMessage({ type: "ready", ports: prepared.parameterPorts() });
            })
            .catch(function (error) {
              self.postMessage({ type: "fatal", message: describeError(error) });
            });
        } else if (msg.type === "evaluate") {
          if (!prepared) throw new Error("engine is not prepared");
          self.postMessage({
            type: "result",
            id: msg.id,
            outcome: prepared.evaluateBindings(msg.bindings),
          });
        }
      } catch (error) {
        self.postMessage({ type: "fatal", id: msg.id, message: describeError(error) });
      }
    };
  }

  var workerUrl;
  var assemblyError = null;
  try {
    var glueSource = atob(glueB64);
    var driverSource = "\n(" + workerMain.toString() + ")();\n";
    workerUrl = URL.createObjectURL(
      new Blob([glueSource, driverSource], { type: "text/javascript" }),
    );
  } catch (error) {
    assemblyError = error;
  }

  window.GraphcalReport.mount({
    baselineBindings: baselineBindings,
    createTransport: function (callbacks) {
      if (assemblyError) {
        throw new Error("could not assemble the report engine (" + assemblyError + ")");
      }
      var worker = new Worker(workerUrl);
      worker.onmessage = function (event) { callbacks.onMessage(event.data); };
      worker.onerror = function (event) {
        callbacks.onError(event.message || "the report engine crashed");
      };
      worker.postMessage({ type: "init", project: projectText, wasm: wasmB64 });
      return {
        postMessage: function (message) { worker.postMessage(message); },
        terminate: function () { worker.terminate(); },
      };
    },
  });
})();

// Entry of the VS Code webview bundle: the same view with its styles and its
// markup, hosted by the extension (see vscodeHost.ts). The shell goes into
// the document before the view looks for its elements.
import "./styles.css";
import shell from "./shell.html?raw";

document.body.insertAdjacentHTML("afterbegin", shell);
await import("./main.ts");

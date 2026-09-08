---
icon: material/code-braces
---

# Graphcal Playground

[**Open the standalone playground**](https://graphcal.org/playground/)

Edit Graphcal in a full-height editor with resizable results, without installing anything. Choose an example such as the rocket equation, structured orbital transfers, indexed maneuvers, assertions, or an exponential-decay plot. Change a parameter and press **Run** (Ctrl/Cmd+Enter), or enable **Auto-run**.

The compiler and evaluator run locally in a Web Worker using WebAssembly. Your source is not uploaded to an evaluation server. **Stop** terminates the worker; loading and evaluation have separate time limits and can be retried.

## Single-file scope

The playground edits exactly one `.gcl` file, up to 256 KiB of UTF-8 source. Its filename is included in shared snippets because it determines the virtual package name used in self-imports. Filenames use an identifier stem (letters, digits, underscores, not starting with a digit) and `.gcl`, up to 128 characters total.

Values, expandable structured/indexed values, compiler diagnostics, runtime errors, assertions, and plots are supported. Click a diagnostic location to select its source. Large output collections have **Show more** controls. Results exceeding the 8 MiB browser display budget are rejected before reaching the UI rather than silently truncated; use the CLI for larger calculations.

Multi-file projects, package dependencies, external plugins, remote data, CLI input files, syntax highlighting, and LSP features are not available. For these workflows, [install Graphcal](installation.md). The [multi-file tutorial](tutorial/step5-multi-file-projects.md) uses the CLI.

## Sharing code

**Share** embeds the current filename and source in a compressed URL fragment (`#v=1&code=…`) and copies the link. If clipboard access is denied, copy the displayed URL manually. Sharing also works for code that does not compile.

- A link is a snapshot, not a live document. After editing, press Share again to include your changes; the page indicates when the URL is stale.
- Opening a shared link restores the source but does not execute it. Review it, then press **Run**.
- Anyone with the link can read the source. It is **not encrypted**; browser history, extensions, and messaging systems may retain it. Do not share secrets.
- Fragments are not sent in HTTP requests to the hosting server. The playground has no source analytics or snippet database.
- URLs are limited to 16 KiB, with a warning above 8 KiB; some messaging systems may truncate long links. Oversized snippets are never truncated. Copy the source manually instead.
- Links preserve source, not historical compiler behavior. The deployed Graphcal version is shown with results; future language versions may evaluate older snippets differently.

**Reset** restores the last loaded example or shared snippet, asking before discarding edits. Loading another example also asks before replacing modified work. There is no automatic local draft storage: share or copy your changes before closing the page.

> [!WARNING]
> Graphcal and the playground are alpha software. Use a current Chrome, Firefox, or Safari with JavaScript, WebAssembly, module workers, and gzip Compression Streams. Browser execution is resource-limited and does not replace independently reviewing engineering calculations.

Continue with the [tutorial](tutorial/index.md) for guided examples.

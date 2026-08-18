---
icon: material/code-braces
hide:
  - toc
---

# Graphcal Playground

Edit and evaluate Graphcal without installing anything. The compiler and evaluator run locally in a Web Worker using WebAssembly; source code is not sent to a server.

!!! warning "Experimental"

    The playground is experimental, and Graphcal itself is currently alpha software. Expect both the language and the browser experience to change.

<div class="gc-playground-host">
  <graphcal-playground example="step-2"></graphcal-playground>
  <noscript><p class="gc-playground-fallback">The browser playground requires JavaScript and WebAssembly. You can still <a href="assets/playground/examples/step-2/main.gcl">open the static example source</a> or run it with the local CLI.</p></noscript>
</div>

!!! info "Browser playground scope"

    The v1 playground supports local single-file and multi-file projects, compiler diagnostics, runtime errors, assertions, algebraic values, indexed values, and plot rendering. A project may contain up to 16 files, with a 256 KiB per-file content limit and a 1 MiB aggregate content limit. The entry and each file path may use at most 1 KiB of UTF-8, with a 16 KiB aggregate path limit. Raw JavaScript requests are checked against these bounds before their strings are copied into WebAssembly memory. Project paths are relative, may not contain `.` or `..`, and may not make one file the parent directory of another. Internally they map to normalized absolute virtual paths, with exactly one text or binary content entry per file. Package dependencies, external plugins, editor/LSP features, and CLI input files are not available yet.

For a guided progression through the language, continue with the [interactive tutorial](tutorial/index.md). To work with local files, Git dependencies, plugins, plots, and editor integration, [install Graphcal](installation.md).

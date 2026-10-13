# View

The application in `web/view/` renders a Graphcal file as a sheet: its view
model (`graphcal view`, the LSP's `graphcal/viewModel`, or the Wasm
`evaluateView`) laid out as columns and wires. The same code runs in three
hosts, chosen at start in `src/main.ts`:

- `EmptyHost`: the page with no address, which only asks for a file
  (`src/start.ts`: a pasted GitHub file URL or `owner/repo/path.gcl`).
- `VsCodeHost`: the VS Code webview, fed by the extension
  (`vite.webview.config.ts` builds that bundle).
- `GitHubHost`: the published viewer at `https://graphcal.org/view/`, when the
  address names a file: `#owner/repo@ref/path/to/file.gcl` (`@ref` optional;
  a slashed branch is percent-encoded).

## The published viewer

The page holds no data. It reads the project around the named file from the
GitHub REST API with the viewer's own token (`src/github.ts`, plain `fetch`
with ETag caching), picks the project's files by the loader's rule
(`src/repoProject.ts`: the nearest `graphcal.toml` above the file is the root,
nested projects are left out, the browser engine's size limits are checked
first), and evaluates in the browser with the Wasm engine. Repository
permissions are GitHub's: a private repository needs a token that can read
it, a public one needs none (at GitHub's anonymous rate limit).

The token is a fine-grained personal access token with Contents: Read-only,
pasted into the page's dialog (`src/login.ts`), kept in `localStorage`, and
sent to `api.github.com` only. The page's Content-Security-Policy (injected at
build in `vite.config.ts`) allows no other connection and no third-party
script. Edits are a what-if over the sources read; nothing is written back.

The page arranges a code pane and the sheet (`src/editor.ts`, CodeMirror with
the playground's lexical colouring): the toggle at the top right shows the
code, both side by side (the split is dragged), or the sheet. The code pane
shows the main sheet's file with the sheet's values inlaid at the declaring
lines; typing re-evaluates after a pause (⌘/Ctrl-Enter at once) as a what-if
like a cell edit, and the selection is shared both ways. The breadcrumb at the
top left opens the explorer (`src/explorer.ts`): the project's files, and for
a repository the ref being read with a menu of branches, tags and open pull
requests, which switch `@ref` in the address. Pane arrangement, split width
and explorer state are remembered in `localStorage`. The VS Code webview
shows the sheet alone.

`#owner/repo@base...head/path.gcl` compares two refs (`src/compare.ts`): the
sheet shows `head` with every node that differs from `base` in a green frame
(new ones tagged, a dashed frame when only the value changed, cells tinted,
scalars with the old value struck through), the code pane shows the base's
lines in red under the head's in green (CodeMirror's unified merge view),
and the explorer dots the files whose content differs. The base project is
read and evaluated after the head's sheet is up.

The edits can leave the page as a patch: "copy changes" next to the reset
button puts a unified diff (`src/diff.ts`, repository paths, a `# graphcal
what-if:` heading) on the clipboard. `git apply` reads it, and the VS Code
extension's "Graphcal: Apply Changes from Clipboard" puts it into the
workspace's files as one undoable edit, matching each hunk by its context.

Not supported in the browser, as in the playground: manifests with
dependencies or plugins, files over 256 KiB, projects over 1 MiB, and
repositories too large for GitHub to list in one tree request.

## Development and publishing

- `just view-serve`: copy the playground's Wasm engine, then serve with vp.
- `just view-build`: lint, types, tests, production build (`web/view/dist`).
- `just docs-build` assembles `web/view/dist` into `site/view/`; GitHub Pages
  CI uses this recipe, so the viewer deploys with the documentation.
- `just site-serve` previews the whole site, with `/view/#…`, at localhost:4173.

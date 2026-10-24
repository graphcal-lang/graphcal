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

## The model contract

The model is projected in `crates/graphcal-project/src/view_model/` and
validated on the page by the zod schemas in `src/model.ts`; it is at
`version` 1, and a wire change bumps it. The Wasm `evaluateView` answers
`evaluated`; `needs_values` (a required param has no value, as in a library
its includers bind), `binding_errors` or `eval_error`, each with the
structure alone; `not_requested` (prepared without the view),
`projection_failed` or `unknown_instance`. The page (`src/projection.ts`)
keeps three states apart: a project that does not check is refused; one that
checks is shown, with values when it evaluated, else with a note saying it
waits for values or failed to evaluate. An edit stands whenever the sources
still check, so a library with a required param can be edited. Its rules:

- A model has a target. A root file is projected alone (`target.kind:
  module`), and so is each include instance its evaluation materializes,
  nested ones included (`target.kind: instance`, with the `root` module and
  the `instance`): the template's declarations as `module`, with the values
  the root's evaluation gives that instance. They are different targets even
  when the include binds nothing. In an instance's view a param the include
  binds is defined by the argument (its `source` and `expr_range` are in the
  including file, so editing it edits the argument), and what the argument
  reads from the including module has `origin: includer`. In Rust a request
  is a `ViewTarget` (`Module`, or `Instance` by the instance's opaque id)
  within the root; `project_views` projects every target of a root at once
  and `ViewProjections::attach_evaluation` values them all from the root's
  evaluation. `graphcal view --instance <id>`, the LSP request's `target`
  (`{kind: "module"}` by default, `{kind: "instance", instance}`) and the
  Wasm `evaluateView(bindings, target)` take one. The page keys models,
  requests and panes by `ViewTarget` (`src/target.ts`: a module, or a root
  module and an instance id), never by module alone; the VS Code host asks
  the extension with `{type: "request", target}`, which the extension turns
  into the root document's request. An instance view is not
  compared with the base ref; inline calls have no target (they are
  evaluated in place).
- Ids are opaque. A node's `id` is derived from the declaration's identity
  (`view_model/wire_key.rs`), so it is unique, stable across evaluations and
  equal between the two refs of a comparison. An include instance's
  declarations are owned by the instance, so an instance's nodes have ids
  apart from its module viewed alone, and equal to the ids its readers use:
  `y`'s dependency `p::out` in the root's view is the `out` of `p`'s view,
  which is how the page wires a pane to its reader (`src/cross.ts`; an
  include alias, which has an id of its own, is matched by name). The page
  compares and looks ids up and never takes them apart: what it shows
  travels in `label` (the module's spelling: `margin`, `num`,
  `e::p_budget`) and `name`. Pairs of ids are keyed by both ends
  (`src/edges.ts`), not by a joined string.
- A node's `kind` is what it declares (`param`, `const`, `node`, `assert`);
  `origin` is where it comes from (`local`, `imported`, `included` with its
  instance `{ id, name, bound }`, `called` with the inline call `scale(…)`
  whose output it is (each call has a node of its own, with no value), or
  `includer` in an instance's view).
- `axes` refer to `indexes[].id`. An index lists its `entries` as
  `{ key, display }`; the entries of an indexed value refer to `key`, and the
  page shows `display` (a variant, a coordinate such as `0.5 s`, a position
  such as `#2`, as `graphcal eval` does). Structural `Fin(N)` axes are listed
  too, with no declaring `module`.
- Edits replace compiler ranges only. `source.expr_range` is the text after
  `=`, parentheses included, and is null when several declarations share it
  (the columns of a multi-declaration table). `source.cells` lists each
  written cell of a table definition by its entry keys (with the node id,
  the slot) and range; a cell with no range is read-only, and the page never
  parses source text to find one. `element` says what one value is, so a
  typed literal is built only for a quantity (an `Int` cell takes `5` as
  typed; a quantity cell gets `5.0` and the unit that cell is shown in).
  Planning an edit is pure (`src/editing.ts`).
- `dimension` is the element type as the declaration spells it (`Torque`,
  `Dimensionless / Time`). Values follow `graphcal eval`: a quantity in its
  display unit, else in the default label of its dimension. A display unit
  that cannot be applied leaves the SI value, and the node lists why in
  `presentation`; every host projects the same way (`graphcal view` also
  prints those lines and exits with 1, as `graphcal eval` does).
- `module.id` and `node.module` are module paths as the loader records them
  for each file, the root included.

`web/view/src/contract/` holds `graphcal view` output for a few fixtures.
The CLI test `view_contract_fixtures_are_current` keeps it current
(`GRAPHCAL_UPDATE_VIEW_CONTRACT=1 cargo test -p graphcal --test view`
rewrites it), and `src/contract.test.ts` checks that it parses and that
every dependency, axis and cell joins.

## Edits and revisions

A host numbers the states of its sources: the GitHub host makes a revision
of the whole project per load, edit and reset; the VS Code extension sends
each copy of a file with a revision and answers an applied edit with the
copy's new one. Every model is recorded with the revision it was projected
from (`Host.projectedAt`), so its ranges are read on that revision, and a
projection that finishes after the sources moved on is not kept
(`src/modelCache.ts`).

An edit is a range of one file at a revision and its replacement
(`SourceEdit` in `src/revisions.ts`), never a whole file. Edits take turns;
on its turn the host places an edit on its latest text, moving the range
past the edits applied since its revision (each file keeps a short history
of them) and refusing it when one of them touched the range or the revision
is older than the history. Two cell edits made on one model therefore both
land, whatever the evaluation in between does. The VS Code host sends the
placed edit as offsets on its latest copy's revision, with one edit in flight
at a time, and validates the extension's messages with zod; the code pane
sends what was typed as such an edit on the revision its text is based on.

## The published viewer

The page holds no data and reads GitHub through the Graphcal browser
extension (the `chrome-graphcal` repository): each read is one message to the
extension (`src/github.ts`, reached as in `src/extension.ts`), which fetches
github.com with the browser's own GitHub sign-in and answers with the data.
So a repository the person can open on github.com, private or not, opens in
the viewer, and there is no token. Without the extension the page says so
and links to it. The page picks the project's files by the loader's rule
(`src/repoProject.ts`: the nearest `graphcal.toml` above the file is the root,
nested projects are left out, the browser engine's size limits are checked
first), and evaluates in the browser with the Wasm engine.

The page's Content-Security-Policy (injected at build in `vite.config.ts`)
allows no connection but its own origin and no third-party script; extension
messaging is not a connection. Edits are a what-if over the sources read;
nothing is written back.

The page arranges a code pane and the sheet (`src/editor.ts`, CodeMirror with
the playground's lexical colouring): the toggle at the top right shows the
code, both side by side (the split is dragged), or the sheet. The code pane
shows the main sheet's file with the sheet's values inlaid at the declaring
lines; typing re-evaluates after a pause (⌘/Ctrl-Enter at once) as a what-if
like a cell edit, and the selection is shared both ways. The pane never
replaces what was typed: it remembers the host's text its document is based
on, takes a change from the host in under the typing (`src/rebase.ts`), keeps
typing the host refused, and sends pending typing before it shows another
file. The breadcrumb at the
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

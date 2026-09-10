# Maintaining English and Japanese documentation

## Layout and deployment

| Edition | Source | Configuration | Public URL |
| --- | --- | --- | --- |
| English (editorial source) | `docs/en/` | `zensical.toml` | `https://graphcal.org/docs/` |
| Japanese | `docs/ja/` | `zensical.ja.toml` | `https://graphcal.org/docs/ja/` |

The relative Markdown page paths match in both editions. English public URLs
have not moved. Both editions build separately under `target/docs-site/` and
are assembled with the standalone playground into one GitHub Pages artifact.
Only a successful main-branch build deploys; PRs build and test without deploying.
Human translation approval happens during **PR review and merge**, not in an
agent-generated "reviewed" flag or a second manual publishing gate.

Zensical currently localizes theme templates but does not manage content
translations, fallback pages, or translation freshness. Do not nest the Japanese
source tree inside the English `docs_dir`: hiding pages from navigation does
not exclude them from a build/search index. Do not depend on unsupported
`exclude_docs`, `draft_docs`, or MkDocs translation plugins.

The header selector opens each edition's home page. A link above the page title
opens the corresponding page in the other language. There are no automatic
browser-language redirects and no silently copied English fallback pages.
The source checker requires complete page parity, including pages outside nav.
The previously unlisted plots reference is now included in both navigations
and sitemaps.

### Shared resources

`docs/en/assets/` owns the screenshot and canonical tutorial example projects.
Japanese Markdown uses `/docs/assets/...` links; it does not duplicate executable
projects. `web/playground/examples/catalog.json` points to those same English-owned
sources. Both editions use the same `/playground/` application and example IDs.
Playground UI, CLI diagnostics, and editor extensions are **not** localized by
this change.

### Search and metadata

Each Zensical build has its own `search.json`, language setting, and sitemap.
Japanese search is tested with `単位`, `次元`, `型`, and `Dimensionless` in real
browsers. Do not configure Lunr or `plugins.search.lang`: Zensical's search
language comes from `theme.language`.

Some of Disco's own search-dialog labels (such as "Search", "results", and
"Filters") remain English in Zensical 0.0.58 even though the header, navigation,
copy tooltips, and other theme text are Japanese. We use the supported Japanese
search configuration rather than patching minified third-party UI code.

`internals/docs-theme/main.html` preserves Zensical's standard metadata but omits
its default HTML-head alternates. Those defaults identify the language home
pages as equivalents of every deep page. Simply replacing them with page URLs
is also unsuitable in 0.0.58: the client locale router treats each head alternate
as a **site root** and fetches `sitemap.xml` below it, causing bogus requests and
incorrect routing assumptions. Instead, `check-docs.py sitemaps` publishes
reciprocal, fully-qualified `xhtml:link`/`hreflang` entries in both sitemaps,
including each page's own language. This is the standard sitemap method,
[equivalent to HTML hreflang for Google](https://developers.google.com/search/docs/specialty/international/localized-versions).
Each HTML document keeps its own canonical URL. The root sitemap index discovers
both locale sitemaps. Template-generated counterpart links remain ordinary,
root-relative links and work in local preview without fetching the live site.

The small `404.html` override supplies the missing skip-link target in Zensical's
stock error page and translates the Japanese build's error title. GitHub Pages
still uses the **single English root 404**; nested 404 files do not implement
language-sensitive server routing.

## Local commands

Install the existing playground/source-build prerequisites from
`docs/en/installation.md`, plus Zensical **0.0.58**, uv, and just. CI pins uv to
**0.12.12** and installs Zensical explicitly. For an isolated pinned renderer,
`uvx --from zensical==0.0.58 zensical ...` is useful without changing your global
installation.

```sh
# Fast source/configuration checks and checker/assembly regression tests (no Wasm).
just docs-check

# Render both editions only; fails on warnings and removes stale locale output.
just docs-render

# Full artifact, including one shared verified playground build.
just docs-build

# English live preview.
just docs-serve

# Japanese prose/layout live preview.
just docs-serve-ja

# Production-like preview of both languages, shared assets, and playground.
just site-serve
# http://127.0.0.1:4173/docs/ja/

# Full build and Chromium/Firefox/WebKit tests, as in CI.
just playground-browser-test
```

A Japanese-only development server does not serve English-owned assets or the
other edition. Use `site-serve` for shared-resource and language-switch review.
Never bypass the playground/embedded-engine integrity checks to make a docs build
pass. Generated HTML, engine files, caches, and browser traces are not source.

## Updating a translation

1. Edit the English page and its Japanese counterpart together. Keep all
   technical requirements, restrictions, error cases, examples, and table rows.
2. Keep code fences **byte-for-byte identical**, including comments, language
   tags, commands, and expected output. Prose, headings, table descriptions,
   image descriptions, and navigation labels are translated; identifiers,
   keywords, dimensions, units, numeric values, and inline code are not.
3. Keep Japanese headings' explicit ASCII IDs aligned with the English page's
   generated IDs (`## 次元 { #dimensions }`). This preserves links such as
   `type-system.md#domain-constraints` across editions. When adding or renaming a
   heading, check the built IDs in both editions. Retain old stable IDs where
   practical instead of breaking existing fragment links.
4. Record **only the pairs you updated**:

   ```sh
   ./internals/check-docs.py record language/dimensions-and-units.md tutorial/step2-dimensions-and-units.md
   ```

   The manifest records both the English source digest and Japanese translation
   digest. It is a synchronization record, not proof of translation accuracy or
   human approval. Review the prose and manifest diff together. A source-only
   typo correction that needs no Japanese change still requires explicitly
   recording the pair and explaining that judgment in review.
5. Run `just docs-check`, then `just playground-browser-test`. A renderer warning,
   stale/missing translation, changed example, missing fragment, or incorrect
   search/canonical/sitemap target must be fixed rather than ignored.
6. Obtain human technical/Japanese review before merging. AI translations are
   drafts until that review; an agent must not assert that review occurred.

For a new page, add both translations and both navigation entries. For deletion,
remove both pages and links, then run `record` with the deleted path to remove
its manifest entry. Renames are a paired removal/addition and require checking
all incoming links. There is intentionally no blanket "mark everything fresh"
command and no machine-generated "reviewed" status.

Do not postpone an urgent English safety correction for translation convenience.
Translate/review the correction promptly. If a Japanese section genuinely must
be withdrawn, change the explicit publication/coverage policy and navigation in
a reviewed change; do not simply weaken CI or publish stale text under a
Japanese language tag.

## Japanese editorial conventions

- Use natural, polite technical Japanese (`です`/`ます`). Avoid literal English
  sentence structure when it obscures the technical meaning.
- Use these terms consistently; retain an English term on first use where useful:

  | English | Japanese |
  | --- | --- |
  | dimension | 次元 |
  | unit | 単位 |
  | type safety | 型安全性 |
  | parameter | パラメーター |
  | node | ノード |
  | index | インデックス |
  | binding | 束縛 |
  | namespace | 名前空間 |
  | reactive computation | リアクティブ計算 |
  | domain constraint | ドメイン制約（値の有効範囲） |
  | nominal / structural | 公称 / 構造的 |
  | projection | 射影 |

- Distinguish a physical dimension from an array's number of axes. Do not use
  "次元" ambiguously where "軸" or "ランク" is required.
- Preserve Graphcal's explicitness: no implicit unit/type conversion, type
  inference, automatic lifting, or implicit null propagation. Translate
  **must**, **cannot**, and **only** as requirements, not suggestions.
- Never translate code keywords, CLI flags, unit symbols, or test outputs into
  invented Japanese syntax. Keep SI values, bounds, and failure semantics exact.
- If the English source appears wrong, report it and fix both editions in a
  reviewed correction; do not silently give the two editions different semantics.

## Validator reading order

These documentation tools do not change the compiler's dependency graph:

1. `docs_localization.py`: validated page/locale values, source/config/manifest checks.
2. `docs_html.py`: HTML parsing and URL resolution at the rendered-document boundary.
3. `docs_sitemap.py`: sitemap parsing/serialization over page and locale values.
4. `check-docs.py`: filesystem/CLI shell; source checks, explicit recording,
   sitemap annotation, and artifact checks.
5. `docs-localization-tests.py`: positive and fail-closed validator fixtures.
6. `site-assemble.mjs` / `site-assemble.test.mjs`: isolated-output assembly and
   rebuild/preflight regression tests.
7. `web/playground/tests/docs-localization.spec.ts`: real-browser integration.

Reference APIs:
[Zensical languages](https://zensical.org/docs/setup/language/),
[theme overrides](https://zensical.org/docs/customization/),
[search compatibility](https://zensical.org/docs/compatibility/mkdocs/plugins/),
[uv scripts](https://docs.astral.sh/uv/guides/scripts/).

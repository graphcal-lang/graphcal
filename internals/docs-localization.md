# Maintaining English and Japanese documentation

## Layout and scope

| Edition | Source | Configuration | Public URL |
| --- | --- | --- | --- |
| English (editorial source) | `docs/en/` | `zensical.toml` | `https://graphcal.org/docs/en/` |
| Japanese | `docs/ja/` | `zensical.ja.toml` | `https://graphcal.org/docs/ja/` |

Each edition builds separately under `target/docs-site/en/` or
`target/docs-site/ja/`. Keep this repository to configuration, content, and
Graphcal's deployment glue. Language switching, search, hreflang/sitemaps,
translation tracking, page-level redirects, and theme fixes belong to Zensical.
Accept missing features or report them upstream rather than maintaining custom
implementations. See [Zensical's roadmap](https://zensical.org/roadmap/).

Both configs use `extra.alternate` and Zensical's built-in language selector.
Sibling edition URLs allow it to preserve the current page. There are no theme
overrides or silently copied English fallback pages.

### Deployment and shared resources

`internals/site-assemble.mjs` combines the editions and standalone playground
with `site-root/` into one GitHub Pages artifact. Since Pages cannot send HTTP
redirects, static pages redirect the domain root and `/docs/` to `/docs/en/`.
Old English deep links are not redirected. The assembly copies the English 404
to the site root; `robots.txt` lists both edition sitemaps.

`docs/en/assets/` owns shared images, styles, and canonical tutorial projects.
Japanese pages link to `/docs/en/assets/...` rather than duplicating them.
`web/playground/examples/catalog.json` uses the same sources, and both editions
link to the shared `/playground/`.

Only successful main-branch builds deploy. Pull requests build without deploying;
human translation approval happens during review and merge.

## Local commands

Install the playground/source-build prerequisites from `docs/en/installation.md`,
plus Zensical **0.0.68** (the CI version) and just.

```sh
# Render both editions, failing on warnings.
just docs-render

# Build and verify the complete artifact, including the playground.
just docs-build

# Preview a single edition.
just docs-serve
just docs-serve-ja

# Preview both editions, shared assets, and playground.
just site-serve
# http://127.0.0.1:4173/docs/en/

# Full build and Chromium/Firefox/WebKit tests.
just playground-browser-test
```

Single-edition servers do not serve the other edition; the Japanese server also
does not serve English-owned assets. Use the assembled preview for language
switching and shared-resource review. Never bypass embedded-engine integrity
checks. Generated HTML, engine files, caches, and browser traces are not source.

## Translation workflow

1. Update the English page and Japanese counterpart together, preserving technical
   requirements, restrictions, error cases, examples, and table rows.
2. Keep the same page paths and navigation structure in both editions, including
   pages outside navigation. Add, rename, or delete pages and incoming links in
   both editions.
3. Keep code fences **byte-for-byte identical**, including language tags, comments,
   commands, and expected output. Translate prose and labels, not identifiers,
   keywords, dimensions, units, numeric values, or inline code.
4. Align Japanese headings' explicit ASCII IDs with the English generated IDs
   (`## 次元 { #dimensions }`). Check rendered IDs when changing headings.
5. Run `just docs-build` and review both editions in the assembled preview,
   including the page-preserving header selector and shared assets.
6. Obtain human technical/Japanese review before merging. Reviewers check that
   the editions are synchronized; there is no freshness manifest or custom
   parity checker. AI translations remain drafts until human review.

Do not delay urgent English safety corrections for translation convenience;
translate and review the corresponding Japanese correction promptly.

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

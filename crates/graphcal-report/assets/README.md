# Vendored Vega bundles

Minified browser bundles embedded into self-contained HTML artifacts by
`graphcal-report` (see `src/vega_assets.rs`). Vendoring keeps generated pages
working offline and from `file://` paths with no CDN dependency.

| File | Package | Version | License |
|---|---|---|---|
| `vega.min.js` | [vega](https://www.npmjs.com/package/vega) | 5.33.1 | BSD-3-Clause |
| `vega-lite.min.js` | [vega-lite](https://www.npmjs.com/package/vega-lite) | 5.23.0 | BSD-3-Clause |
| `vega-embed.min.js` | [vega-embed](https://www.npmjs.com/package/vega-embed) | 6.29.0 | BSD-3-Clause |

All three are Copyright (c) University of Washington Interactive Data Lab and
contributors, distributed under the BSD-3-Clause license
(<https://github.com/vega/vega/blob/main/LICENSE>).

To update: download `https://cdn.jsdelivr.net/npm/<package>@<version>/build/<file>`
for each pinned version, replace the files here, and update the version
constants in `src/vega_assets.rs` together with this table.

SHA-256 of the vendored files:

```text
463f3db6a40b20e9747b4ed38f37ed0add508838f9141b1cf8366784b07b30c8  vega.min.js
58c27358e26f2d319cf62f45bc17a4c8362f08645001df2ec8d341eee4097c7f  vega-lite.min.js
12d02acfbe3ec59ef9a37dd4822a2e04e2961b5bbb671bbe661d2221715b99da  vega-embed.min.js
```

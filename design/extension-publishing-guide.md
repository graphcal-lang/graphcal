# Publishing Graphcal Editor Extensions

This document covers the detailed steps and requirements for publishing the Graphcal VS Code extension and Zed extension.

## Current State of the Extensions

Both extensions are functional but unpublished:

| Aspect | VS Code | Zed |
|--------|---------|-----|
| Location | `editors/vscode/` | `editors/zed/` |
| Syntax Highlighting | TextMate grammar | Tree-sitter grammar |
| LSP Integration | Yes | Yes |
| Version | 0.0.2 | 0.0.2 |
| Publisher field | `"graphcal"` | N/A (author: `shunichironomura`) |
| License in package.json | `"MIT"` | N/A |
| LICENSE file in repo | **Missing** | **Missing** |

---

## Part 1: VS Code Extension Publishing

### References

- [Publishing Extensions (Official Docs)](https://code.visualstudio.com/api/working-with-extensions/publishing-extension)
- [VS Code Marketplace](https://marketplace.visualstudio.com/vscode)
- [Open VSX Registry](https://open-vsx.org/)
- [HaaLeo/publish-vscode-extension GitHub Action](https://github.com/HaaLeo/publish-vscode-extension)

### Prerequisites

1. **Node.js** and **npm** installed.
2. **`@vscode/vsce`** CLI tool (already in `devDependencies`).
3. A **Microsoft account** (for Azure DevOps).
4. An **Azure DevOps organization**.

### Step 1: Add a LICENSE File

The repository currently has no `LICENSE` file. Add an MIT license file at the repository root. The VS Code Marketplace strongly encourages including a license.

### Step 2: Create a Publisher on the VS Code Marketplace

1. Go to the [Visual Studio Marketplace publisher management page](https://marketplace.visualstudio.com/manage/publishers/).
2. Log in with a Microsoft account.
3. Click **"Create publisher"**.
4. Choose a publisher ID (currently `"graphcal"` is set in `package.json` — the publisher ID must match).
5. Fill in the display name and other details.

> **Decision needed:** The current `publisher` field is `"graphcal"`. Decide whether to use an organization publisher (e.g., `"graphcal"`) or a personal publisher (e.g., `"shunichironomura"`). If changing, update `package.json` accordingly.

### Step 3: Create an Azure DevOps Personal Access Token (PAT)

1. Go to [Azure DevOps](https://dev.azure.com/).
2. Select your organization (or create one).
3. Open **User settings** (top right) → **Personal access tokens**.
4. Click **"New Token"**.
5. Configure:
   - **Name:** e.g., `vscode-marketplace-publish`
   - **Organization:** Select **"All accessible organizations"** (common mistake: selecting a specific org).
   - **Scopes:** Select **"Marketplace" → "Manage"** (not just "publish").
   - **Expiration:** Set an appropriate expiration date.
6. Click **Create** and **copy the token immediately** (it won't be shown again).

### Step 4: Prepare `package.json` for Publishing

The current `package.json` is mostly ready but needs the following additions:

```jsonc
{
  // Already present and correct:
  "name": "graphcal",
  "displayName": "Graphcal",
  "description": "Graphcal language support: syntax highlighting and Language Server Protocol (LSP) integration",
  "version": "0.0.2",
  "publisher": "graphcal",
  "license": "MIT",
  "engines": { "vscode": "^1.75.0" },
  "categories": ["Programming Languages"],

  // MISSING — should be added:
  "icon": "icon.png",              // 128x128 or 256x256 PNG (NOT SVG)
  "repository": {
    "type": "git",
    "url": "https://github.com/shunichironomura/graphcal"
  },
  "homepage": "https://github.com/shunichironomura/graphcal",
  "bugs": {
    "url": "https://github.com/shunichironomura/graphcal/issues"
  }
}
```

**Key requirements:**
- **Icon**: Must be PNG or JPEG, not SVG. Recommended 128x128 or 256x256 pixels.
- **Repository URL**: Strongly recommended for marketplace listing.
- **README.md**: Must be present in `editors/vscode/` (the Marketplace uses it as the extension's landing page). The default scaffolded README will cause `vsce` to warn.

### Step 5: Prepare a README.md for the Extension

Create or update `editors/vscode/README.md` with:
- What the extension does (syntax highlighting, LSP features).
- Installation instructions (how to install the `graphcal` binary for LSP).
- Configuration options (`graphcal.lsp.enabled`, `graphcal.lsp.path`).
- Screenshots (optional but recommended).

### Step 6: Review `.vscodeignore`

The current `.vscodeignore` contains:
```
src/**
node_modules/**
tsconfig.json
.vscode/**
```

Consider also adding:
```
package-lock.json
.gitignore
```

This keeps the packaged `.vsix` file small.

### Step 7: Build, Package, and Test Locally

```bash
cd editors/vscode
npm install
npm run compile
npx vsce package
```

This produces a `graphcal-0.0.2.vsix` file. Install it locally to test:
```bash
code --install-extension graphcal-0.0.2.vsix
```

### Step 8: Login and Publish

```bash
npx vsce login graphcal        # Enter PAT when prompted
npx vsce publish                # Publishes to the Marketplace
```

The extension should appear on the Marketplace within a few minutes.

For future version bumps:
```bash
npx vsce publish patch   # 0.0.2 → 0.0.3
npx vsce publish minor   # 0.0.2 → 0.1.0
npx vsce publish major   # 0.0.2 → 1.0.0
```

### Optional: Publish to Open VSX Registry

[Open VSX](https://open-vsx.org/) is a vendor-neutral alternative to the VS Code Marketplace, used by VS Code forks like VSCodium.

1. Create an [eclipse.org](https://eclipse.org) account (include your GitHub username).
2. Log in at [open-vsx.org](https://open-vsx.org/) via GitHub OAuth.
3. Link your Eclipse account on the Open VSX profile page.
4. Sign the [Eclipse Foundation Open VSX Publisher Agreement](https://open-vsx.org/).
5. Generate an access token on the Open VSX settings page.
6. Create the namespace and publish:
   ```bash
   npx ovsx create-namespace graphcal --pat <token>
   npx ovsx publish --pat <token>
   ```
7. To get "verified" status, [claim namespace ownership](https://github.com/EclipseFdn/open-vsx.org/issues) via a public GitHub issue.

### Optional: CI/CD Automation

Use the [HaaLeo/publish-vscode-extension](https://github.com/HaaLeo/publish-vscode-extension) GitHub Action to automate publishing to both marketplaces on git tag pushes:

```yaml
# .github/workflows/publish-vscode.yaml
name: Publish VS Code Extension
on:
  push:
    tags:
      - 'vscode-v*'
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npm ci
        working-directory: editors/vscode
      - name: Publish to VS Code Marketplace
        uses: HaaLeo/publish-vscode-extension@v2
        with:
          pat: ${{ secrets.VSCE_PAT }}
          registryUrl: https://marketplace.visualstudio.com
          extensionFile: editors/vscode/*.vsix
      - name: Publish to Open VSX
        uses: HaaLeo/publish-vscode-extension@v2
        with:
          pat: ${{ secrets.OVSX_PAT }}
```

---

## Part 2: Zed Extension Publishing

### References

- [Developing Extensions (Official Docs)](https://zed.dev/docs/extensions/developing-extensions)
- [zed-industries/extensions Repository](https://github.com/zed-industries/extensions)
- [extensions.toml (live file)](https://github.com/zed-industries/extensions/blob/main/extensions.toml)
- [Zed Extensions Registry](https://zed.dev/extensions)

### Prerequisites

1. A **public GitHub repository** containing the extension code.
2. A valid **LICENSE file** in the extension repository root (required since October 2025).
3. **pnpm** installed (for running `sort-extensions`).

### Step 1: Decide on Repository Structure

Currently, the Zed extension lives at `editors/zed/` inside the main `graphcal` monorepo. The `extension.toml` already has:
```toml
repository = "https://github.com/shunichironomura/graphcal"
```

There are two options:

**Option A: Keep in the monorepo (current structure)**
- The `path` field in the `zed-industries/extensions` `extensions.toml` entry can point to a subdirectory.
- Entry would look like:
  ```toml
  [graphcal]
  submodule = "extensions/graphcal"
  path = "editors/zed"
  version = "0.0.2"
  ```

**Option B: Separate repository**
- Create a dedicated repo (e.g., `graphcal-zed` or `zed-graphcal`).
- Simpler submodule setup but requires maintaining a separate repo.

> **Note:** Other extensions in the Zed registry (e.g., `tombi`) use the `path` field to point to subdirectories, so Option A is viable.

### Step 2: Add a LICENSE File

**Required since October 1, 2025.** The CI in `zed-industries/extensions` will reject PRs without a valid license.

Accepted licenses:
- **Apache 2.0**
- **BSD 2-Clause**
- **BSD 3-Clause**
- **GNU GPLv3**
- **GNU LGPLv3**
- **MIT**
- **zlib**

The license file must be at the **root of the extension repository** (which, in our case using Option A, means the root of `shunichironomura/graphcal`). The filename must have `LICENSE` or `LICENCE` as a prefix (case insensitive).

Since `package.json` already declares MIT, an MIT `LICENSE` file at the repo root would be consistent.

### Step 3: Pin the Grammar to a Specific Commit

The current `extension.toml` references the grammar at `rev = "main"`:
```toml
[grammars.graphcal]
repository = "https://github.com/shunichironomura/graphcal"
rev = "main"
path = "tree-sitter-graphcal"
```

For a published extension, this should be pinned to a **specific commit SHA** or **tag** rather than a branch name. This ensures reproducible builds and prevents unexpected breakage. Update `rev` to a specific commit hash or tag before publishing.

### Step 4: Fork and Clone `zed-industries/extensions`

```bash
# Fork on GitHub first (use personal account, not an organization — Zed staff can push to personal forks)
gh repo fork zed-industries/extensions --clone
cd extensions
```

### Step 5: Add Graphcal as a Git Submodule

```bash
# Must use HTTPS URL, not SSH
git submodule add https://github.com/shunichironomura/graphcal.git extensions/graphcal
```

### Step 6: Add Entry to `extensions.toml`

Add the following entry to the top-level `extensions.toml`:

```toml
[graphcal]
submodule = "extensions/graphcal"
path = "editors/zed"
version = "0.0.2"
```

### Step 7: Sort Extensions

```bash
pnpm sort-extensions
```

This ensures `extensions.toml` and `.gitmodules` entries are alphabetically sorted (required by CI).

### Step 8: Commit and Open a Pull Request

```bash
git add .
git commit -m "Add graphcal extension"
git push origin main  # or your fork's branch
```

Open a PR to `zed-industries/extensions`. The CI pipeline will:
1. Validate Git submodule configuration (HTTPS URLs).
2. Check for a valid license file.
3. Validate the extension manifest (`extension.toml`).
4. Attempt to package the extension using the `zed-extension` CLI.

### Step 9: Wait for Review and Merge

Once merged by the Zed team, the extension is automatically packaged and published to the [Zed extension registry](https://zed.dev/extensions).

### Updating After Initial Publication

1. Make changes in the main `graphcal` repo and push.
2. In your fork of `zed-industries/extensions`:
   ```bash
   git submodule update --remote extensions/graphcal
   ```
3. Update `version` in both:
   - `editors/zed/extension.toml` (in your repo)
   - `extensions.toml` (in the zed-industries/extensions fork)
4. The versions must match.
5. Open a PR.

There is a [community GitHub Action](https://github.com/huacnlee/zed-extension-action) to automate this process.

---

## Part 3: Shared Prerequisites / Action Items

### Action Items Checklist

#### Must-have (blocking publication)

- [ ] **Add a `LICENSE` file at the repository root** — MIT recommended (consistent with VS Code `package.json`). Required by both Zed (mandatory) and VS Code Marketplace (strongly recommended).
- [ ] **Create a VS Code Marketplace publisher** — Register `"graphcal"` publisher at https://marketplace.visualstudio.com/manage/publishers/.
- [ ] **Create an Azure DevOps PAT** — With "Marketplace (Manage)" scope, "All accessible organizations".
- [ ] **Add `repository` field to VS Code `package.json`** — Points to GitHub repo.
- [ ] **Add an extension icon** (VS Code) — 128x128 or 256x256 PNG file.
- [ ] **Write a user-facing `README.md`** in `editors/vscode/` — Features, install instructions, configuration.
- [ ] **Pin the tree-sitter grammar `rev`** (Zed) — Change from `"main"` to a specific commit SHA or tag.

#### Nice-to-have

- [ ] **Add a `CHANGELOG.md`** in `editors/vscode/`.
- [ ] **Set up CI/CD** for automated publishing (GitHub Actions).
- [ ] **Publish to Open VSX** in addition to VS Code Marketplace.
- [ ] **Add screenshots/GIFs** to the VS Code README for better marketplace presence.

### Naming Considerations

- **VS Code**: The extension name `"graphcal"` and display name `"Graphcal"` are fine.
- **Zed**: Extension IDs and names must **not** contain "zed" or "Zed". The current ID `"graphcal"` and name `"Graphcal"` are compliant.

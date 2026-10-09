---
icon: material/download
---

# インストール { #installation }

Graphcal は `graphcal` というコマンドラインプログラム 1 つで動きます。Linux (x86_64、ARM64)、macOS (Intel、Apple Silicon)、Windows (x86_64) 向けにビルド済みバイナリを配布しています。

## インストーラスクリプトでインストールする { #install-with-the-installer-script }

macOS と Linux では次のコマンドを実行します。

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
```

Windows では PowerShell で次のコマンドを実行します。

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.ps1 | iex"
```

インストールが終わったら、新しいターミナルを開くと `graphcal` を使えます。

## パッケージマネージャでインストールする { #install-with-a-package-manager }

Graphcal はまだプレリリースなので、以下のコマンドではバージョンを明示する必要があります。`<version>` は [リリース一覧](https://github.com/graphcal-lang/graphcal/releases) にあるバージョン (`0.0.1-alpha.35` など) に置き換えてください。

### cargo-binstall { #cargo-binstall }

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) はビルド済みバイナリをダウンロードします。

```bash
cargo binstall graphcal@<version>
```

### cargo install { #cargo-install }

`cargo install` は [crates.io](https://crates.io/crates/graphcal) のソースから Graphcal をビルドします。Rust 1.95 以降が必要です。Rust は [rustup.rs](https://rustup.rs/) から入手できます。

```bash
cargo install graphcal@<version> --locked
```

## ソースからビルドする { #build-from-source }

リポジトリをチェックアウトしてビルドするには、次のツールが必要です。

- `rust-toolchain.toml` で固定した Rust ツールチェーン (rustup で管理)
- その `wasm32-unknown-unknown` ターゲット (リポジトリのルートで
  `rustup target add wasm32-unknown-unknown` を実行)
- **`Cargo.lock` で解決された `wasm-bindgen` と同じバージョン**の `wasm-bindgen-cli`
  (`cargo install wasm-bindgen-cli --version <locked-version> --locked` でインストール)
- `PATH` 上にある Binaryen の `wasm-opt`。CI は Binaryen 117 を使っています。ビルド済みのものは
  [Binaryen のリリース](https://github.com/WebAssembly/binaryen/releases/tag/version_117) から入手できます。

ツールを揃えたら、通常どおりビルドします。

```bash
cargo build
```

初回のビルドは通常より時間がかかります。`cargo check` と Clippy を実行するときも、これらのツールが必要です。

## インストールの確認 { #verify-installation }

```bash
graphcal --version
# graphcal <version> (commit: <sha>)
```

## GitHub Actions で使う { #use-in-github-actions }

[`setup-graphcal`](https://github.com/graphcal-lang/setup-graphcal) アクションを使うと、GitHub Actions のランナーに `graphcal` をインストールできます。次のワークフローは、push と pull request のたびに `.gcl` ファイルをチェックし、モデルを評価します。

```yaml
name: Graphcal

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: graphcal-lang/setup-graphcal@v1
        with:
          version: 0.0.1-alpha.35
      - run: graphcal check
      - run: graphcal eval model.gcl
```

アサーションが失敗すると、ジョブも失敗します。alpha 版の間はリリースごとに Graphcal の仕様が変わるので、`version` を固定して結果を再現できるようにしてください。そのほかの設定は[アクションの README](https://github.com/graphcal-lang/setup-graphcal#readme) を参照してください。

## エディターのセットアップ { #editor-setup }

最良の体験を得るには、エディター連携をセットアップして、シンタックスハイライト、診断、そして**計算値を表示するインレイヒント**を利用してください。VS Code、Zed、Helix、Neovim の手順については[エディターのセットアップ](editor-setup.md)を参照してください。

## 次のステップ { #next-steps }

[チュートリアル](tutorial/index.md)に進んで、最初の Graphcal ファイルを書いてみましょう。

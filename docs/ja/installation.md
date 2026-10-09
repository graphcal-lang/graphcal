---
icon: material/download
---

# インストール { #installation }

## ビルド済みバイナリをインストールする { #install-a-prebuilt-binary }

ビルド済みバイナリは [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases) で公開しており、Rust ツールチェーンは必要ありません。

macOS と Linux の場合:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
```

Windows (PowerShell) の場合:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.ps1 | iex"
```

インストーラは `graphcal` を `~/.local/bin` (`$XDG_BIN_HOME` が設定されていればそのディレクトリ) に置き、必要に応じてシェルのプロファイルを書き換えて、そのディレクトリを `PATH` に追加します。インストール後は新しいターミナルを開いてください。

バイナリは x86_64 と ARM64 の Linux、Intel と Apple Silicon の macOS、x86_64 の Windows 向けに用意しています。Linux 版は静的リンクなので、Alpine などの musl ベースのディストリビューションでも動きます。

特定のバージョンをインストールするには、URL の `latest/download` を `download/v<version>` に置き換えます。たとえば `download/v0.0.1-alpha.35` です。

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) を使う場合も、同じバイナリがダウンロードされます。Graphcal がプレリリースの間は、バージョンを明示してください:

```bash
cargo binstall graphcal@<version>
```

## crates.io からインストールする { #install-from-cratesio }

crates.io からビルドするには、Rust stable ツールチェーン (1.95 以降) が必要です。Rust がインストールされていない場合は、[rustup.rs](https://rustup.rs/) から入手してください。

```bash
cargo install graphcal --version '^0.0.1-alpha' --locked
```

Graphcal がプレリリースとして公開されている間は、明示的なバージョン指定が必要です。このコマンドは [crates.io](https://crates.io/crates/graphcal) から互換性のある最新の Graphcal リリースをダウンロードしてビルドし、`graphcal` バイナリを `~/.cargo/bin/` にインストールします。

## ソースチェックアウトからビルドする { #build-from-a-source-checkout }

crates.io からのインストールとは異なり、チェックアウトから CLI をビルドすると、
組み込みのブラウザーエンジンもビルドされます。まず、次の前提条件をインストールしてください:

- rustup で管理される、`rust-toolchain.toml` に固定された Rust ツールチェーン。
- その `wasm32-unknown-unknown` ターゲット (リポジトリのルートで
  `rustup target add wasm32-unknown-unknown` を実行)。
- **`Cargo.lock` で解決された `wasm-bindgen` のバージョン**に一致する `wasm-bindgen-cli`:
  `cargo install wasm-bindgen-cli --version <locked-version> --locked`。
- `PATH` 上にある Binaryen の `wasm-opt`。CI は Binaryen 117 を使用しています。ビルド済みの配布物は
  [Binaryen releases](https://github.com/WebAssembly/binaryen/releases/tag/version_117) から入手できます。

その後、通常のコマンドを実行します:

```bash
cargo build
```

CLI はブラウザーエンジンを自動的にビルドして埋め込みます。初回ビルドは、
ネイティブコードと Wasm コードの両方をコンパイルするため時間がかかることがあります。
`cargo check`、Clippy、エディターのチェックでも、このビルドが実行されることがあります。

オフラインでソースビルドを行うには、依存関係を事前に取得し、`CARGO_NET_OFFLINE=true` を設定してください。

公開されているクレートアーカイブには検証済みのエンジンがすでに含まれているため、**crates.io のユーザーは
これらの追加ツールを必要としません**。レポートの成果物は自己完結したままであり、
ネットワーク接続なしで動作します。

## インストールの確認 { #verify-installation }

```bash
graphcal --version
# graphcal <version> (commit: <sha>)
```

コミットのサフィックスは、ビルドがソースのコミットを特定できる場合に表示されます。

## GitHub Actions で使う { #use-in-github-actions }

[`setup-graphcal`](https://github.com/graphcal-lang/setup-graphcal) アクションを使うと、GitHub Actions のランナーにビルド済みの `graphcal` を数秒でインストールできます。たとえば、push と pull request のたびに、リポジトリ内のすべての `.gcl` ファイルをチェックし、モデルを評価するには次のようにします:

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

`graphcal eval` はアサーションが失敗すると 0 以外の終了ステータスで終了するので、ジョブも失敗します。alpha 版の間はリリースごとに言語が変わるため、`version` を固定して CI の再現性を保ってください。省略すると最新のリリースがインストールされます。すべての入力は、アクションの README を参照してください。

## エディターのセットアップ { #editor-setup }

最良の体験を得るには、エディター連携をセットアップして、シンタックスハイライト、診断、そして**計算値を表示するインレイヒント**を利用してください。VS Code、Zed、Helix、Neovim の手順については[エディターのセットアップ](editor-setup.md)を参照してください。

## 次のステップ { #next-steps }

[チュートリアル](tutorial/index.md)に進んで、最初の Graphcal ファイルを書いてみましょう。

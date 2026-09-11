---
icon: material/download
---

# インストール { #installation }

## 要件 { #requirements }

- Rust stable ツールチェーン (1.91 以降)

Rust がインストールされていない場合は、[rustup.rs](https://rustup.rs/) から入手してください。

## crates.io からインストールする { #install-from-cratesio }

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

## エディターのセットアップ { #editor-setup }

最良の体験を得るには、エディター連携をセットアップして、シンタックスハイライト、診断、そして**計算値を表示するインレイヒント**を利用してください。VS Code、Zed、Helix、Neovim の手順については[エディターのセットアップ](editor-setup.md)を参照してください。

## 次のステップ { #next-steps }

[チュートリアル](tutorial/index.md)に進んで、最初の Graphcal ファイルを書いてみましょう。

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

CLI はリリース最適化されたエンジンを自動的に生成して埋め込みます。生成された
JS、Wasm、チェックサムは Git ではなく Cargo のビルドディレクトリに置かれます。ソースやビルド入力の
変更はキャッシュを無効化し、CLI のみの変更はエンジンを再利用します。初回ビルドと
それ以降のエンジン変更時は、ネイティブコードと Wasm コードの両方をコンパイルするため
時間がかかります。`cargo check`、Clippy、エディターのチェックも生成を引き起こすことがあります。
ネイティブのプロファイル/ターゲットごとに独自のキャッシュがあり、`cargo clean` でそれが削除されます。
**リポジトリの外**にそれまで存在しなかった Cargo 設定ファイル (たとえば
新しいグローバル Cargo 設定) を追加した場合は、`cargo clean -p graphcal` を実行して、
その設定がエンジンの次回の入力チェックに含まれるようにしてください。既存の設定ファイルと
リポジトリの `.cargo` ディレクトリは自動的に監視されます。

ビルドスクリプトは前提条件をインストールしたり、古くなったエンジンを黙って使用したりすることはありません。
オフラインでのソースビルドを行うには、依存関係を事前に取得し、`CARGO_NET_OFFLINE=true`
を設定して、外側とネストされた Cargo ビルドの両方がオフラインモードを使用するようにしてください。Wasm ビルドは、
サポートされている古い Rust バージョンでネイティブ CLI をテストしている場合でも、
常にリポジトリに固定されたツールチェーンを使用します。ネイティブのカバレッジ/サニタイザーフラグは、
このブラウザーエンジンには適用されません。

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

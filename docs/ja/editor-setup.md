---
icon: material/puzzle
---

# エディターのセットアップ { #editor-setup }

Graphcal は、組み込みの LSP サーバーを通じて豊富な言語サポートを提供するエディター拡張機能を用意しています。主要な機能は**計算値をインラインで表示するインレイヒント**で、エディターをライブな計算シートに変えます。

## LSP の機能 { #lsp-features }

Graphcal LSP サーバー (`graphcal lsp`) は次の機能を提供します:

| 機能                 | 説明                                                                                                             |
| -------------------- | ---------------------------------------------------------------------------------------------------------------- |
| **診断**             | リアルタイムのエラー報告: パースエラー、次元の不一致、未知の参照、可視性違反                                     |
| **補完**             | 文脈に応じたキーワード、参照、型、次元、インデックス、単位、インポート、組み込み、コンストラクター               |
| **シグネチャヘルプ** | 組み込み関数およびインポートされたプラグイン関数のシグネチャと、アクティブな位置パラメーター                     |
| **ホバー**           | 型、次元、単位、シグネチャ、ドキュメントの情報                                                                   |
| **定義へ移動**       | 参照からローカルまたはインポートされた宣言へジャンプ                                                             |
| **参照の検索**       | アクティブに読み込まれたプロジェクト全体で正規の使用箇所を特定                                                   |
| **リネーム**         | アクティブに読み込まれたプロジェクトが完全なカバレッジを証明できる場合に、シンボルを安全にリネーム               |
| **ドキュメントシンボル** | 現在のファイル内の宣言をアウトライン表示                                                                     |
| **インレイヒント**   | param、node、const 宣言に対して計算値または型のフォールバックを表示                                              |
| **コードアクション** | 可視性、厳密な指数、インポートなど、サポートされている診断に対するクイックフィックス                             |
| **ドキュメントリンク** | ローダーが解決した `import` および `include` パスのクリック可能なリンク                                        |
| **フォーマット**     | 現在のドキュメントをフォーマット (`graphcal format` と同じ)                                                      |

!!! tip "インレイヒント: ライブ計算ビュー"
インレイヒント機能こそが、Graphcal をライブなスプレッドシートのように感じさせるものです。`.gcl` ファイルを編集すると、LSP が計算グラフを評価し、対象となる `param`、`node`、`const` 宣言の横に結果の値を表示します。値が利用できない場合は型がフォールバックとして表示されます。入力を変更すると、依存するすべての値が更新されるのを確認できます。

リネームは、影響する使用箇所を読み込み済みのプロジェクト内ですべて把握できる
シンボルに限られます。開いていないファイルから使われている公開シンボルや、
新しい名前によって参照が曖昧になる場合には、利用できないことがあります。

## VS Code { #vs-code }

VS Code 拡張機能は、(TextMate 文法による) シンタックスハイライトと完全な LSP サポートを提供します。

### インストール { #installation }

Visual Studio Marketplace から、公開されている [Graphcal 拡張機能](https://marketplace.visualstudio.com/items?itemName=Graphcal.graphcal) をインストールします。

VS Code からインストールすることもできます:

1. 拡張機能ビューを開きます。
2. **Graphcal** を検索します。
3. **Graphcal** が公開している拡張機能をインストールします。

### 設定 { #configuration }

| 設定                   | デフォルト   | 説明                          |
| ---------------------- | ------------ | ----------------------------- |
| `graphcal.lsp.enabled` | `true`       | LSP サーバーの有効化/無効化   |
| `graphcal.lsp.path`    | `"graphcal"` | `graphcal` バイナリへのパス   |

`graphcal` が `PATH` 上にない場合は、`graphcal.lsp.path` にバイナリのフルパスを設定してください。

## Zed { #zed }

Zed 拡張機能は、(tree-sitter 文法による) シンタックスハイライトと LSP サポートを提供します。

### セットアップ { #setup }

Zed 拡張機能はまだ公開されていません。当面は開発拡張機能としてインストールしてください:

1. 拡張機能リポジトリをクローンします: `https://github.com/graphcal-lang/zed-graphcal`
2. Zed でコマンドパレットを開きます
3. **"Extensions: Install Dev Extension"** を選択します
4. クローンした `zed-graphcal` ディレクトリに移動します
5. 拡張機能がインストールされ、有効化されます

## Helix { #helix }

Helix は、Graphcal の tree-sitter 文法と組み込みの LSP サーバーを使用できます。

`~/.config/helix/languages.toml` に次を追加します:

```toml
[[grammar]]
name = "graphcal"
source = { git = "https://github.com/graphcal-lang/tree-sitter-graphcal", rev = "main" }

[language-server.graphcal-lsp]
command = "graphcal"
args = ["lsp"]

[[language]]
name = "graphcal"
scope = "source.graphcal"
file-types = ["gcl"]
roots = ["graphcal.toml"]
comment-token = "//"
language-servers = ["graphcal-lsp"]
indent = { tab-width = 2, unit = "  " }
auto-format = true
```

次に、文法を取得してビルドします:

```sh
hx --grammar fetch
hx --grammar build
```

`graphcal` は `PATH` 上で利用可能である必要があります。そうでない場合は、`command` に Graphcal バイナリのフルパスを設定してください。

### Helix のシンタックスハイライトクエリ { #helix-syntax-highlighting-queries }

上記のコマンドはパーサーバイナリをインストール/更新しますが、Helix はカスタム文法リポジトリからのハイライトクエリファイルを自動的にインストールしたり更新したりは**しません**。Graphcal のハイライトは、次の場所から別途読み込まれます:

```text
~/.config/helix/runtime/queries/graphcal/highlights.scm
```

文法を取得した後、ハイライトクエリをインストールします:

```sh
mkdir -p ~/.config/helix/runtime/queries/graphcal
cp ~/.config/helix/runtime/grammars/sources/graphcal/queries/highlights.scm \
  ~/.config/helix/runtime/queries/graphcal/highlights.scm
```

Helix の文法ソースキャッシュに依存したくない場合は、代わりにクエリを直接ダウンロードしてください:

```sh
mkdir -p ~/.config/helix/runtime/queries/graphcal
curl -fsSL \
  https://raw.githubusercontent.com/graphcal-lang/tree-sitter-graphcal/main/queries/highlights.scm \
  -o ~/.config/helix/runtime/queries/graphcal/highlights.scm
```

## Neovim { #neovim }

`nvim-treesitter` を使用する Neovim の場合:

1. tree-sitter の設定で、文法ソースとして `https://github.com/graphcal-lang/tree-sitter-graphcal` を追加します
2. リポジトリの `queries/highlights.scm` からハイライトクエリをコピーします

LSP サポートを得るには、`.gcl` ファイルに対して stdin/stdout 経由で `graphcal lsp` を実行するように Neovim を設定します。

`nvim-lspconfig` を使用した設定例:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "graphcal",
  callback = function()
    vim.lsp.start({
      name = "graphcal",
      cmd = { "graphcal", "lsp" },
    })
  end,
})
```

## その他のエディター { #other-editors }

tree-sitter と LSP をサポートする任意のエディターでは、[`graphcal-lang/tree-sitter-graphcal`](https://github.com/graphcal-lang/tree-sitter-graphcal) から文法をインストールし、`.gcl` ファイルに対して `graphcal lsp` を実行するように言語サーバーを設定してください。

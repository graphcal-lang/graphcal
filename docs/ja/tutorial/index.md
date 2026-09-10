---
icon: material/school
---

# チュートリアル概要 { #tutorial-overview }

このチュートリアルでは、Graphcal をステップごとに学びます。各ステップは前のステップの上に積み重なり、新しい概念を段階的に導入します。

## 作成するもの { #what-youll-build }

このチュートリアルを終える頃には、次のようなエンジニアリング計算を作成できるようになります。

- リアクティブな DAG の中で入力パラメーターと計算ノードを定義する
- コンパイル時チェック付きの物理次元と単位を使う
- 代数的データ型でデータを整理する
- `dag` ブロックと `include` で再利用可能な計算を書く
- プロジェクトを複数のファイルに分割する
- インデックス付きコレクションと集約を扱う

## チュートリアルのステップ { #tutorial-steps }

| ステップ | トピック | 学ぶこと |
|------|-------|-------------------|
| [ステップ 1](step1-hello-graphcal.md) | Hello, Graphcal | パラメーター、ノード、定数、`@` シジル、`graphcal eval` |
| [ステップ 2](step2-dimensions-and-units.md) | 次元と単位 | 物理次元、単位、次元注釈、単位変換 |
| [ステップ 3](step3-structs-and-blocks.md) | 代数的型 | コンストラクター、ペイロード、フィールドアクセス |
| [ステップ 4](step4-functions.md) | DAG ブロック | `dag` ブロックによる再利用可能な計算、`include`、名前付き引数 |
| [ステップ 5](step5-multi-file-projects.md) | 複数ファイルプロジェクト | `import` 宣言、プロジェクトの構成 |
| [ステップ 6](step6-indexed-values.md) | インデックス付き値 | 有限インデックス、`for` 内包、集約、`scan` |

## サンプルの実行 { #running-the-examples }

単一ファイルのステップは[スタンドアロンのプレイグラウンド](https://graphcal.org/playground/)にリンクしているので、何もインストールせずに始められます。フルサイズのエディターはサンプルと共有可能なソース URL に対応しています。コンパイラーと評価器は WebAssembly でローカルに動作し、ソースをアップロードすることはありません。ステップ 5 は複数ファイルプロジェクトを扱うため、CLI が必要です。

プロジェクトをローカルに保存したり、完全な CLI とエディターツールを使ったりするには、[Graphcal をインストール](../installation.md)して、サンプルを `.gcl` ファイルとして実行してください。

```bash
graphcal eval my_file.gcl
```

## 前提条件 { #prerequisites }

ブラウザーで進める場合、JavaScript と WebAssembly が有効な最新のブラウザー以外に前提条件はありません。ローカルで開発する場合は、診断とインレイヒントのために [Graphcal エディターサポート](../editor-setup.md)のあるテキストエディターを使用してください。

準備はできましたか？[ステップ 1: Hello, Graphcal](step1-hello-graphcal.md) から始めるか、[スタンドアロンのプレイグラウンド](https://graphcal.org/playground/)を開いてください。

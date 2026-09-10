---
icon: material/book-open-variant
---

# 言語リファレンス { #language-reference }

このセクションでは、Graphcal のすべての言語機能について形式的なドキュメントを提供します。

## 概要 { #overview }

Graphcal は、計算の**有向非巡回グラフ(DAG)**を中心に構築された、工学計算のためのドメイン固有言語です。すべての `.gcl` ファイルは、パラメーター(入力)、ノード(計算された値)、定数を記述し、それらは明示的な参照によって接続されます。

この言語は階層化された型システムを持ちます:

| 層 | 機能 | 目的 |
|-------|---------|---------|
| 1 | [プリミティブ](type-system.md) | プリミティブ値型(正規の一覧) |
| 2 | [次元](dimensions-and-units.md) | 物理次元の代数(コンパイル時の型) |
| 3 | [単位](dimensions-and-units.md) | 次元に付随する値レベルのスケーリング係数 |
| 4 | [代数的データ型](algebraic-data-types.md) | コンストラクター、ペイロード、パターンマッチング |
| 5 | [インデックス](indexes.md) | コレクションのための有限ラベル集合 |
| 6 | [DAG ブロック](functions.md) | `dag` ブロックと `include` による再利用可能な計算 |

## リファレンスページ { #reference-pages }

<div class="grid cards" markdown>

- :material-graph:{ .lg .middle } **計算モデル**

    ---

    DAG の意味論、`param`/`node`/`const node`、`@` シジル。

    [:octicons-arrow-right-24: 計算モデル](computation-model.md)

- :material-format-list-bulleted-type:{ .lg .middle } **型システム**

    ---

    実体の完全な階層化、正規のプリミティブ一覧、および
    明示的な変換。

    [:octicons-arrow-right-24: 型システム](type-system.md)

- :material-ruler:{ .lg .middle } **次元と単位**

    ---

    次元の代数、単位の定義、変換、プレリュード。

    [:octicons-arrow-right-24: 次元と単位](dimensions-and-units.md)

- :material-shape:{ .lg .middle } **代数的データ型**

    ---

    単一コンストラクター型と複数コンストラクター型、`match` 式、およびジェネリクス。

    [:octicons-arrow-right-24: ADT](algebraic-data-types.md)

- :material-format-list-numbered:{ .lg .middle } **インデックス**

    ---

    名前付き、座標、および `Fin(N)` インデックス。`for`、`scan`、および `unfold`。

    [:octicons-arrow-right-24: インデックス](indexes.md)

- :material-function:{ .lg .middle } **DAG ブロック**

    ---

    `dag` ブロックと `include` による再利用可能な計算。

    [:octicons-arrow-right-24: DAG ブロック](functions.md)

- :material-power-plug:{ .lg .middle } **Extern 関数(プラグイン)**

    ---

    `import plugin` ブロック: 次元シグネチャが宣言された、
    外部から提供される関数。

    [:octicons-arrow-right-24: Extern 関数](extern-functions.md)

- :material-code-parentheses:{ .lg .middle } **式**

    ---

    演算子、優先順位、`if`/`else`、リテラル。

    [:octicons-arrow-right-24: 式](expressions.md)

- :material-file-multiple:{ .lg .middle } **複数ファイルプロジェクト**

    ---

    `import` 宣言、パッケージ依存関係、エイリアス、循環検出。

    [:octicons-arrow-right-24: 複数ファイル](multi-file.md)

- :material-check-decagram:{ .lg .middle } **アサーションと属性**

    ---

    `assert` 宣言、許容誤差チェック、`#[assumes(...)]`。

    [:octicons-arrow-right-24: アサーション](assertions.md)

- :material-chart-line:{ .lg .middle } **プロット**

    ---

    `plot` 宣言、チャートの種類、インタラクティブな可視化。

    [:octicons-arrow-right-24: プロット](plots.md)

- :material-package-variant:{ .lg .middle } **組み込みリファレンス**

    ---

    プレリュードの次元、単位、定数、および関数。

    [:octicons-arrow-right-24: 組み込み](built-ins.md)

</div>

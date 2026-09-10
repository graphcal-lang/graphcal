---
icon: material/numeric-1-circle
---

# ステップ 1: Hello, Graphcal { #step-1-hello-graphcal }

このステップでは、最初の Graphcal ファイルを作成し、3 つの主要な宣言の種類を学びます。

## スプレッドシートとの類比 { #the-spreadsheet-analogy }

スプレッドシートを使ったことがあれば、Graphcal の中核となるモデルはすでに理解できています。

| スプレッドシート | Graphcal | 説明 |
|-------------|----------|-------------|
| 入力セル | `param` | 利用者が与える値で、上書きできます |
| 数式セル | `node` | 他の値から導出される計算値 |
| 名前付き定数 | `const node` | 決して変化しない固定値 |

## 最初のファイル { #your-first-file }

`mass_budget.gcl` というファイルを作成します。

```
param dry_mass: Dimensionless = 1200.0;
param fuel_mass: Dimensionless = 2800.0;
const node margin_factor: Dimensionless = 1.1;

node total_mass: Dimensionless = @dry_mass + @fuel_mass;
node mass_with_margin: Dimensionless = @total_mass * @margin_factor;
node mass_ratio: Dimensionless = @total_mass / @dry_mass;
```

## ブラウザーで試す { #try-it-in-your-browser }

[このサンプルをプレイグラウンドで開く](https://graphcal.org/playground/?example=hello)か、[ソースを読む](/docs/assets/playground/examples/step-1/main.gcl)ことができます。フルサイズのエディターでコードを編集すると、Auto-run が有効な場合は結果が更新されます。または **Run** を押してください。

## ローカルで実行する { #run-it-locally }

```bash
$ graphcal eval mass_budget.gcl
dry_mass         = 1200
fuel_mass        = 2800
margin_factor    = 1.1
total_mass       = 4000
mass_with_margin = 4400
mass_ratio       = 3.333333
```

## コードの理解 { #understanding-the-code }

### パラメーター (`param`) { #parameters-param }

パラメーターは計算への入力です。その既定値は、明示的なコマンドライン束縛で置き換えることができます。

```
param dry_mass: Dimensionless = 1200.0;
```

- **`param`** -- 入力パラメーターを宣言します
- **`dry_mass`** -- 名前 (慣例として `lower_snake_case`)
- **`: Dimensionless`** -- 次元注釈 (`Dimensionless` は単なる数値を意味します)
- **`= 1200.0`** -- 既定値

### 定数 (`const node`) { #constants-const-node }

定数はコンパイル時に確定している固定値です。

```
const node margin_factor: Dimensionless = 1.1;
```

- **`const node`** -- コンパイル時定数を宣言します
- **`margin_factor`** -- 名前 (慣例として `lower_snake_case`)

### ノード (`node`) { #nodes-node }

ノードはリアクティブ計算グラフを構成する計算値です。

```
node total_mass: Dimensionless = @dry_mass + @fuel_mass;
```

- **`node`** -- 計算値を宣言します
- **`@dry_mass`** -- `@` シジルを使ってパラメーターまたはノードを参照します

### `@` シジル { #the-sigil }

`@` 接頭辞は、計算グラフ内の値を参照する方法です。

- `@name` は `param`、`node`、または `const node` を参照します
- 裸の `NAME` は組み込み定数 (`PI`、`E`、`TAU` など) を参照します
- `@` シジルによって、どの値が計算に参加しているかが視覚的に明確になります

## パラメーターの束縛 { #binding-parameters }

繰り返し指定できる `--param` オプションを使って、コマンドラインからパラメーターの値を束縛できます。コマンドラインで与えられなかったパラメーターは、宣言された既定値を保持します。

```bash
$ graphcal eval mass_budget.gcl --param 'dry_mass=1200.0' --param 'fuel_mass=3500.0'
dry_mass         = 1200
fuel_mass        = 3500
margin_factor    = 1.1
total_mass       = 4700
mass_with_margin = 5170
mass_ratio       = 3.916667
```

下流のすべてのノード (`total_mass`、`mass_with_margin`、`mass_ratio`) は自動的に更新されます。

## 命名規則 { #naming-conventions }

Graphcal では次の命名規則を推奨しています。

| 宣言 | 推奨される規則 | 例 |
|-------------|----------------------|---------|
| `param`、`node`、`const node`、`dag` | `lower_snake_case` | `dry_mass`、`total_dv`、`margin_factor` |
| `type`、`index`、`dim` | `PascalCase` | `TransferResult`、`Maneuver` |

## 学んだこと { #what-you-learned }

- 上書き可能な入力値のための **`param`**
- リアクティブグラフ内の計算値のための **`node`**
- コンパイル時定数のための **`const node`**
- グラフの値を参照するための **`@`** シジル
- コマンドラインからパラメーターを束縛するための **`--param`**

## 次のステップ { #next-step }

[ステップ 2](step2-dimensions-and-units.md) では、物理次元と単位を追加して、単位の不一致をコンパイル時に検出します。

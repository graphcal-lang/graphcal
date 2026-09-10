---
icon: material/numeric-4-circle
---

# ステップ 4: DAG ブロック (再利用可能な計算) { #step-4-dag-blocks-reusable-computation }

このステップでは、`dag` ブロックを使って再利用可能な計算を書き、`include` でそれをインスタンス化する方法を学びます。

## DAG ブロックの定義 { #defining-a-dag-block }

`dag` ブロックは、独自のパラメーターとノードを持つ再利用可能なサブ DAG を定義します。すでに知っている `param` / `node` / `@` と同じ構文を使います。

```
dim GravParam = Length^3 / Time^2;

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    pub node v: Velocity = sqrt(@gm / @r);
}
```

`dag` ブロックは名前付きのテンプレートであり、`include` するまで実行されません。

## DAG ブロックのインクルード { #including-a-dag-block }

DAG ブロックをインスタンス化するには `include` を使います。引数リストは必須です (空でも構いません)。出力は `::{ ... }` の波括弧リストで射影され、文は `;` で終わります。

```
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
const node r_earth: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;

include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    ::{ v as v_parking };
```

- **名前付き引数**: `gm: @gm_earth` は `@gm_earth` を `gm` パラメーターに渡します。引数は囲んでいるスコープで評価されます。
- **出力の選択**: `::{ v as v_parking }` は公開された `v` ノードを選択し、`v_parking` に改名します。インクルードされる出力は、DAG が同じファイル内にある場合でも `pub` として宣言されていなければなりません。
- インクルードされたノードは、計算グラフ内の通常のノードになります。

## 複数出力の DAG { #multi-output-dags }

`dag` は複数の出力を公開できます。

```
dag hohmann_transfer {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;

    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    pub node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    pub node total_dv: Velocity = @dv1 + @dv2;
}
```

インクルード箇所で必要な出力を選びます。

```
param target_alt: Length = 35786.0 km;

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
)::{ total_dv as transfer_dv, dv1 as departure_dv };
```

## インクルード全体のエイリアス { #aliasing-the-whole-include }

出力を 1 つの接頭辞のもとにまとめたい場合は、波括弧リストの代わりにインスタンス化全体にエイリアスを付けます。

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt) as parking;
node v_parking: Velocity = @parking::v;
```

1 つの `include` において、エイリアスと波括弧リストは同時に使えません。

## グラフ内での DAG 結果の利用 { #using-dag-results-in-the-graph }

インクルードされた出力は通常のグラフノードであり、`@` で参照します。

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    ::{ v as v_parking };

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
)::{ total_dv as transfer_dv };

node total: Velocity = @v_parking + @transfer_dv;
```

## DAG 本体は分離されている { #dag-bodies-are-isolated }

`dag` 本体からは、自身の宣言、自身のインポート、そして自身のインクルードの出力だけが見えます。囲んでいるファイルのトップレベルスコープからの字句的な継承はありません。トップレベルの `const node` (またはその他のコンパイル時の名前) を `dag` の中で使うには、上の例のようにインクルード箇所で `param` として渡すか、`dag` 本体の中で明示的に `import` してください。完全な規則については[複数ファイルプロジェクト](../language/multi-file.md)を参照してください。

## 学んだこと { #what-you-learned }

- 再利用可能な計算テンプレートを定義するための **`dag`** ブロック
- 名前付き引数で DAG ブロックをインスタンス化する **`include`**
- `::{ name as alias }` で出力を選択・改名する**出力の選択**
- `as` で出力を 1 つの接頭辞のもとにまとめる**インクルード全体のエイリアス**
- 1 つの DAG ブロックからの**複数の出力**
- DAG ブロックはトップレベル宣言と同じ `param` / `node` / `@` 構文を使い、厳密なスコープ分離を持ちます

## ブラウザーで試す { #try-it-in-your-browser }

この完全なサンプルは、両方の DAG ブロックを定義してインスタンス化します。インクルードの引数や選択する出力を編集して、再実行してみてください。

[このサンプルをプレイグラウンドで開く](https://graphcal.org/playground/?example=functions)か、[ソースを読む](/docs/assets/playground/examples/step-4/main.gcl)ことができます。ファイル名は `main.gcl` のままにしてください。DAG 本体は `main` から宣言を自己インポートしています。

期待される初期出力には、選択された `v_parking`、`transfer_dv`、`departure_dv` の出力と `total` が含まれます。

## 次のステップ { #next-step }

[ステップ 5](step5-multi-file-projects.md) では、`import` 宣言を使ってプロジェクトを複数のファイルに分割します。

---
icon: material/chart-line
---

# Plot 宣言 { #plot-declarations }

Plot 宣言は、計算グラフから計算された値を可視化するチャートを定義します。
これらは [Vega-Lite](https://vega.github.io/vega-lite/) を使ってレンダリングされ、
インタラクティブな HTML または JSON 出力を生成します。ブラウザー評価 API はどちらも、
グループ化された図やレイヤー化された図を含め、図の仕様を通常のネストした
JavaScript オブジェクトとして返します。これは Vega-Lite や `vegaEmbed` に
そのまま渡すことができます。

## 構文 { #syntax }

```
plot <name> = {
    mark: <mark_type>,
    encode: {
        <channel>: <expr>,
        ...
    },
    <property>: <expr>,
    ...
};
```

`plot` 宣言は次の要素を持ちます。

- **名前**（`param` や `node` と同様に、慣例として `lower_snake_case`）。
- 視覚的なマークの種類を指定する **`mark` フィールド**（必須）。
- データを視覚チャネルにマッピングする **`encode` ブロック**（必須。少なくとも
  1 つのチャネルが必要）。
- `title` などの省略可能な**プロパティ**。

各フィールド（`mark`、`encode`、各エンコーディングチャネル、各プロパティ）は
最大 1 回だけ出現できます。重複は構文解析エラーです。

plot レベルのプロパティは次のとおりです。

| プロパティ | 型 |
|----------|------|
| `title` | 文字列リテラル |
| `width` | 正の無次元数 |
| `height` | 正の無次元数 |
| `x_label` | 文字列リテラル |
| `y_label` | 文字列リテラル |

プロパティ名と値の型は `graphcal check` によって検証されます。未知の名前
（たとえば `title:` の綴り間違い）はエラー、型の誤った値（たとえば
`title: 42.0`）はエラー、生のレンダリング量に対する次元付きの値
（たとえば `width: 2.0 m`）は拒否されます。単位が暗黙に取り除かれることは
決してありません。`width`/`height` は厳密に正でなければなりません（評価時に
検査されます）。

### マークの種類 { #mark-types }

| 種類 | 説明 |
|------|-------------|
| `point` | 散布図／点マーク |
| `line` | 傾向や時系列のための折れ線グラフ |
| `bar` | カテゴリ比較のための棒グラフ |
| `area` | 面グラフ（塗りつぶし領域） |
| `rect` | 矩形マーク（ヒートマップ、2D ビン） |
| `tick` | 分布のためのティックマーク |

マークの種類には省略可能なプロパティを指定できます。

```gcl
plot styled = {
    mark: line { stroke_width: 2.0 },
    encode: { ... },
};
```

| マークプロパティ | 型 |
|---------------|------|
| `stroke_width` | 無次元数 |
| `opacity` | 無次元数 |
| `size` | 無次元数 |
| `color` | 文字列リテラル |
| `filled` | 真偽値 |
| `interpolate` | 文字列リテラル |

### エンコーディングチャネル { #encoding-channels }

`encode` ブロックはデータを視覚チャネルにマッピングします。

| チャネル | 説明 |
|---------|-------------|
| `x` | X 軸の位置 |
| `y` | Y 軸の位置 |
| `color` | 色のエンコーディング（ヒートマップにも使用） |
| `size` | サイズのエンコーディング |
| `shape` | 形状のエンコーディング |
| `opacity` | 不透明度のエンコーディング |
| `detail` | 詳細チャネル（グループ化用） |
| `text` | テキストチャネル |
| `tooltip` | ツールチップチャネル |

チャネルの値は通常、インデックス付きのデータを生成する `for` 内包表記です。
`Fin(2)` のような構造的軸は、plot 専用の内包表記で直接使用できます。
別途のインデックス宣言やノードの注釈は不要です。

例:

```gcl
encode: {
    x: for m: Maneuver { @delta_v[m] },
    y: for m: Maneuver { @mass[m] },
},
```

### 表示上の失敗 { #presentation-failures }

表示のみに関する失敗が、有効な数値チャネルを破棄することはありません。評価器は別個の表示診断を報告し、**チャネル全体**に SI を使用します。そのため、変換に成功したエントリと未変換の SI エントリが混在することはありません。チャネル内で互換性のない表示単位がある場合も、可視の診断と SI へのフォールバックが生じます。形状、ドメイン、計算上の失敗は、引き続き該当する plot のレンダリングを妨げます。

### チャネルの整列 { #channel-alignment }

`graphcal check` は、評価の前にすべてのエンコーディング式を推論します。
無効な演算子や呼び出し、代数的な値や `Complex` 値などのプロットできない葉、
効果のないネストした表示変換、互換性のないチャネル軸を拒否します。
実行時の整列も、不正な値に対する多層防御として同じ検査を保持します。

1 つの plot のすべてのチャネルは、単一の共有された行集合に平坦化されます。

- 行集合は、*最も広い*軸集合を持つチャネルのインデックス軸の直積です。
  `color: for p: P, t: T { ... }` のような 2 変数の内包表記は、
  `P × T` のセルごとに 1 行を駆動します。
- 他のすべてのチャネルは、それらの軸の部分集合にわたる必要があります。その値は、
  言及していない軸にわたってブロードキャストされます。インデックスを持たない
  チャネル（単なるインデックスなしの値）は、すべての行で繰り返されます。
- 無関係なインデックスにわたるチャネル（たとえば `x: for s: Step { ... }` と
  `y: for p: Pair { ... }`）には意味のある行の対応がなく、エラーとして
  拒否されます。行が暗黙にパディングされたり、ずれて整列されたりすることは決してありません。
- plot で表現できない値（代数的な値、または 1 つのチャネル内での数値とラベルの
  混在）はエラーです。インデックスのバリアント名がデータの代わりに
  使われることは決してありません。

真偽値はラベル `"true"`/`"false"` として、インデックスキーはそのレンダリング
形式（ラベル、位置、または座標）として、日時は時間軸を持つ ISO 8601
タイムスタンプとして、数値は量的データとしてエンコードされます。座標軸を
量的にプロットするには、座標を明示的に取り出してください
（`for t: Step { coord(t) }`）。

### 単位を考慮した軸タイトル { #unit-aware-axis-titles }

Graphcal は、チャネルの検査済みの推論された次元と表示の由来から、軸タイトルを
自動生成します。結果は、量が構文上どこに現れるかに依存しません。
`@distance * 2.0` と `2.0 * @distance` のような等価な式は同じタイトルを
受け取ります。次元付きの軸タイトルは "Dimension (unit)" の形式で
整形されます。

- 表示単位 `km/s` を持つ `@velocity` は軸タイトル **"Velocity (km/s)"** を生成します
- 表示単位 `W` を持つ `@power` は軸タイトル **"Power (W)"** を生成します
- 無次元の値は自動タイトルを生成しません

エンコーディング内の表示単位変換は、レンダリングされる数値データも変換します。
Graphcal は、Vega-Lite の行をシリアライズする前に、各正準値を対象単位の
スケールで割ります。これはスカラーチャネルとインデックス付きチャネルに一貫して
適用され、計算値は内部的には正準のままです。動的な表示単位を解決できない
場合、変換済みのラベルの下に未変換のデータを出力するのではなく、レンダリングが
失敗します。

自動生成されたタイトルは、明示的な `x_label` または `y_label` プロパティで
上書きできます。ラベルの上書きはタイトルのみを変更し、チャネルの数値変換は
変更しません。

```gcl
plot custom_labels = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] -> km/s },
        y: for m: Maneuver { @mass[m] -> kg },
    },
    x_label: "Mission Delta-V",
    y_label: "Spacecraft Dry Mass",
};
```

## 例 { #examples }

### 折れ線グラフ { #line-chart }

```gcl
index Sample = { T0, T1, T2, T3, T4 };

node elapsed: Time[Sample] = { ... };
node altitude: Length[Sample] = { ... };

plot altitude_over_time = {
    mark: line,
    encode: {
        x: for sample: Sample { @elapsed[sample] -> s },
        y: for sample: Sample { @altitude[sample] -> km },
    },
    title: "Altitude Over Time",
};
```

### 棒グラフ { #bar-chart }

```gcl
index Mode = { Normal, Eco, Boost };

node mode_code: Dimensionless[Mode] = { ... };
node power: Power[Mode] = { ... };

plot power_by_mode = {
    mark: bar,
    encode: {
        x: for mode: Mode { @mode_code[mode] },
        y: for mode: Mode { @power[mode] -> W },
    },
    title: "Power by Operating Mode",
};
```

### 散布図 { #scatter-plot }

```gcl
index Maneuver = { Departure, Correction, Insertion };

node delta_v: Velocity[Maneuver] = { ... };
node mass: Mass[Maneuver] = { ... };

plot mass_vs_dv = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] -> km/s },
        y: for m: Maneuver { @mass[m] -> kg },
    },
    title: "Mass vs Delta-V",
};
```

### ヒートマップ { #heat-map }

```gcl
plot efficiency_map = {
    mark: rect,
    encode: {
        x: for p: Pressure { coord(p) },
        y: for t: Temperature { coord(t) },
        color: for p: Pressure, t: Temperature { @efficiency[p, t] },
    },
    title: "Efficiency Map",
};
```

## 表示と可視性 { #display-and-visibility }

Plot は**デフォルトで単独表示されます**。plot を書けば、それが表示されます。

```gcl
plot curve_a = {
    mark: line,
    encode: {
        x: for t: Time { coord(t) },
        y: for t: Time { @altitude[t] -> km },
    },
    title: "Altitude",
};
```

plot を合成専用の構成要素（`figure` や `layer` 宣言から参照されるが、単独では
レンダリングされない）として保持するには、`#[hidden]` でマークします。

```gcl
#[hidden]
plot curve_a = { mark: line, encode: { ... } };
```

`#[hidden]` の plot は引き続き計算グラフに参加し、`figure` および `layer` 宣言から
参照できます。単に単独のチャートとして表示されないだけです。`#[hidden]` は
単一指定のメタデータであり、1 つの対象に最大 1 回だけ出現できます。これは
`plot` 宣言に対してのみ有効です（figure と layer は何からも参照できないため、
これらを非表示にすることは削除と等価になります）。

表示とファイル間の可視性は独立した軸です。`pub` は plot を利用側ファイルから
include 可能にし（他の宣言に対する `pub` と同様）、表示については何も
規定しません。`#[hidden]` は、宣言しているファイルがエントリポイントである
場合の、そのファイル自身の出力のみを制御します。

## ファイル間の Plot { #cross-file-plots }

ライブラリの plot が利用側の出力に暗黙に表示されることは決してありません。
ライブラリの利用者はそのコードを編集できないため、何を表示するかは
（ライブラリの作者ではなく）利用者が制御します。include の波括弧リストで
`pub plot` を指名することが、表示の要求になります。

```gcl
include pkg.engine(fuel: 500.0 kg)::{ delta_v, thrust_curve, mass_breakdown as mb };
```

- ライブラリの plot を include 可能にするには `pub` でなければなりません。`pub` は
  単一の意味（ファイル境界を越えてエクスポートされる）を保ちます。他の宣言と
  まったく同じです。
- include された plot は、**そのインスタンス**、つまり include のパラメーター束縛に
  対して評価されます。同じライブラリの 2 つのインスタンス化が、同じ plot を
  異なるエイリアスで include でき、両方がレンダリングされます。
- include された plot は、ローカルのエイリアスでルートの名前空間に入り、ルートの
  `figure`/`layer` 宣言から参照できます。ルートの宣言とのエイリアスの衝突は、
  通常の名前重複エラーです。
- plot を**合成専用**で include するには（たとえば単独出力なしで利用側の figure に
  レイヤーとして重ねる場合）、include 項目に `#[hidden]` を
  付けます。

```gcl
include pkg.engine(fuel: 500.0 kg)::{
    thrust_curve,                    // displayed
    #[hidden] mass_breakdown as mb,  // included for composition only
};

figure summary = { plots: [thrust_curve, mb] };
```

- plot 以外の include 項目に対する `#[hidden]` はエラーです。それ自体が
  `#[hidden]` と宣言されているライブラリの plot を明示的に include すると、
  その plot は表示**されます**。include は利用者の明示的な要求であり、
  ライブラリの作者は利用者の出力を制御しないためです。
- `import` で plot を要求するとエラーになります。plot はインスタンスに対して
  評価される実行時のシンクであり、`import` はコンパイル時の名前のみを
  運ぶためです。
- どの波括弧リストにも指名されて*いない*ライブラリの plot（モジュール形式の
  `include ... as alias` の背後にあるすべてを含む）は、単に利用側の出力に
  含まれません。

## Figure 宣言 { #figure-declarations }

`figure` 宣言は、複数の plot を、横に並んだサブプロット（水平連結）を持つ
1 つの結合チャートにまとめます。

```
figure <name> = {
    plots: [<plot_name>, ...],
    <field>: <expr>,
    ...
};
```

`figure` 宣言は次の要素を持ちます。

- **名前**（慣例として `lower_snake_case`）。
- 結合する plot の名前を列挙する **`plots` フィールド**: `plots: [a, b]`。
- `title`（文字列リテラル）などの省略可能な**フィールド**。

| フィールド | 型 | 説明 |
|-------|------|-------------|
| `plots` | plot 名のリスト | サブプロットとして含める plot（必須） |
| `title` | 文字列リテラル | figure のタイトル |

`title` は figure がサポートする唯一のプロパティです。figure は横並びの連結として
レンダリングされ、全体の幅／高さを持たないためです。サイズは構成要素の plot に
設定してください（または `layer` を使用してください）。figure に対する
`width:`/`height:` は検査時エラーです。

### 例 { #example }

```gcl
plot curve_a = {
    mark: line,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
    title: "Values Squared",
};

plot curve_b = {
    mark: bar,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] + 1.0 },
    },
    title: "Values Plus One",
};

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Side-by-side Comparison",
};
```

これにより、出力には**3 つ**の図が生成されます。`curve_a`（単独）、
`curve_b`（単独）、`comparison`（結合されたサブプロットチャート）です。

### 単独 Plot の非表示 { #hiding-standalone-plots }

結合された figure のみを出力するには、個々の plot を `#[hidden]` でマークします。

```gcl
#[hidden]
plot curve_a = { mark: line, encode: { ... } };

#[hidden]
plot curve_b = { mark: bar, encode: { ... } };

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Combined View",
};
```

これにより**1 つ**の図 `comparison` が生成されます。`#[hidden]` の plot は引き続き
評価されて結合 figure に含まれますが、単独のチャートとしては
表示されません。

## Layer 宣言 { #layer-declarations }

`layer` 宣言は、共有された軸上に複数の plot を重ね合わせ、レイヤー化された
マークを持つ 1 つのチャートを生成します。これは、同じデータに対して異なる
マークの種類（たとえば線 + 点）を組み合わせるのに役立ちます。

```
layer <name> = {
    plots: [<plot_name>, ...],
    <field>: <expr>,
    ...
};
```

| フィールド | 型 | 説明 |
|-------|------|-------------|
| `plots` | plot 名のリスト | 重ね合わせる plot（必須） |
| `title` | 文字列リテラル | layer のタイトル |
| `width` | 正の無次元数 | チャートの幅（ピクセル） |
| `height` | 正の無次元数 | チャートの高さ（ピクセル） |

### Layer の例 { #layer-example }

```gcl
plot line_trace = {
    mark: line,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
};

plot point_trace = {
    mark: point,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
};

layer line_with_points = {
    plots: [line_trace, point_trace],
    title: "Line with Points",
};
```

これにより、線マークと点マークが同じ軸上に重ね合わされます。

## 主な特性 { #key-properties }

- plot、figure、layer の名前は、`param` や `node` と同様に、慣例として
  `lower_snake_case` を使用します。
- plot の本体は、任意の `@param` や `@node`、および定数を参照できます。
- plot、figure、layer は**葉ノード**です。どの宣言もこれらを `@` で参照することは
  できません。
- plot は依存関係グラフに参加します（参照するノードに依存します）が、実行時の
  値を生成しません。
- figure と layer は plot を名前で参照し、解決時に検証されます。未知の名前、
  他の figure/layer への参照（ネストはできません）、`plots:` 内の重複エントリは
  検査時エラーであり、`plots:` リストは空であってはなりません。

## CLI 出力 { #cli-output }

plot の出力は、`graphcal eval` の `--plot` オプションで制御します。

```bash
# Open interactive chart in default browser
graphcal eval file.gcl --plot browser

# Print only the plot JSON array to stdout
graphcal eval file.gcl --plot json

# Write a self-contained HTML page (headless/CI-friendly)
graphcal eval file.gcl --plot report.html
```

`--plot json` モードでは、標準出力は図オブジェクトのちょうど 1 つの JSON 配列であり、
各オブジェクトは `name` と `spec` を持ちます。通常の評価出力は抑制されるため、
結果を JSON ツールに直接パイプできます。

```json
[
  { "name": "curve_a", "spec": { /* Vega-Lite JSON */ } },
  { "name": "comparison", "spec": { /* Vega-Lite hconcat spec */ } }
]
```

詳細は [CLI リファレンス](../cli-reference.md#plot-output)を参照してください。

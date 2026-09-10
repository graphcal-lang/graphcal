---
icon: material/function
---

# DAG ブロック（再利用可能な計算） { #dag-blocks-reusable-computation }

Graphcal は、再利用可能でパラメーター化された計算を定義する唯一の仕組みとして `dag` ブロックを使います。`dag` は名前付きのサブ DAG であり、好きなだけ何度でもインスタンス化でき、各インスタンスは独自のパラメーター束縛を持ちます。

## 宣言構文 { #declaration-syntax }

`dag` ブロックは、独自のパラメーターとノードを持つ名前付きの再利用可能なサブ DAG を定義します。

```
dim GravParam = Length^3 / Time^2;

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    pub node v: Velocity = sqrt(@gm / @r);
}
```

本体は、ファイルのトップレベル宣言と同じ `param` / `node` / `const node` / `@` シジル構文を使います。

### 再利用可能な数式のための型レベル入力 { #type-level-inputs-for-reusable-math }

DAG は、必須の次元、型、インデックスを明示的な**型レベル入力ポート**として公開できます。ブロック内で `pub(bind)` を使って宣言し、`include` 箇所で各ポートを名前で束縛します。

```gcl
dag magnitude {
    pub(bind) dim ElementDim;
    pub(bind) index Axis;

    param values: ElementDim[Axis];
    pub node squared: ElementDim^2 = dot(@values, @values);
    pub node result: ElementDim = sqrt(@squared);
}

pub index SpatialAxis = { X, Y, Z };
param displacement: Length[SpatialAxis] = {
    SpatialAxis#X: 3.0 m,
    SpatialAxis#Y: 4.0 m,
    SpatialAxis#Z: 12.0 m,
};

include magnitude(
    dim ElementDim: Length,
    index Axis: SpatialAxis,
    values: @displacement,
) as displacement_magnitude;

assert magnitude_is_13m = @displacement_magnitude::result == 13.0 m;
```

これは、推論される DAG ジェネリクスに対する Graphcal の明示的な代替手段です。同じ DAG を別の次元と別の名前付き軸で再び include でき、各インスタンス化はその型レベル束縛を代入した後に検査されます。必須の `type T;` と座標インデックスの形式も同じように機能します。射影可能な `param` が使う束縛済みインデックスは、それ自体が include 境界を越えて可視でなければなりません。上の例で `pub index SpatialAxis` としているのはそのためです。

基数（cardinality）は束縛された `Index` によって運ばれるため、サイズ多相な配列アルゴリズムの多くは別途の自然数パラメーターを必要としません。一般的な `Nat` ジェネリックな DAG 宣言とサイズの算術は、このモデルの対象外のままです。

型レベル束縛は `include` に属します。式形式の `@dag(args)::out` 呼び出しは値パラメーターのみを供給します。どちらの形式でも、束縛は名前から式への一意な対応付けです。順序はどの入力が束縛されるかに影響せず、同じリスト内に同じターゲット名を 2 回以上書くことはできません。

### 複数出力の DAG { #multi-output-dags }

1 つの `dag` は複数の出力を公開できます。

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

## `include` による DAG ブロックの使用 { #using-dag-blocks-with-include }

DAG ブロックは `include` 宣言を使ってインスタンス化され、サブ DAG が現在の計算グラフに埋め込まれます。引数リストは必須です（空でもかまいません）。出力は `::{ ... }` の波括弧リストで射影します。

```
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
const node r_earth: Length = 6371.0 km;
param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
)::{ total_dv as transfer_dv, dv1 as departure_dv };
```

- パラメーターは名前付き引数として渡します。
- 出力ノードは `::{ ... }` の波括弧リストの中で選択し、必要に応じて `as` でエイリアスを付けます。
- 選択された出力は外側の DAG の通常のノードになり、`@transfer_dv`、`@departure_dv` などとして参照できます。
- `include` は `;` で終わります。

### include 全体へのエイリアス { #aliasing-the-whole-include }

いくつかの出力だけが必要な場合は波括弧リストが便利です。DAG のすべての出力を 1 つのプレフィックスの下に取り込むには、代わりにインスタンス化全体にエイリアスを付けます。

```
include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt) as parking;
node speed: Velocity = @parking::v;
```

1 つの `include` において、エイリアスと波括弧リストは排他的です。

## ファイルをまたぐ DAG ブロック { #cross-file-dag-blocks }

別のモジュールで定義された DAG ブロックは、ドット区切りの完全なパッケージパスで参照します。

```
include lib.orbital.hohmann_transfer(gm: @gm_earth, r1: @r1, r2: @r2)
    ::{ total_dv };
```

`(` の前のパスはパッケージルートからの絶対パスです。完全なパス解決の規則については[マルチファイルプロジェクト](multi-file.md)を参照してください。

選択された、あるいはエイリアス経由でアクセスされるすべての出力は、同一ファイル内の include であっても public でなければなりません。モジュール境界を越える場合は、DAG 宣言自体も `pub` でなければなりません。DAG 内のプライベートなノードは DAG 本体自身からのみ利用できます。

## DAG 呼び出し（式形式） { #dag-calls-expression-form }

式の中では、`@dag(args)::out` は匿名の実行時 `include` に対する糖衣構文であり、各呼び出し箇所が新しいインスタンス化になります。引数は周囲の式のスコープで評価されるため、ループ変数を参照できます。

```
index Region = { A, B };

dag id_len {
    param v: Length;
    pub node result: Length = @v;
}

node dist: Length[Region] = { Region#A: 1.0 m, Region#B: 2.0 m };

node distances: Length[Region] = for r: Region {
    @id_len(v: @dist[r])::result
};
```

`@` の直後に来るものは、ローカルの DAG、インポートされたモジュールエイリアス、またはそのいずれかを通して到達する子です。ファイルルートとソース内に入れ子になった `dag` モジュールは、同じコンパイル済み表現と呼び出し形式を使います。`@module(args)::out` はインポートされたターゲット自体を呼び出し、`@module.dag(args)::out` はその子を呼び出します。射影は引き続き必須です。射影されていない DAG インスタンスはグラフの値ではありません。射影は明示的にエクスポートされたノードまたはパラメーター入力ポートを指定できます。パラメーターの射影はその実効的な束縛値／デフォルト値を返します。

呼び出しは実行時 DAG をインスタンス化するため、`const node` の本体や定義域の境界では利用できません。呼び出しを実行時の宣言に置き、実行時の値が許される場所でその宣言を参照してください。

## import と include の違い { #import-vs-include }

`import` キーワードと `include` キーワードは異なる目的を持ちます。

- **`import`** はコンパイル時の名前、すなわち `dim`、`unit`、`type`、`index`、`const node`、`dag` をスコープに取り込みます。実行時の項目（`param`、`const` でない `node`）をインポートするとエラー（M020）になります。アサーションは具体的なインスタンスの結果であり、同様にインポートできません（M024）。
- **`include`** は、必要に応じてパラメーター束縛を伴って DAG をインスタンス化し、選択された値を外側のグラフに公開します。選択可能な項目には、明示的なノードのエクスポート、パラメーターの実効値、インスタンスローカルなアサーションが含まれます。

```
// constants.gcl
pub const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;
pub const node r_earth: Length = 6371.0 km;

// main.gcl
import myproject.constants::{ gm_earth, r_earth };

dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    pub node v: Velocity = sqrt(@gm / @r);
}

include orbital_velocity(gm: @gm_earth, r: @r_earth + @parking_alt)
    ::{ v as v_parking };
```

可視性（`pub`、`pub(bind)`）、必須のパラメーターとインデックス、パラメーター化された include の詳細については[マルチファイルプロジェクト](multi-file.md)を参照してください。依存先のアサーションを実行するには、具体的な `include` を作成し、その波括弧リストでアサーションを選択します。[アサーションとモジュール境界](multi-file.md#assertions-and-module-boundaries)を参照してください。

## なぜ DAG ブロックなのか { #why-dag-blocks }

`dag` ブロックは、他の言語であれば*純粋関数*（単一出力、式レベル）と*パラメーター化されたライブラリのインポート*（複数出力、ファイルレベル）に分かれるものを 1 つでカバーする単一の仕組みです。

- 複数の出力（単一の戻り値に限定されません）。
- トップレベル宣言と同じ `param` / `node` の意味論。
- DAG 本体内で値を参照するための同じ `@` シジル。
- `include` によってファイルレベルの DAG と合成可能。
- 厳密なスコープの分離。DAG が使うすべての名前は、その内部で宣言されるか、その内部でインポートされるか、`param` として供給されなければなりません。外側のファイルのトップレベルスコープからの継承はありません。

この厳密な分離の規則により、`dag` の内部でトップレベルの定数を使うには、include 箇所で `param` として渡すか、`dag` 本体の中で明示的に `import` するかのどちらかになります。

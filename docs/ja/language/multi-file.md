---
icon: material/file-multiple
---

# マルチファイルプロジェクト { #multi-file-projects }

Graphcal はコードを **パッケージ** に編成します。すべての `.gcl` ファイルは、正確に
1 つのパッケージに属します。`import` または `include` は、パッケージルートから始まる
絶対パスによって外部のモジュールを識別します。インポート後、式では、その宣言が導入した
ローカル名または `as` エイリアスを使用します。絶対パスの構文は、パッケージがマニフェストの
下にディスク上に存在する場合でも、作成したばかりの単一ファイルである場合でも同じです。
これらのケース間で変わるのは、外部から見えるパッケージ名だけです。

2 つの宣言が、外部の素材を DAG に取り込みます。

- **`import`** は *名前*(コンパイル時参照: `type`、`dim`、
  `unit`、`const node`、コンストラクター、`dag`、`index`)をローカル
  スコープに取り込みます。インポートは何もインスタンス化しません。
- **`include`** はパラメーター束縛を伴って DAG を *インスタンス化* し、
  サブグラフとして埋め込み、その出力をノードとして公開します。

どちらも同じパス規則を使用します。ドット区切りのセグメントで、パッケージルートからの
絶対パスです。相対パス、`..`、引用符付きのファイル文字列、および Graphcal ソース内の
`/` は存在しません。

## ファイルはパッケージである { #files-are-packages }

パッケージには名前とモジュールのツリーがあります。名前は、次の 2 つのいずれかから
決まります。

| 種類       | 名前の由来                          | 条件                                                                  |
|--------------|--------------------------------------|-----------------------------------------------------------------------|
| **仮想 (Virtual)**  | ファイルのステム(拡張子を除いた `.gcl` ファイル名) | `graphcal.toml` が存在しない、*または* ファイルがマニフェストのパッケージ名前空間の外にある |
| **実 (Real)**     | `graphcal.toml` の `package.name`    | ファイルが `<source_dir>/<package_name>.gcl` に、または `<source_dir>/<package_name>/` の下に存在する |

**仮想パッケージは単一のファイル** であり、スタンドアロンの Graphcal スクリプトです。
パッケージは正確に 1 つのモジュール、すなわちそのファイル自身で構成されます。仮想
パッケージで解決される唯一のインポートパスは、ファイル自身のステムです(後述の
[自己参照](#self-reference-a-file-is-its-own-package) を参照)。
仮想パッケージのファイルから兄弟ファイルをインポートすることはできません。
ローダーは `import helper::{X};` を拒否し、[実パッケージへの昇格](#promoting-to-a-real-package)
を指し示す構造化されたエラーを返します。

これは、ファイルの隣に `graphcal.toml` が置かれている場合にも当てはまります。
マニフェストは、パッケージ名前空間の内側
(`<source_dir>/<package_name>.gcl` または
`<source_dir>/<package_name>/` の下)に存在するファイルだけを「所有」します。
マニフェストと同じ階層にある単独の `.gcl` ファイルは、依然としてファイル横断インポートの
権限を持たない仮想パッケージのスクリプトです。規則は正確に 1 つだけです。
ファイルをまたいでインポートするには、そのファイルがパッケージの名前空間ディレクトリ内に
存在しなければなりません。

**実パッケージ** は、`source_dir` の下のディレクトリツリーに配置された多数のファイルに
またがることができます。解決は、パスに書かれたとおりに `<source_dir>/<segments>.gcl`
をたどります。

`nasa.rocket.dynamics` のようなパスは、パッケージルートから始めてツリーをたどります。
パッケージ `nasa` → ディレクトリ `rocket` → ファイル `dynamics.gcl`
(または `rocket.gcl` の内部で宣言されたインライン `dag dynamics { ... }`)です。
`::{...}` または `as` 句の前にあるパスは **常にモジュールを指し**、シンボルを指すことは
ありません。パーサーは構文だけからモジュールとシンボルの境界を判別します。

### ファイル名とディレクトリ名 { #file-and-directory-names }

すべての `.gcl` ファイル名のステムと、ソースルート以下のすべてのディレクトリは、
有効な Graphcal 識別子(snake_case、ハイフンなし、空白なし、キーワードでない)で
なければなりません。コンパイラーは `match.gcl`、`my-helpers.gcl`、`MyModule.gcl`
のようなファイルを即座に拒否します。ファイル名は `import` / `include` 宣言における
パスセグメント *そのもの* であるため、そこで構文的に使用可能でなければなりません。

## `import` 形式 { #the-import-form }

`import` は現在の DAG のスコープに名前を取り込みます。3 つの表層形式があり、
各形式は **書き下した名前だけを正確に** 導入します。暗黙の追加はありません。

```graphcal
import nasa.rocket;                                      // bare: brings `rocket`
import nasa.rocket as nr;                                // alias: brings `nr`
import nasa.rocket::{type Orbit, dim Length, compute_thrust as ct};
// brace: brings `Orbit`, `Length`, and `ct` only
```

各形式は、スコープに入るものが異なります。

| 形式                                                   | 追加される名前                                |
|--------------------------------------------------------|--------------------------------------------|
| `import nasa.rocket;`                                  | `rocket`(モジュール、その末尾名で)    |
| `import nasa.rocket as nr;`                            | `nr`(エイリアス付きのモジュール)              |
| `import nasa.rocket::{type Orbit};`                     | `Orbit` のみ。`rocket` は **含まれない**            |
| `import nasa.rocket::{type Orbit, dim Length, compute_thrust as ct};` | `Orbit`、`Length`、`ct` のみ |
| `import nasa.rocket as nr::{type Orbit};`               | パースエラー。エイリアスとブレースは相互排他 |

モジュール全体の名前は、インポートされた DAG モジュールそのものを表し、その内容の単なる
プレフィックスではありません。ファイルルートとインライン DAG は 1 つの抽象であるため、
`@rocket(args)::out` または `@nr(args)::out` はそのターゲットを直接呼び出し、
`@rocket.child(args)::out` はエクスポートされた子 DAG に降りていきます。インポートされた
モジュールエイリアスとローカル DAG は、この呼び出し可能な名前空間を共有します。両者が
同じ綴りを異なるターゲットに束縛している場合、呼び出しは曖昧となり、名前を変更しなければ
なりません。

モジュール修飾子と非修飾の項目の *両方* が必要な場合は、2 つの文を書きます。

```graphcal
import nasa.rocket;            // brings: rocket
import nasa.rocket::{type Orbit}; // brings: Orbit
// Now both `rocket::Orbit` and `Orbit` are usable.
```

これは Gleam からの意図的な相違です。Graphcal のブレース形式は、モジュールの末尾名を
**併せて取り込みません**。各 `import` はスコープに入るものを正確に指名すべきであり、
インポート一覧を眺める読者が、導入される名前の正確な集合を把握できるようにします。

### 選択的インポートはカテゴリーを宣言する { #selective-imports-declare-their-category }

項 (Term) 以外のすべての名前空間には、明示的なマーカーがあります。マーカーのない項目は
項の名前空間のみを選択します(定数、DAG、コンストラクター、およびランタイム宣言。
ランタイム宣言はその後、純粋インポートポリシーによって拒否されます)。

| 項目の形式 | 選択対象 |
|-----------|---------|
| `name` | 項の宣言またはコンストラクター |
| `type Name` | 構造体型または共用体型 |
| `dim Name` | 次元 |
| `unit name` | 単位 |
| `index Name` | インデックス |

```graphcal
import nasa.rocket::{
    type Orbit,
    dim Length,
    unit nautical_mile,
    MAX_THRUST,
    index Maneuver,
};
```

マーカーによって、インポートは型付きのマニフェストになります。エクスポート側が
`nautical_mile` を単位から別種のエンティティに変更した場合、インポートは新しいカテゴリーに
黙って束縛される代わりに、その行で失敗します。M022 診断は、その綴りを持つエクスポート
されたすべてのカテゴリーを、`nautical_mile` や `unit nautical_mile` のようなコピー&
ペースト可能な形式で列挙します。

型名とコンストラクターは、意図的に異なる名前空間を占めます。同じ綴りの型と
コンストラクターの組の両側をインポートするには、両方の項目を書きます。

```graphcal
import school.records::{type Student, Student};
```

単位は単位位置でのみ使用されるため、同様に項と綴りを共有できます。各側を明示的に
インポートしてください。例: `import finance::{unit JPY, JPY};`。値とコンストラクターは、
どちらも項であり式の中で区別できないため、名前を共有することは **できません**。

インポートされた型システム宣言は、定義元モジュールからの意味的な依存関係を保持します。
例えば、`dim WeightedRate` をインポートすると、非公開または公開の兄弟次元を通じて
解決された展開が保持され、`unit point` をインポートすると、その正規の基底次元とスケールが
保持され、`type Request` をインポートすると、ネストされたレコードおよびインデックスの
フィールド型が保持されます。これは、次元や単位の定義がモジュール修飾された依存関係を
使用する場合にも当てはまります。これらの依存関係は、利用側に追加のソース名を導入しません。
兄弟をインポートするのは、利用側のソースがそれを直接指名する場合だけにしてください。
利用側のソースが値を明示的に構築する場合、コンストラクターは引き続き別個の項インポートです。

直接インポートまたはモジュール修飾インポートは、その保持された Static 依存関係の閉包に
未解決の必須入力がない場合にのみ有効です。必須の `pub(bind)`
`type`、`dim`、`index` はそれ自体が純粋インポート境界を越えることができず、シグネチャが
それらに推移的に依存する宣言も同様です。代わりに、所有するブループリントを `include` で
インスタンス化し、カテゴリーが正確に一致する束縛を供給してください。モジュール全体の
エイリアスは、後の修飾付き include のためにブループリントを指名することは引き続き可能です。
この制限は、ソースが閉じていないメンバーをインポートされたコンパイル時の値として選択する
場合に適用されます。

### 項目のエイリアス { #aliasing-items }

ブレースリスト内の各項目には、個別にエイリアスを付けられます。

```graphcal
import nasa.rocket::{type Orbit as O, compute_thrust as ct};
```

エイリアスは、直接のローカル宣言と同じ名前空間固有の予約規則 (N009) に従います。

- `type`、`dim`、`index` のエイリアスは、プレリュードの次元や組み込み型の綴りを
  再利用できません。
- `unit` のエイリアスは、プレリュードの単位の綴りを再利用できません。
- 項のエイリアスは、`E`、`PI`、`sum`、`sin` などの組み込み定数や関数を含め、
  可視なあらゆる項の綴りを再利用できません。

これらは名前空間ごとのポリシーであり、単一のグローバルな予約語リストではありません。
項のエイリアスは、`UTC` のような Static の時間スケールの綴りを引き続き使用できます。
これらの名前は異なる名前空間を占めるためです。選択的 `include` の項目と `pub` 再エクスポートにも
同じ規則が適用されます。

### `import` が取り込めるもの { #what-import-may-bring }

`import` 境界を越えるのは、コンパイル時の名前だけです。

| 宣言の種類 | 選択的項目 | インポート後の参照 |
|------------------|----------------|------------------------|
| `const node` | `name` | `@name` |
| `dim` | `dim DimName` | `DimName` |
| `unit` | `unit unit_name` | `unit_name` |
| `type` | `type TypeName` | `TypeName` |
| `index` | `index IndexName` | `IndexName` |
| コンストラクター | `name` | 値の式で使用 |
| `dag` | `name` | `include` または `@name(args)::out` で使用 |

ランタイム値(`const` でない `node` とあらゆる `param`)は **インポートできません**。
別のファイルからランタイム値を利用するには、生成側の DAG を `include` で
インスタンス化してください([`include` 形式](#the-include-form) を参照)。
アサーションはインスタンスの結果であり、プロット、フィギュア、レイヤーはランタイムの
可視化要求であるため、いずれも純粋な `import` 境界を越えることはできません。
インスタンススコープのアサーションとプロットは、代わりに `include` のブレースリストを
通じて要求してください([ファイル横断プロット](plots.md#cross-file-plots) を参照)。

インポートされたモジュールエイリアスは、型、式、単位の各位置における第一級の修飾子です。
2 つのインポートが同じ末尾名をエクスポートする場合、モジュール修飾パスを書いて
所有者を明示的に選択します。

```graphcal
import collide.a as a;
import collide.b as b;

const node gain: Dimensionless = a::bias;
node phase_score: Dimensionless[a::Phase] = for current_phase: a::Phase {
    match current_phase {
        a::Phase#Burn => 1.0,
        a::Phase#Coast => 2.0,
    }
};
node result: a::Item = a::Pick(distance: 2.0 m);
node span: Length = 2.0 a::mile;
node span_miles: Length = @span -> a::mile;
```

コンパイラーはこれらのパスを正規のエクスポート項目に解決します。異なるモジュールからの
同じ末尾名の型、インデックス、コンストラクター、ラベルをマージすることはありません。

### 選択的インポートの再エクスポート { #selective-import-re-exports }

選択的インポートは、項目に `pub` を付けることで、束縛を 1 項目ずつ再エクスポートします。

```graphcal
import nasa.rocket::{ pub type Orbit, compute_thrust };
//                   ^^^ only `Orbit` is re-exported
```

マーカーなしおよびエイリアス付きのインポートは、デフォルトで非公開です。DAG 全体の
インポートの先頭に `pub` を付けると、メンバーを平坦化することなく公開 DAG エイリアスが
作成されます。

```graphcal
import nasa.rocket;             // private alias `rocket`
import nasa.rocket as r;        // private alias `r`
pub import nasa.rocket;         // public alias `rocket`
pub import nasa.rocket as nr;   // public alias `nr`
```

公開エイリアスは、ターゲットの正規の同一性を保持します。ターゲットとそのすべての
DAG 祖先は、公開で到達可能でなければなりません。パッケージルートの DAG は有効な
ターゲットです。選択的インポートの先頭に `pub` を付けることは無効です。選択された
各項目が独自の可視性を持つためです。

```graphcal
pub import nasa.rocket::{ type Orbit }; // parse error
import nasa.rocket::{ pub type Orbit }; // explicit selective re-export
pub(bind) import nasa.rocket;           // parse error
```

インポートは決して束縛可能ではありません。`pub(bind)` は束縛可能な宣言にのみ付けられます。

## `include` 形式 { #the-include-form }

`include` は DAG をインスタンス化し、その本体をサブグラフとして埋め込み、
その出力を外側の DAG のノードとして公開します。括弧内の引数リストは
**必須** であり(空でもかまいません)、これにより `include` は `import` と
構文的に区別されます。

```graphcal
// Bare: leaf becomes the alias; outputs accessed as @compute_thrust::<output>
include nasa.rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg);
node t: Force = @compute_thrust::thrust;

// Aliased: outputs accessed as @ct::<output>
include nasa.rocket.compute_thrust(orbit: @o) as ct;
node t: Force = @ct::thrust;

// Brace list: selects (and optionally renames) outputs as direct nodes
include nasa.rocket.compute_thrust(orbit: @o)::{ thrust };
include nasa.rocket.compute_thrust(orbit: @o)::{ thrust, isp, mass_flow as mdot };
node t: Force = @thrust;

// Non-Term projections use the same explicit markers as selective imports.
include nasa.rocket.compute_thrust(orbit: @o)::{
    type Orbit as EffectiveOrbit,
    dim Thrust as EffectiveThrust,
    unit newton as effective_newton,
    index Maneuver as EffectiveManeuver,
};
// An unmarked item still selects only a projectable Term.
```

| 形式                                                            | 結果                                                |
|-----------------------------------------------------------------|-------------------------------------------------------|
| `include path.dag(args);`                                       | `... as dag` の糖衣構文。末尾名がエイリアスになる       |
| `include path.dag(args) as a;`                                  | 出力は `@a::<output>` で参照                      |
| `include path.dag(args)::{x};`                                   | `x` 自体が現在の DAG のノードになる          |
| `include path.dag(args)::{x as y};`                              | 同上。名前を変更                                         |
| `include path.dag(args) as a::{x};`                              | パースエラー。エイリアスとブレースは相互排他      |

ブレースリストで選択された出力、または include エイリアスを通じて参照される出力は、
同じファイル内の DAG であっても公開でなければなりません。モジュール境界をまたぐ場合、
`path.dag` で指名される DAG も `pub` でなければなりません。出力の名前を変更しても、
その可視性は変わりません。include された DAG 内の非公開ノードは、実装の詳細のままです。

コンストラクターは、それを所有する型の API の射影可能な一部です。したがって、所有型と
そのコンストラクターを選択すると、Static 特殊化された ADT の構築とマッチングが可能に
なります。

```graphcal
dag sensor {
    pub(bind) dim Quantity;
    pub type Reading { Missing, Present(value: Quantity) }
}

include sensor(dim Quantity: Length)::{
    type Reading,
    Missing,
    Present,
};
node reading: Reading = Present(value: 2.0 m);
```

コンストラクターは `Reading` と同じ特殊化を維持します。コンストラクターの 1 つを
選択しながら所有型自体を再束縛することは拒否されます (`M032`)。
ネストされた `dag` は再利用可能なブループリントであり、構成済みインスタンスのメンバーでは
ないため、include のブレースリストからそれを選択することは、その項目で拒否されます
(`M031`)。代わりに、ネストされたブループリントはドット区切りの DAG パスで指定してください。
例: `include library.parent.child(args)::{output};`。

したがって `graphcal eval` は、デフォルトではエントリー DAG と、include の選択済み
または射影可能な出力のみを表示します。デバッグ時に各インスタンスの束縛済み params と
非公開のヘルパー値も出力するには、`--output-view all` を使用してください。このオプションは
表示のみを変更します。include する側の DAG から非公開の名前にアクセスできるようにする
ものではありません。

ブレースリストの項目は、include されたファイルの `pub plot` を指名することもできます。
これは、このインスタンスに対するライブラリのチャートを表示するという利用側の要求です。
プロットはローカルエイリアスの下でルート名前空間に入ります。項目に `#[hidden]` を
付けると、フィギュア/レイヤーの合成のためだけに含められます。
[ファイル横断プロット](plots.md#cross-file-plots) を参照してください。

### 構成済み include はネストできる { #configured-includes-may-nest }

include されたファイルやインライン DAG は、独自の構成済み include を持つことができます。
各階層は別個のインスタンススコープであり、束縛はそれを直接 include するスコープで
評価されます。

```graphcal
// leaf.gcl
param x: Dimensionless;
pub node doubled: Dimensionless = @x * 2.0;

// middle.gcl
param x: Dimensionless;
include demo.leaf(x: @x) as leaf;
pub node out: Dimensionless = @leaf::doubled;

// main.gcl
include demo.middle(x: 3.0) as middle;
node result: Dimensionless = @middle::out; // 6.0
```

プロジェクトの依存関係グラフが非循環である限り、ネストは任意の深さにできます。
複数の外側のインスタンスは、再帰的に分離された内側のインスタンスを作成します。
非公開の値は非公開のままであり、選択済み/公開の出力は外側のインスタンスパスを維持し、
アサーション診断は `middle.leaf.positive` のような完全なパスを使用します。
構成可能な入力を公開する中間 DAG は、上記のように独自の `param` を宣言し、
それを明示的に転送しなければなりません。

include されたファイルが内部で使用するインポートは、その構成済みインスタンスとともに
分離されます。2 つのライブラリがそれぞれ異なるモジュールからローカルの `C` をインポート
(または同じモジュールエイリアスを使用)しても、include された各本体は、include の順序や
ネストに関係なく、自身の正規の宣言を読み続けます。include の束縛式はインポート側に属し、
宣言シグネチャと生成側の本体は include されたファイルに属します。診断は、あるファイルの
スパンを別のファイルのテキストに適用するのではなく、失敗した式を所有するファイルを
報告します。
通常の `unit` は、この境界をまたいでもインスタンススコープのままです。そのスケールは
具体的な include の束縛から評価され、繰り返しまたはネストされた include は独立した単位の
同一性とスケールを保持します。通常の単位は引き続き `import` では利用できません。
ブループリントに対して安定した、インポート可能な単位には `const unit` を使用してください。

### `include` は DAG の `import` を必要としない { #include-does-not-require-import-of-the-dag }

include パスはパッケージルートからの絶対パスであるため、DAG 自体に対する事前の
`import` は不要です。導入される名前は、DAG の出力(ブレースリストで指名されるか、
エイリアスによる)だけです。ただし、param インターフェースの **型** は、引き続き
`import` でスコープに取り込まなければなりません。

```graphcal
dag mission {
    import nasa.rocket::{type Orbit};          // type for the param
    param o: Orbit;

    include nasa.rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg)::{ thrust };

    node total: Force = @thrust + 100.0 N;
}
```

### 選択的 include 出力の再エクスポート { #selective-include-output-re-exports }

1 つの選択的 include リスト内では、各生成元はその名前空間内で高々 1 回しか現れることが
できません (M027)。繰り返しの選択に異なるローカルエイリアスを与えても、それらが別個に
なるわけではありません。アサーション、プロット、`#[hidden]`、`#[expected_fail]`、
`#[assumes]` のメタデータは、1 つの生成元の出現に付加されます。1 回だけ選択し、
そのローカル名を一貫して使用してください。

include もまた非公開の使用箇所です。ブレースリスト内で各出力に `pub` を付けることで、
明示的に選択した出力だけを再エクスポートします。

```graphcal
include nasa.rocket.compute_thrust(orbit: @o)::{ pub thrust, mass_flow };
//                                                      ^^^^^^^^^ private here
```

下流の include は、先行する選択的 include が作成した公開エイリアスを選択できます。
Graphcal はそのエイリアスをインスタンス宣言として実体化するため、const とランタイムの
出力は、`as` エイリアスや名前空間形式のアクセスを含む多段のファサードを通じて、
正規のターゲットを保持します。アサーションとプロットは、通常の値になるのではなく、
専用のインスタンスカテゴリーを保持します。

マーカーなしおよびエイリアス付きの include は、非公開の構成済みインスタンスを作成します。
先頭に付ける `pub include` および `pub(bind) include` 形式はパースエラーです。Graphcal は
依存関係が制御する出力名前空間を丸ごと公開することはありません。

### DAG 呼び出し式 { #dag-call-expression }

式の内部では、`@dag(args)::out` は匿名のランタイム
`include ... as <synthetic>; @<synthetic>::out` の糖衣構文です。射影は、DAG 内の
外部から射影可能な値、すなわち明示的にエクスポートされたノードか param 入力ポートの
いずれかを指名しなければなりません。param を射影すると、その有効な呼び出し束縛、
または省略された場合はそのデフォルトが読み取られます。

```graphcal
dag mission {
    import nasa.rocket::{compute_thrust, type Orbit};
    param o: Orbit;
    node t: Force = @compute_thrust(orbit: @o, dry_mass: 800.0 kg)::thrust;
    node effective_mass: Mass =
        @compute_thrust(orbit: @o, dry_mass: 800.0 kg)::dry_mass;
}
```

各呼び出し箇所は新しいインスタンス化であり、DAG の `assert` 宣言は
`include` パスとまったく同様に、インスタンス化ごとに検査されます。
必須の Static 入力は、include と同じ明示的マーカーを使用します。
`type Element: LocalElement`、`dim Measure: Length`、および
`index Axis: LocalAxis`(または `Fin(N)`)です。必須の Static 入力を省略すると
`I010` になります。供給された束縛は、その呼び出し箇所でパラメーターと出力の
シグネチャを特殊化します。

呼び出しは、ターゲットがファイルルートであるかソースにネストされた `dag` であるかに
関係なくランタイムのグラフインスタンス化であるため、`const node` 本体とドメイン境界では
拒否されます。式には報告面がないため、失敗した(またはエラーになった)assert は
呼び出し側の式自体を失敗させます。呼び出し側のノードは ``assertion `v_positive`
failed in inline call of dag `checked` (assertion evaluated to
false)`` のような評価エラーを報告し、障害はそのノードに分離されます。`#[expected_fail]` の
反転は通常どおり適用されます。期待された失敗はエラーではなく、
予期しない成功はエラーです。

準備済み評価は、各呼び出し可能対象の依存関係スケジュールと検査済み定数プールを
保持します。そのプランを再利用しても、呼び出し状態はマージされません。供給された
パラメーター、計算された値、アサーション、動的単位はインスタンスローカルのままです。
ルート評価と呼び出し評価は、同じデフォルトおよびドメイン検査を適用します。通常の
ルートノードの失敗は独立した結果を利用可能なままにしますが、呼び出し内部の失敗は
それを含む式を失敗させます。独立したプラグイン呼び出しの順序は
[未規定](extern-functions.md#purity-and-invocation-order) です。

引数は外側の式スコープで評価されるため、外側の `for`、`scan`、
`unfold`、`match` 束縛のローカル変数を参照できます。

```graphcal
node distances: Length[Region] = for r: Region {
    @scale(factor: 2.0, v: @dist[r])::result
};
```

#### インポートされたモジュールの呼び出し: `@module(args)::out` { #imported-module-calls-moduleargsout }

すべてのファイルルートとすべてのインライン `dag` ブロックは、同じ種類の DAG モジュールです。
モジュール全体の `import` はその正確なターゲットを再利用可能なモジュールエイリアスとして
束縛するため、ターゲットがファイル内にあるか別の DAG の内部にあるかにかかわらず、
エイリアス自体が呼び出し可能です。

```graphcal
// Invoke the imported file-root DAG itself.
import nasa.rocket as rocket;
node file_output: Force = @rocket(orbit: @o)::thrust;

// Invoke an inline DAG imported by its full module path.
import nasa.rocket.compute_thrust as thrust;
node inline_output: Force = @thrust(orbit: @o, dry_mass: 800.0 kg)::thrust;
```

エイリアスなしのインポートは、パスの末尾名を束縛します(`import nasa.rocket;` は
`rocket` を束縛します)。選択的 DAG インポートは、同様に選択されたローカル名で
呼び出し可能です。

```graphcal
import nasa.rocket::{compute_thrust as thrust};
node t: Force = @thrust(orbit: @o, dry_mass: 800.0 kg)::thrust;
```

エイリアスは子 DAG を修飾することもできます。ファイルモジュールをインポートした後、
同じ `compute_thrust` DAG には次のようにして到達できます。

```graphcal
import nasa.rocket as rocket;
node t: Force = @rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg)::thrust;
```

射影されるメンバーは、明示的にエクスポートされたノードか param 入力ポートでなければ
なりません。モジュール境界をまたぐ場合、呼び出しパスがたどるすべてのインライン DAG も
明示的にエクスポートされていなければなりません。ファイルルートは、そのパッケージで
アドレス指定可能なモジュールです。

引き続き拒否される *のは*、射影を省略することです。末尾の `::<out>` を伴わない
`@dag(args)`、`@module(args)`、`@module.dag(args)` はパースエラーです。
射影のない DAG インスタンスはグラフ値ではなく、`@` が要求するのはグラフ値です。

## モジュールとしてのインライン DAG { #inline-dags-as-modules }

別のモジュール内の `dag` 宣言は、ファイルのトップレベルにあるか別の DAG の内部に
ネストされているかにかかわらず、それ自体がモジュールであり、パスを拡張することで
アドレス指定できます。

```graphcal
// orbit_analysis.gcl  (virtual package: orbit_analysis)
dag analyze {
    type IntermediateResult { IntermediateResult(value: Length) }

    dag deeper {
        import orbit_analysis.analyze::{type IntermediateResult};
        param r: IntermediateResult;
        // ...
    }
}
```

パス `orbit_analysis.analyze.deeper` は、パッケージ `orbit_analysis`、サブモジュール
`analyze`、サブモジュール `deeper` と読みます。パッケージ横断の `nasa.rocket.compute_thrust`
と同一のアドレス指定規則です。解決は、物理的な `.gcl` ファイルを指名する最長の
プレフィックスを選択し、その後、残りのすべてのインライン DAG セグメントを正確にたどります。
異なる親の下にある同名の子孫が、互いにエイリアスになることはありません。

インライン DAG 内の `import` および `include` 宣言は、実際のプロジェクト依存関係
エッジです。それらは依存関係を読み込み、ファイルルートのインポートと同じ可視性検査で
値とコンパイル時の名前をインポートし、DAG が `include dag(args)` として消費されるか
`@dag(args)::out` として消費されるかにかかわらず尊重されます。

兄弟のトップレベル DAG も、同じ方法でアドレス指定されます。

```graphcal
// orbit_analysis.gcl
dag double {
    param x: Length;
    node y: Length = @x * 2.0;
}

dag analyze {
    param input_dist: Length;
    include orbit_analysis.double(x: @input_dist)::{ y as doubled };
    node final: Length = @doubled + 1.0 m;
}
```

### 親 DAG の再帰的 include { #recursive-parent-dag-include }

インライン DAG は、それを囲む DAG の `include` を完全なパスで書くことができますが、
そのソースは無限の具体的インスタンスグラフを形成します。Graphcal は、モジュール
スコープを構築する前に、直接および相互の再帰的 include を拒否します。再帰的
インスタンス化は、ランタイムの制御フロー機構ではありません。

## 自己参照: ファイルはそれ自身のパッケージである { #self-reference-a-file-is-its-own-package }

インライン DAG の内部から *現在のファイル* のトップレベル宣言に到達するには、
ファイル自身のパッケージアドレスを使用します。相対的なショートカット、`super`、
`..` はありません。

仮想パッケージでは、ファイルのステムがパッケージ名です。

```graphcal
// dynamics.gcl  (virtual package: dynamics)
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }
const node earth_mu: GravParam = 3.986e5 km^3/s^2;

dag analyze {
    dag energy {
        import dynamics::{type OrbitType, earth_mu};   // file's own name
        param o: OrbitType;
        node e: SpecificEnergy = -@earth_mu / (2.0 * @o.sma);
    }
}
```

自己インポートは、分離されたインライン DAG 本体の内部でのみ有効です。ファイルルートは、
単一セグメントの仮想パッケージ名によっても、完全修飾パッケージパスによっても、自身の
ファイルをインポートできません (`M033`)。そのトップレベル宣言はすでにスコープ内に
あるため、そのようなインポートは冗長であり、エイリアスは曖昧な重複束縛のセマンティクスを
持つことになります。同名のインライン DAG は、引き続きファイルの仮想パッケージ名より
優先されます。

インライン DAG の内部では、自己インポートはファイル横断インポートとまったく同じ
純粋インポートポリシーを使用します。定数、コンストラクター、DAG、マーカー付きの
型システム名はコンパイル時項目です。params とノードはインスタンスを必要とします。
アサーションと可視化要求は、隠れた外側のファイルインスタンスが存在しないため
拒否されます。

実パッケージでは、同じ参照に完全なパッケージパスを使用します。

```graphcal
// On disk: src/nasa/rocket/dynamics.gcl
// Source address: nasa.rocket.dynamics
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    dag energy {
        import nasa.rocket.dynamics::{type OrbitType};
        param o: OrbitType;
        // ...
    }
}
```

注: `/` はディスク上のファイルシステムパス(ツーリングの関心事)に現れるものであり、
Graphcal ソースには決して現れません。

## 厳格な分離 { #strict-isolation }

インライン DAG 本体は、自身の宣言、自身の `import`、自身の `include` の出力
**だけ** を参照できます。外側のファイルのトップレベルスコープからのレキシカルな継承は
なく、外側の DAG 本体からの継承もありません。DAG が使用するすべての名前は、
その内部で宣言されるか、明示的にインポートされなければなりません。

```graphcal
// dynamics.gcl
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    // ERROR: `OrbitType` is not visible here without an import.
    param o: OrbitType;
}

dag analyze_ok {
    import dynamics::{type OrbitType};
    param o: OrbitType;
}
```

この規則は、トップレベルであれインラインであれ、すべての DAG にわたって一様です。
これは、ファイルベースの DAG とインライン DAG を交換可能にするのと同じ分離保証です。
同じ名前解決、同じスコープ規則、同じ依存関係の可視性です。

## 実パッケージへの昇格 { #promoting-to-a-real-package }

実パッケージは、`graphcal.toml` マニフェストによって宣言されます。プロジェクトルートに
作成してください。

```toml
[package]
name = "nasa"
# source_dir = "src"  # optional, defaults to "src"
```

ソースは `<source_dir>/<package_name>/` の下に配置します。

```text
my_project/
  graphcal.toml            # [package] name = "nasa"
  src/
    nasa/
      constants.gcl
      rocket.gcl
      orbital/
        transfer.gcl
```

これでファイルは、次のようにアドレス指定されます。

```graphcal
import nasa.constants::{g0};
import nasa.rocket::{type Orbit, compute_thrust};
import nasa.orbital.transfer::{dv};
```

### 自己参照の移行 { #migrating-self-references }

仮想パッケージが昇格されると、各ファイルの自己参照は、ファイルステムのみの形から
完全なパッケージパスに書き換えなければなりません。

```graphcal
// Before (virtual package `dynamics`):
import dynamics::{type OrbitType};

// After (real package `nasa`, file at src/nasa/rocket/dynamics.gcl):
import nasa.rocket.dynamics::{type OrbitType};
```

LSP の rename リファクタリングが、この書き換えの機械的な部分を処理します。

### カスタムソースディレクトリ { #custom-source-directory }

`source_dir` を上書きして、別の場所を指定します。

```toml
[package]
name = "myproject"
source_dir = "lib"
```

これで `import myproject.helpers` は
`<project_root>/lib/myproject/helpers.gcl` に解決されます。

`source_dir` はポータブルな相対ディレクトリであり、ホストネイティブのパスではありません。
1 つ以上の通常の `/` 区切りコンポーネントを含んでいなければなりません。空の綴り、
`.`/`..` コンポーネント、絶対パス、繰り返しまたは末尾の区切り文字、
バックスラッシュ、制御文字、プラットフォーム固有のパスプレフィックスは拒否されます。
`src` や `lib` のような名前付きディレクトリを使用してください。`source_dir = "."` は
サポートされていません。

## パッケージ依存関係 { #package-dependencies }

実パッケージは、Git から取得される他の Graphcal パッケージに依存できます。MVP は、
正確なコミットに固定された、ソースのみの Git 依存関係をサポートします。

```toml
[package]
name = "mission"
source_dir = "src"

[dependencies]
orbital = {
    git = "https://github.com/acme/orbital.git",
    rev = "0123456789abcdef0123456789abcdef01234567",
}
```

規則:

- `rev` は必須であり、完全な 40 文字のコミットハッシュでなければなりません。
- ブランチ、タグ、バージョン範囲、`latest`、レジストリ、アーカイブ、公開、
  および Git リポジトリ内の暗黙的なパッケージ探索は、この MVP の一部では
  ありません。
- `git` は、パース済みのリモート HTTPS URL、`ssh://` URL、および SSH の scp 風
  `user@host:path` URL のみを受け付けます。すべてのソースは、ホストと空でないリポジトリ
  パスを含んでいなければなりません。ローカルパス、`file://`、平文 HTTP、その他の URL
  スキーム、埋め込みの資格情報、クエリ文字列、フラグメント、ドット付き/曖昧なパス、
  およびオプションに見える authority またはパスコンポーネントは、取得前に拒否されます。
- 非公開リポジトリには、基盤となる Git 取得実装が現在の環境で取得できる資格情報が
  必要であるため、サポートは環境に依存します。例えば、SSH キー/エージェントが
  利用可能な場合は SSH が機能する一方で、互換性のある credential helper または
  非対話型の資格情報プロバイダーが構成されていない限り、HTTPS は失敗する可能性が
  あります。SSH URL 内の SSH ユーザー名は許可されます。パスワード、トークン、
  その他の秘密情報を URL に埋め込んではなりません。
- 依存関係テーブルのキーは、`import` および `include` パスで使用される、
  ソースから見える依存関係名です。
- `package` が省略された場合、取得されたパッケージの `[package].name` は
  依存関係キーと一致しなければなりません。
- `package` が存在する場合、依存関係キーはローカルエイリアスであり、`package` が
  取得されたパッケージの実際のパッケージ名を指名します。

!!! warning "ローカルまたはサポートされない Git ソースからの移行"
    ファイルシステムパス、`file://`、`http://`、または以前は受け付けられていた
    その他の綴りを使用していたマニフェストとロックファイルは、到達可能な HTTPS または
    SSH リモートに切り替えたうえで、`graphcal deps lock` で `graphcal.lock` を再生成
    しなければなりません。Graphcal には現在、ローカルパスの依存関係形式がありません。
    ローカル開発リポジトリは、サポートされるリモートトランスポートを通じて提供する
    必要があります。

```toml
[dependencies]
units_v1 = { package = "units", git = "https://github.com/acme/units.git", rev = "1111111111111111111111111111111111111111" }
units_v2 = { package = "units", git = "https://github.com/acme/units.git", rev = "2222222222222222222222222222222222222222" }
```

ソースは、これらのエイリアスを明示的に解決します。

```graphcal
import units_v1.si::{unit m as m_v1};
import units_v2.si::{unit m as m_v2};
```

依存関係を持つパッケージを検査または評価する前に、`graphcal deps lock` を実行してください。
このコマンドは `graphcal.lock` を書き出します。これは決定論的でツールが保守する
ロックファイルであり、パッケージインスタンス、正確な Git コミット、ソースツリーハッシュ、
直接依存関係エッジ、および解決に使用された Graphcal/標準ライブラリのバージョンを
記録します。ロックファイルはパッケージインスタンス間の依存関係グラフのエッジを記録します。
これは「パッケージ名ごとに 1 バージョン」という平坦なマップではないため、同じパッケージの
複数のリビジョンが共存できます。現在のロックバージョンでは、すべての固定テーブルは
閉じたスキーマです。未知のルート、パッケージ、ソース、ツリーハッシュ、プラグインの
フィールドは、無視されたうえで再シリアライズ時に落とされるのではなく、拒否されます。
ソースツリーおよびプラグインの SHA-256 値は、正確に 64 桁の小文字 16 進数でなければ
なりません。同じ正規 Git URL とコミットに対するエントリーは、ソースツリーのダイジェストが
一致していなければなりません。循環検証は明示的な型付き作業スタックを使用するため、
深くネストした非循環の依存関係グラフが Rust や WebAssembly の呼び出しスタックを
使い果たすことはありません。循環診断は、循環を閉じるパッケージインスタンスのパスを
保持します。唯一のパスソースはルートパッケージ自身であり、`.` に固定されています。
すべての依存関係エントリーは Git に裏付けられていなければならず、すべてのパッケージ
エントリーは宣言されたルートから到達可能でなければなりません。任意の絶対/親パスと
孤立したエントリーは、ローダーがソースルートを導出または正規化する前に拒否されます。

`graphcal check`、`graphcal eval`、`graphcal graph`、および LSP は、ロックファイルと
ローカルに実体化されたキャッシュエントリーのみを読み取ります。これらは依存関係を
取得したり `graphcal.lock` を更新したりしません。ロックファイルが存在しない、古い、
バージョンが不一致である、または存在しないかハッシュが一致しないキャッシュ済みソースを
指している場合、これらは失敗し、`graphcal deps lock` の実行を求めます。

`graphcal deps lock` は、ソースツリーのダイジェストが以前に検証されたロックと一致する
場合にのみ、取得なしでキャッシュ済みチェックアウトを再利用します。キャッシュの同一性は
型付きです。正規リモート URL と不変のコミットがソースディレクトリを選択し、検証済みの
ツリーダイジェストが不変の世代を選択します。同一ソースの書き込み側はアドバイザリロックで
直列化され、そのキャッシュディレクトリ内で取得をステージングし、リネームによって世代を
公開します。有効な古い世代は置き換えられないため、ルート化されたファイルシステム
ケーパビリティを保持する並行の評価器は引き続き使用可能です。
`GRAPHCAL_CACHE_DIR` はキャッシュルートを上書きします。相対値は、生成側とローダーの
パスが導出される前に、1 つの絶対的な同一性に解決されます。

!!! note "キャッシュレイアウトの移行"
    既存のロックファイルは引き続き有効ですが、コンテンツアドレス指定のレイアウトは、
    古いバージョンの Graphcal が書き込んだチェックアウトディレクトリを再利用しません。
    新しいキャッシュレイアウトを実体化するには、ネットワークアクセスのある状態で
    `graphcal deps lock` を一度実行してください。

読み込みはまた、モジュールを解決する前に、すべてのロックエントリーを正確に 1 つの
実体化されたマニフェストに束縛します。パッケージ名、ソースディレクトリ、依存関係
エイリアスの集合は、双方向で一致しなければなりません。これにより、モジュール解決に
使用されるソースディレクトリが、整合性検証の対象となるディレクトリと正確に一致することが
保証されます。古いロックファイルや手で編集されたロックファイルが、ハッシュ化されていない
ツリーにインポートをリダイレクトすることはできません。

カスタムフィールド、大文字のハッシュ、不正な形式のハッシュ、ルート以外のパスソース、
または到達不能なパッケージエントリーを持つ既存の手編集ロックファイルは、それらの
フィールド/エントリーを削除し、`graphcal deps lock` を実行して正規のピンを再生成
しなければなりません。`graphcal.lock` の内部にプライベートなメタデータを保持しないで
ください。別のファイルに保持してください。絶対パス、`.`/`..`、バックスラッシュ、
ドライブプレフィックスに似たコロン、または制御文字のコンポーネントを含むプラグインパスは、
アーティファクトをポータブルな相対パスに移動し、ロックを再生成しなければなりません。

ロックの作成と読み込みは、同じ決定論的なソースツリーアルゴリズムを使用します。
パッケージツリーには、通常のディレクトリと通常ファイルのみを含めることができます。
シンボリックリンクと特殊ファイルは、たどられるのではなく拒否されます。すべての正規パスは
ロックされたパッケージルートの下に留まらなければならず、相対パス名は UTF-8 でなければ
なりません。ルート化されたディスクアクセスは開かれたディレクトリケーパビリティに対する
ハンドル相対であるため、並行するファイル、親ディレクトリ、またはディレクトリ一覧の
シンボリックリンクの差し替えは、検証後に脱出することができません。
ルートパッケージは引き続きエディターのオーバーレイを使用し、キャッシュされた各依存関係は
独自の不変のルート化されたファイルシステムケーパビリティを通じて読み取られます。
オーバーレイの構築自体もケーパビリティを保持します。既存のバッファーは正規の基底同一性を
使用し、未保存のバッファーは、最も近い既存の親がルート化された基底を通じて正規化された
後にのみ受け付けられます。相対パス、そのルートの外側のパス、ファイル/ディレクトリの
衝突は拒否されます。無関係なワークスペースルートの開いているバッファーは、アクティブな
プロジェクトのスナップショットに追加されません。

プロジェクトの取り込みは、割り当ての前に上限が設けられます。CLI および LSP の読み込み、
`graphcal deps lock`、プラグイン検査は、`.gcl` ソース、ソースツリーファイル、または
WASM プラグインごとに 16 MiB、マニフェストごとに 1 MiB、ロックファイルごとに 4 MiB
という制限を共有し、読み込まれるアーティファクト/ソースツリーエントリーの合計 10,000 個
および 256 MiB という共有上限があります。プラグインのピン留めは、ファイルシステム検索の
前にポータブルなルート相対アーティファクトパスを検証し、モジュール全体を割り当てるのでは
なく、通常ファイルをインクリメンタルにハッシュ化します。ロックのパースはさらに、
グラフ検証の前に、パッケージエントリー数を構成されたファイル数の上限に制限します。
制限を超えるとローダーエラーになります。評価器の作業予算は、これらの先行する I/O 上限に
取って代わるものではありません。

### 依存関係の可視性 { #dependency-visibility }

依存関係名は、現在コンパイルされているパッケージインスタンスに対してローカルです。
パッケージ `mission` が `orbital.constants` をインポートすると、Graphcal は最初の
セグメントを `mission` の直接依存関係の名前空間で解釈します。

```text
current package: pkg-mission
first segment: orbital
mission.dependencies[orbital] = pkg-orbital
remaining path: constants
=> pkg-orbital/src/orbital/constants.gcl
```

パッケージが解決できるのは、自身のパッケージ名と自身の直接依存関係エイリアスだけです。
推移的な依存関係は暗黙的には見えません。`mission` が `orbital` に依存し、`orbital` が
`units` に依存する場合、`mission` が直接の `units` 依存関係またはエイリアスを併せて
宣言しない限り、`mission` のソースは `import units...` と書くことができません。
`orbital` は引き続き、`units` から派生した値、次元、単位、型、インデックス、DAG を
公開 API を通じて公開できます。その場合、`mission` はそれらを `orbital` を通じて指名し、
元のパッケージインスタンスの同一性が保持されます。

パッケージで定義された次元、単位、型、インデックス、代数的データ型は、パッケージ
インスタンスごとに名目的 (nominal) です。同じパッケージ名と同じソースの綴りを持つ
2 つのロック済みパッケージインスタンスは、Git コミット、ソースの同一性、または
依存関係グラフが異なる場合、別個のものです。標準ライブラリの次元と単位は例外です。
これらは、`graphcal.lock` に記録されたアクティブな Graphcal ツールチェーンと
標準ライブラリのバージョンに結び付けられた、シングルトンの同一性です。

## 標準ライブラリの予約: `graphcal` と `std` { #stdlib-reservation-graphcal-and-std }

最初のセグメント `graphcal` と `std` は、Graphcal の標準ライブラリのために
予約されています。ユーザーパッケージは `graphcal` や `std` と名付けることができず、
ユーザーソースは、標準ライブラリからインポートする場合を除き、どちらのセグメントでも
パスを始めることはできません。

```graphcal
import std.math::{sin, cos};   // (reserved) — stdlib import
```

標準ライブラリ自体はまだ設計中です。プロジェクトが実験的な標準ライブラリを明示的に
オプトインしない限り、ユーザーコードでの参照は「stdlib not yet available」診断で
拒否されます。

## 可視性、束縛可能性、入力ポート { #visibility-bindability-and-input-ports }

Graphcal は、2 つの外部境界の役割を区別しています。

- **エクスポート** は、`pub` または `pub(bind)` によって include/import 境界を
  またいで可視化された通常の宣言です。
- **入力ポート** は `param` で宣言されます。これは暗黙の `pub` マーカーを持つ通常の
  宣言ではなく、その入力ポートとしての役割は別途追跡されます。ポートは束縛可能であると
  同時に外部から読み取り/射影可能であるため、呼び出し側はその有効な束縛値/デフォルト値を
  観測できます。

通常の宣言では、可視性と束縛可能性は **2 軸に分かれます**。

- **可視性** (`pub`): 宣言が include/import 境界をまたいでエクスポートされるか
  どうか。
- **束縛可能性** (`pub(bind)`): インポート側が include 束縛でそれを *上書き* できるか
  どうか。束縛可能性はエクスポートの可視性を含意します。

| 注釈   | エクスポート? | 束縛可能? | 用途                                                                 |
|--------------|:---------:|:---------:|-------------------------------------------------------------------------|
| (なし)       | いいえ        | いいえ        | 内部ヘルパー、非公開の値                                        |
| `pub`        | はい       | いいえ        | 定数、利用側が読み取るが配線し直さない派生次元 / 単位 / 型 |
| `pub(bind)`  | はい       | はい       | 省略可能または必須のインデックス / 型 / 次元                             |

`param` は、この注釈マトリックスの外にあります。宣言の種類そのものが、名前付きの
DAG 入力ポートを直接作成します。

- `param x: T;` は必須の入力ポートです。
- `param x: T = expr;` はデフォルト付きの入力ポートです。
- 呼び出し可能な DAG のすべての param は、その名前付き include/呼び出しシグネチャに属します。
- 呼び出し可能な DAG の param の有効値は、選択または射影することもできます。
- エントリー DAG の params は、`--param` / `--params-json` /
  `--params-json-file` のインターフェースに属します。

`pub` は入力ポートに何の状態も追加しないため、注釈付きの両方の綴りは
パースエラーです。

```graphcal
pub param dry_mass: Mass = 1200.0 kg;   // parse error
pub(bind) param dry_mass: Mass;         // parse error
param dry_mass: Mass = 1200.0 kg;       // OK: defaulted input port
```

param は、引き続き出力として読み取り可能です。選択すると、供給された値、または
呼び出し側が束縛を省略した場合はそのデフォルトが返されます。

```graphcal
include external_dag()::{ x as default_x };
include external_dag(x: 1.0)::{ x as bound_x };

node projected: Dimensionless = @external_dag()::x;
```

その有効値を選択または再エクスポートしても、入力ポートの役割は推移的に保持されません。
自身の呼び出し側に `x` を束縛させたい中間 DAG は、独自の `param x` を宣言し、
ネストされた束縛でそれを転送しなければなりません。

### 内部の値はデフォルトで非公開 { #internal-values-are-private-by-default }

内部の計算値には `node` を、内部の固定値には `const node` を使用してください。特別な
名前を付けた `param` を使用しないでください。すべての param は DAG の入力シグネチャの
一部です。内部のパラメーター化は、非公開の DAG またはモジュール境界の背後に置くべき
ものです。

```graphcal
param dry_mass: Mass = 1200.0 kg;             // named input port
const node structure_mass: Mass = 500.0 kg;   // private fixed value
node wet_mass: Mass = @dry_mass + @structure_mass; // private computed value
```

非公開の、`param` でない項目をインポートすると、エラー `V001` が発生します。

```graphcal
// ERROR: cannot import private item `internal_helper` from `lib`
import lib::{internal_helper};
```

### 必須項目は `pub(bind)` でなければならない { #required-items-must-be-pubbind }

`pub(bind)` は、Static 宣言に 2 つの型付き入力の役割のいずれかを与えます。

- 本体/デフォルトを持つ宣言は **省略可能な入力** です。その include 束縛を省略すると
  宣言のデフォルトの同一性が保持され、供給すると具体的な束縛ターゲットが
  射影されます。
- 本体を持たない宣言は **必須の入力** であり、すべての include インスタンスは
  正確に同じカテゴリーの束縛を供給しなければなりません。

通常の `pub` 宣言は固定されており、上書きできません。必須の
`index` / `type` / `dim` 宣言はライブラリの束縛可能なインターフェースを形成するため、
`pub(bind)` と宣言しなければなりません。必須項目にマーカーなしの `pub` を書くと
エラー `V002` になります。

```graphcal
// ERROR: required index must be declared `pub(bind)`
pub index Phase;

// OK
pub(bind) index Phase;

// Required types and dims follow the same rule.
pub(bind) type Element;
pub(bind) dim Distance;
```

必須の `param` は V002 の対象外です。`param` は注釈のない入力ポートを直接宣言する
ものであり、必須であることはデフォルトの欠如によって表現されるためです。

### 公開内の非公開 (`V003`) { #private-in-public-v003 }

可視の宣言は、その有効なシグネチャで非公開の型システム項目
(`dim`、`type`、`index`、`base dim`)を参照してはなりません。これには、ネストされた
ジェネリック引数と、すべてのジェネリックパラメーターのデフォルトが含まれます。
公開型の引数を省略しても、非公開の型、次元、インデックスが黙って選択されては
なりません。コンパイラーは、エイリアスを通じたものも含め、正規の解決済み依存関係を
検査します。これにより、インポート側が見ることのできない名前の漏洩が防がれます。
この規則に違反するとエラー `V003` になります。

```graphcal
dim TransferSpeed = Length / Time;
// ERROR: `pub node` `speed` references private dim `TransferSpeed`
pub node speed: TransferSpeed = 10.0 m/s;

// Fix: make the dim visible too.
pub dim TransferSpeed = Length / Time;
pub node speed: TransferSpeed = 10.0 m/s;

// Defaults are part of the public signature too.
type Secret { Secret }
// ERROR: omitting T would expose private type `Secret`.
pub type Wrapper<T: Type = Secret> { Wrapper(value: T) }
```

### `pub(bind)` インデックスとバリアントリテラル (`V004`) { #pubbind-indexes-and-variant-literals-v004 }

`index` が `pub(bind)` と宣言されている場合、そのバリアントリテラルは、定義ファイルの
`node` / `const` 本体や、公開のシンク(`plot` / `assert` / `figure` / `layer`)に
現れることができません。理由は、インポート側がインデックスを異なるバリアント集合に
再束縛する可能性があり、それによってリテラルが孤立してしまうためです。
`for p: I { ... }` でインデックスを抽象化するか、バリアント固有の値を `param` に
移動してください。この規則に違反するとエラー `V004` になります。

```graphcal
pub(bind) index Phase = { Design, Test };
// ERROR: variant literal `Phase#Design` of `pub(bind) index` cannot be
//        used in the defining file
param phase_cost: Dimensionless[Phase];
pub node cost: Dimensionless = @phase_cost[Phase#Design];
```

### include の上書きは整合しなければならない (`V005`) { #include-overrides-must-reconcile-v005 }

include が束縛可能なシンボル `s` を上書きし、保持されているパラメーターのデフォルトの
いずれかが、名目的に `s` に結び付いた操作を依然として実行する場合、インポート側は
その依存パラメーターも *併せて* 再束縛しなければなりません。これらの依存関係には、
インデックスラベル、フィールド選択、コンストラクター呼び出しとマッチパターン、
名目的なインデックス/型のジェネリック引数が含まれます。これらはフィールドや
コンストラクターの綴りではなく、正規の意味的所有者によって比較されます。
たまたま同じ `x` フィールドや `Left` コンストラクターを持つ別の型に型を置き換えた
場合でも、整合が必要です。そうでなければ、デフォルトは異なる名目的契約の下で
黙って再解釈されることになります。これがエラー `V005` です。

```graphcal
// lib.gcl
pub(bind) index Phase = { Design, Test };
param cost: Dimensionless[Phase] = { Phase#Design: 1.0, Phase#Test: 2.0 };

// main.gcl
pub(bind) index NewPhase = { Review, Ship };
// ERROR: include overrides index `Phase` but does not re-bind `cost`,
//        whose default mentions `Phase#Design`
include lib(index Phase: NewPhase);

// Fix: re-bind `cost` as well.
include lib(
    index Phase: NewPhase,
    cost: { NewPhase#Review: 1.0, NewPhase#Ship: 2.0 },
);
```

`dim` および `param` の上書きは、V005 を決して引き起こしません。それらの置換は
全体的(代数的 / 値による)であり、孤立した名目的な言及を残さないためです。
診断は上書きを導入した include を指し示します。報告された各依存パラメーターを
明示的に束縛することで整合が取れます。

### 再エクスポートとジェネリクスの漏洩 (`V006`) { #re-exports-and-generics-leakage-v006 }

選択的にエクスポートされた include 出力は、その有効なシグネチャで束縛された型、次元、
インデックスに言及することがあります。include が `pub(bind)` シンボルをインポート側で
*非公開* の名前に置換した場合、下流の利用側は指名できないシンボルを見ることになります。
これがエラー `V006` です。

```graphcal
// container.gcl
pub(bind) type Element { Element }
pub const node origin: Element = Element;

// main.gcl
type Inner { Inner } // private at the importer
// ERROR: re-exported const node `origin` references private type `Inner`
include container(type Element: Inner)::{ pub origin };

// Fix: make the substituted name visible too.
pub type Inner { Inner }
include container(type Element: Inner)::{ pub origin };
```

V006 は、明示的に `pub` とマークされた出力/項目のみを検査します。依存関係に別の
公開出力を追加しても、インポート側のファサードは変わりません。この検査がたどるのは、
インポート側への型付きの include 置換だけです。置換されていない組み込み名、
依存関係ローカルの名前、修飾名は、同じ綴りのインポート側の宣言として再解釈される
のではなく、元の同一性を保持します。

### 再利用可能なテンプレート本体はパラメトリックでなければならない (`V007`) { #reusable-template-bodies-must-be-parametric-v007 }

ローカル定義を持つ `pub(bind)` Static 宣言は、定数ではなく省略可能な入力です。
Graphcal は、束縛可能なすべての `type`、`dim`、`index` を固定 (rigid) ポートとして
扱い、再利用可能な各 DAG 本体を一度だけ検査します。したがって、実行可能な本体は
許容されるすべての束縛に対して有効でなければならず、省略可能なポートのデフォルトの
具体的な構造や代数に依存することはできません。違反はエラー `V007` です。

例えば、束縛可能な型のデフォルトのフィールドとコンストラクターは、再利用可能な契約の
一部ではありません。

```graphcal
pub(bind) type Record { Record(x: Dimensionless) }
param record: Record;

// ERROR V007: another valid `Record` binding need not have field `x`.
pub node x: Dimensionless = @record.x;
```

同様に、束縛可能な次元のデフォルトと固定次元との等価性は、実行可能な本体を
正当化できません。

```graphcal
pub(bind) dim Output = Length;
param length: Length = 1.0 m;

// ERROR V007: `Output` may be rebound to a dimension other than Length.
pub node result: Output = @length;
```

代わりにポートをパラメトリックに使用するか、具体的な振る舞いや値を `param` を通じて
渡すか、デフォルト定義が契約の本質的な部分である場合は `bind` を取り除いてください。

```graphcal
pub(bind) dim Output = Length;
param value: Output;
pub node result: Output = @value; // OK for every Output binding
```

この規則は `node`、`const node`、`assert`、`plot`、`figure`、`layer`、および
ランタイム `unit` の本体を対象とします。パラメーターのデフォルトは、意図的に V007 の
対象外です。それらは入力のデフォルトとして検査され、名目的な `type`/`index` の
デフォルトは、include がその依存関係を置き換えた場合、引き続き V005 の整合の
対象となります。

## パラメーター化された include { #parameterized-includes }

`include` における param 束縛、または明示的にマークされた `type`、`dim`、`index`
束縛は、特定の値または型レベルの引数で依存関係をインスタンス化します。これが、再利用可能な「ライブラリ」
DAG を呼び出し箇所で特殊化する方法です。

### Param 束縛 { #param-bindings }

```graphcal
include nasa.rocket.compute_thrust(dry_mass: 800.0 kg)::{ thrust };
```

異なる値による複数のインスタンス化は、独立したサブグラフを
生成します。

```graphcal
include nasa.rocket.compute_thrust(dry_mass: 800.0 kg, isp: 320.0 s) as stage_1;
include nasa.rocket.compute_thrust(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

通常のインポートは、不変の検査済みモジュール定義を再利用します。この共有は
インスタンスをマージしません。等しい Static 束縛であっても、独立したパラメーター値、
ランタイムでスケールされる単位、アサーション検査を保持します。インポートされた定数は、
呼び出し側のランタイム環境ではなく、定義元モジュールの検査済みスコープに由来します。

束縛式は、外側のスコープの `@` 値を参照できます。

```graphcal
param my_mass: Mass = 800.0 kg;
include nasa.rocket.compute_thrust(dry_mass: @my_mass)::{ thrust };
```

### 必須パラメーター { #required-parameters }

デフォルト値なしで宣言された `param` は **必須** です。インポート側は
`include` 束縛(またはエントリーポイントファイルの場合は、コマンドラインの
`--param`、`--params-json`、`--params-json-file`)でそれを供給しなければなりません。

```graphcal
// lib/rocket_engine.gcl
param dry_mass: Mass;                     // required — must be provided
param fuel_mass: Mass;                    // required — must be provided
param isp: Time = 320.0 s;                // optional — has default

pub const node g0: Acceleration = 9.80665 m/s^2;
pub node v_exhaust: Velocity = @isp * @g0;
pub node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
pub node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```graphcal
// main.gcl
include lib.rocket_engine(dry_mass: 800.0 kg) as engine;
node dv: Velocity = @engine::delta_v;
```

`include` またはインライン DAG 呼び出しが必須の param を省略すると、コンパイラーは
そのインスタンス化箇所で `G004` を発行します。評価の準備時にエントリー DAG の param が
未充足のままである場合、コンパイラーはその宣言で `O003` を発行します。

### インデックス束縛 { #index-bindings }

[必須インデックス](indexes.md#required-indexes) を、互換性のある Index 引数で
束縛します。右辺は、include する側の DAG 内のインデックスを指名するか、構造的な
`Fin(N)` 軸を直接使用できます。

```graphcal
// lib/budget.gcl
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```graphcal
// main.gcl
pub index MyPhase = { Design, Build, Test };

include lib.budget(
    index Phase: MyPhase,
    cost: { MyPhase#Design: 10.0, MyPhase#Build: 20.0, MyPhase#Test: 5.0 },
)::{ total };

node result: Dimensionless = @total;  // 35.0
```

位置軸の DAG は、その濃度に対して名前付きエイリアスを必要としません。

```graphcal
dag scale {
    pub(bind) index Axis;
    param xs: Dimensionless[Axis];
    pub node ys: Dimensionless[Axis] = for i: Axis { @xs[i] * 2.0 };
}

include scale(
    index Axis: Fin(3),
    xs: table[Fin(3)] { 1.0; 2.0; 3.0; },
) as scaled;
```

`Fin` の濃度は通常の型レベル Nat の加算と乗算を使用し、この呼び出し箇所で具体的な
正の値に正規化されなければならず、実用上のインデックス濃度の制限の対象のままです。

#### 種類 (Kind) の一致 { #kind-matching }

制約のない必須インデックス(`pub(bind) index Axis;`)は離散ポートであり、名前付き
インデックスまたは `Fin(N)` のいずれかを受け付けます。具体的な束縛可能な名前付き
インデックスは名目的なままであり、別の名前付きインデックスにのみ再束縛できます。
座標インデックスは座標インデックスにのみ束縛できます。離散軸を座標ポートに束縛すること
(またはその逆)は、コンパイルエラーです。

#### 座標インデックスの次元の一致 { #dimension-matching-for-coordinate-indexes }

必須の座標インデックスを束縛する際、供給される座標インデックスは
**同じ次元** を持っていなければなりません。

```graphcal
// lib.gcl
pub(bind) index Step: Time;   // requires dimension Time

// main.gcl
index MyStep = range(0.0 s, 10.0 s, step: 1.0 s);     // OK
include lib(index Step: MyStep);

index DistStep = range(0.0 m, 100.0 m, step: 10.0 m); // dimension is Length
include lib(index Step: DistStep);                              // ERROR: dimension mismatch
```

### 部分的な束縛 { #partial-bindings }

デフォルトを持つ任意の param またはインデックスに対して、束縛は省略可能です。
上書きしたいものだけを束縛してください。残りはデフォルトを維持します。
必須インデックス(デフォルトのないもの)は、常に束縛しなければなりません。

```graphcal
// rocket.gcl has params: dry_mass (default), fuel_mass (default), isp (default)

// OK: only dry_mass is overridden; fuel_mass and isp keep their defaults
include lib.rocket(dry_mass: 800.0 kg) as r;

// OK: all params are explicitly bound
include lib.rocket(dry_mass: 800.0 kg, fuel_mass: 2800.0 kg, isp: 320.0 s) as r;
```

### 検証 { #validation }

- 束縛リストは、名前から引数への一意な写像です。束縛の順序は影響せず、値、
  インデックス、型、次元のターゲットを繰り返すことは、最後の束縛が勝つ上書きではなく
  コンパイルエラーです。
- マーカーのない束縛名は、正確に `param` 入力ポートを識別します。Static
  束縛には明示的な `index`、`type`、`dim` マーカーが必要であり、`pub(bind)` と
  マークされた宣言をターゲットとします。`param` マーカーは無効です。
- `node`、`const node`、または未知の名前を束縛することは、コンパイルエラーです。
- include/呼び出しされる DAG の必須入力ポートは、束縛によって供給されなければなりません。
  エントリー DAG の必須入力ポートは、`--param`、
  `--params-json`、または `--params-json-file` によって供給されなければなりません。
- すべての必須インデックスは、束縛によって供給されなければなりません。
- インデックス束縛の値は、インポート側のスコープにおける互換性のあるインデックス名か、
  具体的な構造的 `Fin(N)` 引数でなければなりません。外側のジェネリック DAG は
  必須インデックスを転送できます。include の連鎖は、最終的に具体的なインデックスを
  供給しなければなりません。
- 制約のない必須インデックスは、名前付きおよび構造的な有限軸を受け付けます。
  具体的な名前付きインデックスは名前付きのみのままであり、座標インデックスは
  座標のみのままです。座標インデックスの次元は、同時的な次元束縛を適用した後に
  一致しなければなりません。
- 座標インデックス束縛の次元は、同時的な型レベル置換が適用された後に
  検査されます。

## 循環インポート { #circular-imports }

Graphcal は、循環インポートをコンパイル時に検出します。

```graphcal
// a.gcl
import b::{x};

// b.gcl
import a::{y};
// ERROR: circular import detected
```

## プロジェクト編成パターン { #project-organization-patterns }

### 定数 / パラメーター / メイン { #constants-parameters-main }

一般的なパターンでは、単一パッケージ内の別々のファイルに関心事を
分離します。

```text
project/
  graphcal.toml   -- [package] name = "project"
  src/
    project/
      constants.gcl   -- shared physical constants
      params.gcl      -- tunable input parameters
      main.gcl        -- computation graph, imports the others
```

### ライブラリ / アプリケーション { #library-application }

再利用可能な DAG については、実パッケージにまとめて、完全なパスで
インポートします。

```text
project/
  graphcal.toml   -- [package] name = "project"
  src/
    project/
      lib/
        orbital.gcl   -- reusable orbital mechanics DAGs
        thermal.gcl   -- thermal analysis DAGs
      main.gcl        -- application-specific graph
```

### 必須パラメーターを持つ再利用可能なテンプレート { #reusable-templates-with-required-parameters }

必須の params を使用して、特定の値でインスタンス化しなければならないライブラリ
ファイルを作成します。

```graphcal
// src/project/lib/rocket.gcl
param dry_mass: Mass;          // required
param fuel_mass: Mass;         // required
param isp: Time = 320.0 s;     // optional default

pub const node g0: Acceleration = 9.80665 m/s^2;
pub node v_exhaust: Velocity = @isp * @g0;
pub node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
pub node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```graphcal
// src/project/main.gcl
include project.lib.rocket(dry_mass: 800.0 kg, fuel_mass: 2000.0 kg, isp: 320.0 s) as stage_1;
include project.lib.rocket(dry_mass: 500.0 kg, fuel_mass: 1200.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

### 必須インデックスを持つ再利用可能なテンプレート { #reusable-templates-with-required-indexes }

```graphcal
// src/project/lib/budget.gcl
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```graphcal
// src/project/main.gcl
pub index ProjectPhase = { Design, Build, Test };

include project.lib.budget(
    index Phase: ProjectPhase,
    cost: { ProjectPhase#Design: 10.0, ProjectPhase#Build: 20.0, ProjectPhase#Test: 5.0 },
)::{ total };

node project_cost: Dimensionless = @total;
```

## アサーションとモジュール境界 { #assertions-and-module-boundaries }

`import` はコンパイル時のみのものであり、隠れたデフォルトインスタンスを決して作成しない
ため、アサーションを評価したり伝播したりしません。アサーションのインポートを試みると
拒否されます (`M024`)。

すべての明示的な `include` は具体的なランタイムインスタンスを作成し、そのインスタンス内の
アサーションを、ネストされた include のアサーションも含めて評価します。ネストされた
インスタンスは、インポート側にリベースされる際に、完全な具体的所有者パスを保持します。
内部の所有関係の不整合は、テンプレートへの古い参照ではなくエラーになります。
インポート側の宣言が `#[assumes(...)]` でそのインスタンスローカルの結果を指名する
必要がある場合は、include のブレース内でアサーションを選択してください。

```graphcal
include project.checks(limit: 50.0)::{ limit, limit_positive };

#[assumes(limit_positive)]
node ratio: Dimensionless = @limit / 2.0;
```

詳細は [アサーション](assertions.md#assertions-in-multi-file-projects) を参照してください。

## 評価のエントリーポイント { #evaluation-entry-point }

`graphcal eval` を実行する際、エントリーファイルはコマンドラインで渡したファイルです。
すべての `import` および `include` の依存関係は、そのファイルから推移的に
解決されます。

```bash
graphcal eval project/src/project/main.gcl
```

エントリーファイルのパッケージの種類は、その所在によって決まります。
`graphcal.toml` が祖先ディレクトリに存在し、**かつ** エントリーファイルが
そのパッケージの名前空間
(`<source_dir>/<package_name>.gcl` または
`<source_dir>/<package_name>/` の下)の内側にある場合、マニフェストがパッケージ
レイアウトを定義し、ファイルはファイル横断インポートを使用できます。それ以外の場合、
すなわち祖先にマニフェストがない場合、またはマニフェストは存在するがエントリーファイルが
その名前空間の外側にある場合、ファイルは単一ファイルの仮想パッケージとして扱われ、
自己参照のみが可能です。

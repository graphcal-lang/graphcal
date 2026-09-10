---
icon: material/format-list-bulleted-type
---

# 型システム { #type-system }

このページは、Graphcal の型システムの正式なリファレンスです。カインド、型レベルの実体、3 つの型階層、それらの値が属する項レベルという実体の階層全体に加え、次元代数、式の型付け規則、ジェネリクス、型の等価性を説明します。

入門資料については[チュートリアル](../tutorial/index.md)を参照してください。個別の機能については、[次元と単位](dimensions-and-units.md)、[代数的データ型](algebraic-data-types.md)、[インデックス](indexes.md)、[DAG ブロック](functions.md)を参照してください。

## 型レベルの領域と記法 { #type-level-domains-and-notation }

以下の定義に登場する大文字は**メタ変数**であり、graphcal のソースに暗黙に存在する名前ではありません。

- `D` は**次元**全体を動きます。次元は、基本次元を有理数乗したものの積の標準形です。`Dimensionless` は単位元となる次元です。ジェネリック宣言の `<D: Dim>` は、この領域を動くソースレベルの変数を導入します。
- `S` は、サポートされる**時刻系**の閉じた集合、`UTC`、`TAI`、`TT`、`TDB`、`ET`、`GPST`、`GST`、`BDT`、`QZSST` を動きます。`S` は意味論上の記法にすぎません。graphcal には現在、ジェネリックカインド `TimeScale` はありません。
- `A` は、`type` 宣言で導入された名前全体を動きます。
- `Gᵢ` は、`A` の対応するジェネリックパラメーターが受け入れるジェネリック引数全体を動きます。各引数は、そのパラメーターのカインド（`Dim`、`Type`、`Index`、`Nat`）に照らして検査されます。
- `Iᵢ` は、有限で順序を持つ**インデックス軸**全体を動きます。軸は `index` 宣言から導入することも、`T[Fin(3)]` の `Fin(3)` のような明示的な構造的インデックスから導入することもできます。
- `J̄` は、空でもよい、順序を持つ状態軸の列全体を動きます。漸化式の規則では、`T[J̄]` は列が空なら `T`、それ以外なら `T[J₁, ..., Jₙ]` を意味します。空の `J̄` の前のコンマは省略されるため、`T[I, J̄]` は `T[I]` になります。
- `N` は、型レベルの**自然数**全体を動き、`Fin` の要素数や `Nat` ジェネリック引数として使われます。ジェネリック宣言の `<N: Nat>` は、この領域を動くソースレベルの変数を導入します。

`Quantity(D)`、`Complex(D)`、`Key(I)`、`Datetime(S)` の丸括弧は意味論上の記法にすぎず、コンストラクター呼び出しでも、そのまま記述できるソース構文でもありません。`Complex(D)` と `Key(I)` のソース上の型は `Complex<D>` と `Key<I>` です。`A<G₁, ..., Gₙ>` と `T[I₁, ..., Iₘ]` は graphcal のソース構文に対応しています。山括弧はジェネリックな代数的型の適用を表し、角括弧は値型に 1 つ以上の軸によるインデックスを付けます。

`Dim`、`Type`、`Index`、`Nat` は**ジェネリックパラメーターのカインド**であり、値型ではありません。厳密な定義は[ジェネリックパラメーターのカインド](#generic-parameter-kinds)を参照してください。カインド付けの記法では、次のように表します。

```text
D : Dim        D is a dimension
S : TimeScale  S is a supported time scale (semantic notation only)
T : Type       T is a single-value type, called a ValueType below
I : Index      I is a finite, ordered index axis
N : Nat        N is a type-level natural number
```

組み込みおよびユーザー宣言の型レベルの構成子には、次のカインドレベルのシグネチャがあります。矢印は graphcal の関数型ではありません。

```text
Quantity : Dim -> Type
Complex  : Dim -> Type
Key      : Index -> Type
Int      : Type
Bool     : Type
Datetime : TimeScale -> Type
A        : K₁ × ... × Kₙ -> Type     for generic kinds Kᵢ declared by A
Fin      : Nat -> Index               with the validity obligation N > 0
range    : Quantity(D) × Quantity(D) × Quantity(D) -> Index   (start, end, step:)
linspace : Quantity(D) × Quantity(D) × Nat -> Index           (start, end, points:)
_(min: _, max: _) : Quantity(D) | Int | Datetime(S) -> ConstrainedType
_[_]     : ConstrainedType × one-or-more Index axes -> DeclType
```

したがって、`Type` は単一の値の型を要素とするカインドです。このページでは、その要素を **ValueType** と呼びます。`Index` は軸を要素とするカインドであり、ラベルやインデックス付きコレクションを要素とするものではありません。`range` と `linspace` は `index` 宣言の右辺でのみ使用できます。一方、`Fin(N)` は Index が求められる任意の位置に直接記述できます。すべての ValueType は境界のない ConstrainedType でもあるため、通常のインデックス付き形式は単純な `T[I]` です。ConstrainedType と DeclType はともに[実体の階層](#entity-stratification)で定義します。

## 実体の階層 { #entity-stratification }

Graphcal プログラム内のすべての実体は、いずれか 1 つのユニバースの、厳密に 1 つの階層に属します。以下の文法は、カインド、それらが分類する型レベルの実体、3 つの型階層、それらの値が属する項レベルという全体像を一か所にまとめたものです。

```text
Kinds:   Dim | TimeScale | Type | Index | Nat

Level 0: type-level entities, classified by kinds
  D : Dim        = B | Dimensionless | D * D | D / D | D^(p/q)
  S : TimeScale  = UTC | TAI | TT | TDB | ET | GPST | GST | BDT | QZSST
  N : Nat        = 0 | 1 | 2 | ... | N + N | N * N
  I : Index      = X | C | Fin(N)                                (N >= 1)

Level 1: Primitive = Quantity(D) | Complex(D) | Key(I) | Int | Bool | Datetime(S)

Level 2: ValueType (T : Type) = Primitive | A | A<G₁, ..., Gₙ>
         GenericArg         G = D | T | I | N

Level 3: DeclType             = CT | CT[I₁, ..., Iₘ]             (m >= 1)
         ConstrainedType   CT = T | T(min: b, max: b)

Term level: runtime entities, classified by types
  v : T                a single value — one DAG node
  w : T[I₁, ..., Iₘ]   an indexed value — a total map, one v per label tuple
```

この文法で追加したメタ変数は次のとおりです。

- `B` は**基本次元**全体を動きます。これには `base dim` 宣言と、プレリュードの基本次元（SI の 7 つの基本次元と `Angle`）が含まれます。
- `X` は**名前付きインデックス**全体を動き、それぞれが順序付きラベル `X.ℓ₁, ..., X.ℓₖ` を宣言します。
- `C` は**座標インデックス**全体を動きます。これは `range` または `linspace` で構築される、有限で順序付きの `Quantity(D)` 座標列です。
- `b` はコンパイル時定数の境界式全体を動きます。制約の境界はいずれも省略できます。

各型レベルの領域には、そのカインドを持ち、スコープ内にある**ジェネリックパラメーター**も含まれます。`type Vec3<D: Dim, I: Index, N: Nat, F: Type>` の内部では、パラメーター `D` は次元、`I` はインデックス軸、`N` は Nat、`F` は ValueType です。`TimeScale` は、ジェネリックパラメーターもユーザー宣言も持たない唯一のカインドです。その 9 つの要素は閉じた集合を構成し、`Datetime<S>` 型構文と静的な `epoch<S>` 引数にのみ現れます。

これらの階層を結ぶ構成子は、`Quantity : Dim -> Type` から `_[_] : ConstrainedType × Index axes -> DeclType` まで、[型レベルの領域と記法](#type-level-domains-and-notation)にカインドレベルの矢印シグネチャで一覧化しています。項レベルで対応するものは[項レベル](#the-term-level)に示します。

この階層の*外部*にある実体、すなわち単位、ラベル、コンストラクター、関数、宣言、モジュールは、後述の[宣言と、それが導入する実体](#declarations-and-the-entities-they-introduce)および[組み込み・ローカル・境界の実体](#built-in-local-and-boundary-entities)に一覧化しています。

### レベル 0: 型レベルの実体 { #level-0-type-level-entities }

これらの実体は、プログラムの検査中にのみ存在します。いずれも値ではなく、型ではなくカインドによって分類されます。

**次元**（`D : Dim`）は、`Dimensionless` を単位元として基本次元上の代数を構成します。

```text
_ * _    : Dim × Dim -> Dim
_ / _    : Dim × Dim -> Dim
_ ^(p/q) : Dim -> Dim                for each non-zero rational p/q
```

次元の標準形は、基本次元を有理数乗したものの積です。`Velocity` のような名前付きの組立次元は透過的な別名です。[次元代数](#dimension-algebra)を参照してください。

**時刻系**（`S : TimeScale`）は、上記の 9 要素からなる閉じた集合です。`S` は意味論上の記法にすぎず、ソースレベルの `TimeScale` カインドや時刻系変数は存在しません。その表記は、`Datetime<UTC>` や `epoch<UTC>(...)` のような Static の時刻系位置でのみ解決されます。通常の式では代わりに Term の名前解決が行われます。そのため Term のコンストラクターに `UTC` という名前を付けることはできますが、そのような Term が存在しない場合の裸の `UTC` は実行時の値にはならず、名前空間の誤りを示す診断を受けます。

**Nat**（`N : Nat`）は型レベルの自然数であり、リテラル、`Nat` ジェネリックパラメーター、それらの多項式からなります。

```text
_ + _ : Nat × Nat -> Nat
_ * _ : Nat × Nat -> Nat
```

減算はありません。大きい側を加算で表現してください（入力を `Fin(N + 1)`、出力を `Fin(N)` とします）。Nat 式は正規化され、2 つの Nat が等しいのは、その正規形が一致する場合に限ります。Nat の用途は、`Fin(N)` の要素数と `Nat` ジェネリック引数の 2 つだけです。

**インデックス軸**（`I : Index`）には 3 つの具体的な形式があり、それぞれが固有の要素順序を持ちます。

| 形式 | 宣言方法 | 要素 | 要素の順序 |
|------|-------------|----------|---------------|
| 名前付き `X` | `index X = { ℓ₁, ..., ℓₖ };` | ラベル `X.ℓᵢ` | ラベルの宣言順 |
| 座標 `C` | `index C = range(...);` または `linspace(...)` | `Quantity(D)` 座標 | 開始点から終了点へ向かう座標順 |
| 構造的 `Fin(N)` | Index が求められる位置に直接記述 | 整数位置 `0 ... N-1` | 位置の昇順 |

インデックス構成子 `range`、`linspace`、`Fin` のカインドレベルの矢印シグネチャは、[型レベルの領域と記法](#type-level-domains-and-notation)に示しています。名前付きインデックスには構成子がなく、ラベル集合そのものが宣言です。

`Fin(0)` は無効であり、コンパイラーの実用上の上限を超える要素数も拒否されるため、すべての軸は有限で空ではありません。軸の要素は型レベルの実体のままですが、それぞれが項レベルでは一様に `Key(I)` 型の**キー**として反映されます（[インデックスの機能](#index-capabilities)を参照）。修飾付きラベルは `Key(X)` 定数です。座標および `Fin` のループ変数は `Key(C)` / `Key(Fin(N))` の値を束縛し、その座標や整数の内容は `coord` / `to_int` で明示的に取り出します。

### レベル 1–3: 型の階層 { #levels-13-the-type-hierarchy }

- **Primitive（レベル 1）** — 分割不能な原子的データです。実数量、次元を持つ複素数量、インデックスキー、整数、真偽値、または 1 つの時刻系に属する日時を表します。
- **ValueType（レベル 2）** — 単一の論理的な値です。プリミティブ、または公称的な代数的型のインスタンスです。これはカインド `Type` の領域であり、1 つの DAG ノードが格納し、1 つの DAG パラメーターが受け渡し、式が計算するものです。
- **ConstrainedType** — 必要に応じて境界を含む `min`/`max` の値域制約で絞り込んだ ValueType です。制約を付けられるのは、数量、`Int`、`Datetime<S>` 型だけです。[値域制約](#domain-constraints)を参照してください。
- **DeclType（レベル 3）** — 型注釈が表すものです。制約付きの型に、必要に応じて 1 つ以上の軸によるインデックスを付けます。注釈は `param`、`node`、`const node` 宣言、DAG パラメーター、コンストラクターのペイロードフィールドに現れます。インデックスを付けると、1 つの ValueType がラベルタプルから値への全域写像に持ち上げられます。インデックス付き注釈の制約は要素ごとに適用されます。

裸の `A` は、非ジェネリックな代数的型（またはすべての引数にデフォルト値があるジェネリック型）です。すべての `type` 宣言は、コンストラクターが 1 つでも複数でも、同じ公称的な代数的型の概念を定義します。コンストラクターとそのペイロードフィールドは、`A` の値の構成方法を記述します。それ自体が別の型なのではないため、`ValueType` の定義に独立した選択肢としては現れません。

ジェネリック引数 `G` はカインド検査を受け、表記によって分類されることはありません。各引数位置は宣言された `Dim`、`Type`、`Index`、`Nat` カインドを要求し、カインドを越える指定はエラーです（`ByNat<Fin(3)>`、`Dimensionless[3]`）。`Type` カインドに含まれるのは ValueType だけです。インデックス軸や `Velocity[Maneuver]` のようなインデックス付き DeclType は、有効な `Type` 引数ではありません。

### 次元を持つ複素数量 { #dimension-aware-complex-quantities }

`Complex : Dim -> Type` は組み込みのプリミティブ型構成子です。`Complex<D>` の値は、実部と虚部を SI 基本単位の binary64 成分として格納し、両成分は同じ次元 `D` を持ちます。

```graphcal
node displacement: Complex<Length> = complex(3.0 m, 4.0 m);
node dimensionless: Complex<Dimensionless> = complex(1.0, -2.0);
```

次元引数は必須であり、単位ではなく次元を指定します。`Complex<Length>` と `Complex<Length / Time>` は有効ですが、裸の `Complex` と `Complex<m>` はエラーです。ジェネリックな次元パラメーターを直接使用できます。

```graphcal
type Phasor<D: Dim> {
    Phasor(value: Complex<D>),
}
```

Graphcal には複素数リテラル構文はありません。`complex(re, im)`、`polar(magnitude, phase)`、`to_complex(real)` によって明示的に構築します。明示的な構築により、`i` のような識別子が特別な字句的意味を持つことを防ぎ、実数から複素数への昇格を可視化します。[複素数関数](built-ins.md#complex-functions)と[複素数演算の対応表](expressions.md#complex-arithmetic)を参照してください。

複素数値にはインデックスを付けることができ、代数的型のペイロードにも格納できます。`count` は要素型に依存しないため、インデックス付き複素数値を受け入れます。数量の集約と線形代数カーネルは、まだ複素数要素を受け入れません。また、複素数値は実験的なプラグイン ABI を通過できません。`z` を直接プロットする代わりに、`re(z)`、`im(z)`、`abs(z)`、`phase(z)` をプロットしてください。

### 必須実体（ホール） { #required-entities-holes }

次元、代数的型、名前付きまたは座標インデックスは、*必須*として宣言できます。`pub(bind) dim D;`、`pub(bind) type T;`、`pub(bind) index X;`、`pub(bind) index C: Time;` は、対応するユニバースに、定義を持たない抽象的な要素を導入します。必須実体は、ライブラリやローカルの `dag` ブロック内では具体的な実体と同様に使用し、`include` 束縛で名前を指定して具体的な実体に束縛します。これは `param` 値を渡すのと同じ束縛構文です。項レベルと型レベルの両方の引数が、include 境界を通過します。この明示的な必須実体のパターンにより、暗黙のジェネリック推論を使わずに、次元と軸に関して多相な再利用可能 DAG を提供できます。単位と時刻系には必須形式がありません。[可視性・束縛可能性・入力ポート](multi-file.md#visibility-bindability-and-input-ports)を参照してください。

### 項レベル { #the-term-level }

値は ValueType に属します。値は実数量または複素数量、整数、真偽値、日時、あるいはコンストラクターによって構築した代数的な値です。インデックス付きの値は、軸のラベルタプルごとに 1 つの値を持つ全域写像です。値には、型に影響しない 2 種類の表示メタデータが付随します。実数量または複素数量の**単位**と、日時の**タイムゾーン**です。いずれも表示だけを選択します。実部と虚部は SI 基本単位で、時点はその時刻系で格納されます。

構造的な値の構成子と除去子には、次のシグネチャがあります。ここで `ℓᵢ` は軸 `Iᵢ` のラベル、`U` は漸化式の状態の要素 ValueType、`J̄` は空でもよい固定軸の列です。

```text
Cᵢ          : DT₁ × ... × DTₖ -> A                value former, one per constructor of A
map / table : T at each label tuple -> T[I₁, ..., Iₘ]
for         : T at each label tuple -> T[I₁, ..., Iₘ]
_[_]        : T[I₁, ..., Iₘ] × Key(I₁) × ... × Key(Iₘ) -> T
_.fᵢ        : A -> DTᵢ                            single-constructor A only
scan        : T[I] × U[J̄] × (U[J̄] × T -> U[J̄]) -> U[I, J̄]
unfold      : I × U[J̄] × (U[J̄] × Key(I) × Key(I) -> U[J̄]) -> U[I, J̄]
sum, maximum, minimum, mean, rss : Quantity(D)[I] -> Quantity(D)
argmax, argmin : Quantity(D)[I] -> Key(I)
product     : Quantity(D)[I] -> Quantity(D^|I|)
count       : T[I] -> Int
X.ℓ         : Key(X)                              label expression, one per label of X
key         : Fin(N) × static Nat -> Key(Fin(N))  compile-time range check
fin_key     : Fin(N) × Int -> Key(Fin(N))         runtime range check
floor_key, ceil_key, nearest_key : C × Quantity(D) -> Key(C)
coord       : Key(C) -> Quantity(D)               C a coordinate axis over D
to_int      : Key(Fin(N)) -> Int                  Fin keys only
_ + _       : Key(Fin(N)) × static Nat c -> Key(Fin(N + c))
dot         : Quantity(D1)[I] × Quantity(D2)[I] -> Quantity(D1 × D2)
matmul      : Quantity(D1)[I, J] × Quantity(D2)[J, K] -> Quantity(D1 × D2)[I, K]
transpose   : Quantity(D)[I, J] -> Quantity(D)[J, I]
trace       : Quantity(D)[I, I] -> Quantity(D)
norm        : Quantity(D)[I] -> Quantity(D)
cross       : Quantity(D1)[I₃] × Quantity(D2)[I₃] -> Quantity(D1 × D2)[I₃]
outer       : Quantity(D1)[I] × Quantity(D2)[J] -> Quantity(D1 × D2)[I, J]
solve       : Quantity(D1)[I, I] × Quantity(D2)[I] -> Quantity(D2 / D1)[I]
inverse     : Quantity(D)[I, I] -> Quantity(D⁻¹)[I, I]
det         : Quantity(D)[I, I] -> Quantity(D^|I|)
```

このページのほかの箇所と同様に、矢印は意味論上の記法であり、関数型ではありません。コンストラクター呼び出しでは必ずフィールド名を記述します（`Cᵢ(f₁: v₁, ...)`）。ジェネリックな `A` のコンストラクターは、適用済みの `A<G₁, ..., Gₙ>` を生成します。`scan` と `unfold` のクロージャー位置は特殊構文であり、関数値ではありません。`unfold` の第 1 引数は次元 `D` の座標インデックスへの明示的な参照で、そのクロージャーは前と現在の要素キー `Key(I)` を束縛します。漸化式の状態には固定軸 `J̄` を持たせることができ、漸化式の軸はその先頭に追加されます。これによってインデックス付き宣言型が `Type` カインドの要素になるわけではありません。マップとテーブルのリテラルは全域的である必要があり、インデックス順に正規化されます。要素アクセスでは各軸のキーを指定する必要があります（修飾付きラベルが定数キーの表記です）。集約は厳密に 1 軸だけを縮約します。これには `argmax`/`argmin` も含まれ、同値の極値がある場合はインデックス順で最初のものを選びます。`key` の静的な位置と Fin キーの `+` はコンパイル中に検査され、`fin_key`、`floor_key`、`ceil_key` は評価時に範囲検査されます。線形代数の縮約では、対応する各位置に同じ型付き軸が必要です。`I₃` は厳密に 3 要素を持つ軸を表します。

項レベルの名前は、`param`（名前付き DAG 入力ポート）、`node`（計算される値）、`const node`（コンパイル時の値）によって束縛されます。複数宣言は、共有する 1 つのテーブルリテラルから複数のスロットを束縛します。ローカル名、すなわちループ変数、`match` 束縛、`scan`/`unfold` のクロージャー変数は、単一の式の内部で値（要素キー、累積値、要素値）を束縛します。
項レベルに第一級関数はありません。組み込み関数、外部関数、DAG ブロックは呼び出すかインスタンス化するものであり、格納することはできません。

### DAG との対応 { #dag-correspondence }

この階層は計算モデルに直接対応しています。

> 評価 DAG のノードは **ValueType** 型を持ちます。`ValueType[Index]` 型の宣言は、インデックスラベルごとに 1 つの DAG ノードへ展開されます。`ValueType[I, J]` 型の宣言は、ラベルタプル `(i, j)` ごとに 1 つの DAG ノードへ展開されます。

| 宣言型 | DAG ノード |
|-----------------|-----------|
| `node x: Velocity` | 1 ノード |
| `node x: Velocity[Maneuver]`（3 ラベル） | 3 ノード |
| `node x: Velocity[Phase, Maneuver]`（2 x 3） | 6 ノード |

`for` 内包表記は、単一の宣言を複数の DAG ノードへ展開します。各ノードは、データ依存関係を除けば独立に評価できるため、インデックス付きの値は自然に並列化できます。これは、インデックス付きの値の要素ごとの算術演算や比較に明示的な `for` が必要な理由でもあります。コレクション全体を操作するのではなく、個々の DAG ノードの計算を定義するためです。

### 宣言と、それが導入する実体 { #declarations-and-the-entities-they-introduce }

各宣言形式は、1 つのユニバースに実体を導入します。後述の[名前のユニバース](#name-universes)では、各スコープで末端名が重複しないようにします。次の表は、宣言形式の完全な一覧です。

| 宣言 | 導入するもの | ユニバース | 分類基準 |
|-------------|-----------|----------|---------------|
| `base dim Information;` | 基本次元 | 型 | `Dim` |
| `dim Velocity = Length / Time;` | 組立次元（透過的な別名） | 型 | `Dim` |
| `base unit bit: Information;` | ユーザー定義の基本次元の唯一の基準単位 | 単位 | その次元 |
| `const unit km: Length = 1000 m;` | コンパイル時に倍率が決まる単位 | 単位 | その次元 |
| `unit EUR: Money = (@rate) USD;` | 実行時に倍率が決まる単位 | 単位 | その次元 |
| `type A<...> { C1(...), C2, ... }` | 代数的型とそのコンストラクター | 型。コンストラクターはコンストラクター名前空間 | `Type`。コンストラクターは `A` の値を構成します |
| `index X = { ... };` | 名前付き軸とそのラベル | インデックス。ラベルは `X.` の下で修飾 | `Index` |
| `index C = range(...);` / `linspace(...);` | 座標軸 | インデックス | `Index` |
| `pub(bind) dim D;` / `type T;` / `index X;` / `index C: D;` | 必須実体（ホール） | 型またはインデックス | そのカインド |
| `param p: DT;` / `param p: DT = expr;` | 名前付き DAG 入力ポート | 値 | その DeclType |
| `node n: DT = expr;` | 計算されるグラフ値 | 値 | その DeclType |
| `const node k: DT = expr;` | コンパイル時の値 | 値 | その DeclType |
| `param a: T[I], node b: U[I] = table[...] { ... };` | 軸を共有する複数のポート／値（複数宣言） | 値 | スロットごとの DeclType |
| `assert a = expr;` | 検査されるアサーション | 値 | 本体: `Bool`、`Bool[I]`、または `actual ~= expected +/- tolerance` |
| `plot p = { ... };` | チャート仕様 | 値 | — |
| `figure f = { ... };` / `layer l = { ... };` | プロットの合成（タイル配置／重ね合わせ） | 値 | — |
| `dag d { ... }` | 再利用可能なサブ DAG のひな形 | 値 | —（関数型はありません） |
| `import pkg.mod;` / `... as m;` / `...::{ items }` | 呼び出し可能な DAG モジュールの別名、または列挙した項目 | モジュール／DAG、または項目ごと | — |
| `import plugin "name" as ns { fn ...; }` | `ns::fn(...)` として呼び出せる外部関数 | プラグイン別名 | `Dim`/`Index` 変数に関する外部シグネチャ |
| `include pkg.dag(bindings) ...;` | 埋め込まれた DAG インスタンス。選択した出力はノードになります | 値 | 出力の DeclType |
| `type` ヘッダー内の `<P: Dim>` など | ジェネリックパラメーター | その宣言のスコープ内 | 宣言されたカインド |

### 組み込み・ローカル・境界の実体 { #built-in-local-and-boundary-entities }

残りの実体は、言語に組み込まれているもの、単一の式にローカルなもの、または構築と表示の境界にのみ存在するものです。

| 実体 | 由来 | 分類基準 | 出現する場所 |
|--------|-----------|---------------|------------------|
| プレリュードの次元と単位 | プレリュード | `Dim` / その次元 | 型構文、リテラルと変換先 |
| 時刻系 `UTC` ... `QZSST` | 組み込みの閉じた集合 | `TimeScale`（意味論上） | `Datetime<S>` 型構文、`epoch<S>` |
| 組み込み定数 `PI`、`E`、`TAU` | プレリュード | `Dimensionless` | 式内で裸の名前で参照 |
| 組み込み関数（`sqrt`、`sum`、...） | プレリュード | 次元多相なシグネチャ | 呼び出し位置のみ |
| `scan` / `unfold` | 特殊構文形式 | それぞれの型付け規則 | 式。クロージャーは構文であり値ではありません |
| インデックスラベル `X.ℓ` | 名前付きインデックス宣言 | 式では `Key(X)`、パターンに類する位置ではセレクター | 式、インデックスアクセス、マップ／テーブルのキー、`match` パターン、`#[expected_fail(...)]` のキー、include のインデックス束縛 |
| 座標 | 座標インデックス宣言 | `Key(C)`。座標は `coord` で取り出します | ループ変数と座標検索を介して、インデックスアクセス、等値比較、`coord` による取り出し |
| `Fin` の位置 | 構造的な軸 | `Key(Fin(N))`。整数は `to_int` で取り出します | ループ変数、`key`、`fin_key` を介して、インデックスアクセス、等値比較、加算によるキー演算 |
| コンストラクター | `type` 宣言 | その代数的型の値を構成 | 構築呼び出しと `match` パターン |
| ペイロードフィールド | コンストラクター宣言 | その DeclType | `Ctor(field: expr)` による構築、`.field` アクセス、パターン束縛 |
| ループ変数 | `for v: I` | 反復対象の軸の `Key(I)` | 内包表記の本体 |
| `match` 束縛 | パターン内の `field: name` | 束縛したフィールドの型 | アームの式 |
| クロージャー変数 | `scan` の束縛変数 `acc`、`item`。`unfold` の束縛変数 `prev_state`、`prev_i`、`i` | 累積値／要素値。`unfold` の `prev_i`、`i` は `Key(I)` | `scan` / `unfold` の本体 |
| 属性 | 宣言の前の `#[...]` | 閉じた集合: `assumes`、`expected_fail`、`hidden`。`lazy` は予約済みですが拒否されます（A023） | 宣言メタデータ |
| 文字列リテラル | 引用符で囲んだ単一行のテキスト | —（`String` ValueType はありません） | `datetime`/`epoch` 引数、タイムゾーン表示先、プロットのプロパティ、プラグインのパス |
| タイムゾーン | 引用符で囲んだ IANA 名 | 同梱の tzdb に照らして検証 | `datetime(..., tz)` による構築、`-> "Area/Location"` による表示 |
| モジュール／パッケージ | ディレクトリツリーと `import` | — | ドット区切りのパス接頭辞 |

## 名前のユニバース { #name-universes }

Graphcal は、1 つのスコープ内で 3 つの宣言ユニバースを排他的に保ちます。

- **型ユニバース**: 代数的型と次元。
- **インデックスユニバース**: 名前付きおよび座標インデックスの宣言。
- **値ユニバース**: `param`、`node`、`const node`、`assert`、プロット、figure、layer、`dag` の宣言。

同じスコープ内では、末端名を宣言できるのはこれらのユニバースのうち 1 つだけです。たとえば、`type M { Mk }` と `index M = { A }` は名前の重複エラーになります。`dim M = Length;` と `type M { ... }` または `index M = { ... }` の組み合わせも同様です。

コンストラクターは独自のコンストラクター名前空間に属します。そのため、レコード形状の宣言では、慣用的な `type T { T(...) }` という表記を引き続き使用できます。型名とそのコンストラクター名は、型・インデックス・値の位置で競合しません。

単位は、数量リテラルや変換先などの単位構文でのみ参照されます。単位名が型・インデックス・値の末端名と一致しても、それらの位置でどの宣言に解決されるかは変わりません。逆方向の名前解決は意図的に厳密です。単位構文で同じ表記の非単位宣言が見つかった場合、意味的な同一性が異なるプレリュードやレジストリの単位へ暗黙にフォールバックせず、誤ったユニバースの宣言として報告します。

## 型の分類 { #type-categories }

### プリミティブ（レベル 1） { #primitives-level-1 }

分割不能な型です。それぞれが単一の原子的データを表します。

| 意味論上の型 | 表現 | 次元を持つか |
|---------------|----------------|----------------------|
| `Quantity(D)` | SI 基本単位での IEEE 754 binary64 の大きさ | はい |
| `Key(I)` | 軸の同一性と要素位置 | いいえ |
| `Int` | 64 ビット符号付き整数 | いいえ |
| `Bool` | 真偽値 | いいえ |
| `Datetime(S)` | 時刻系 `S` における高精度のエポック | いいえ |

評価器の境界では、`Quantity` の大きさと `Complex` 値の両直交成分は、有限の binary64 数でなければなりません。NaN と無限大は、意味論上の実行時の値になる前に拒否されます。符号付きゼロと表現可能な符号付き非正規化数は有効なままであり、正規化によって消去されません。数値カーネルは中間結果に生の binary64 作業バッファーを使用できますが、その出力は、実行時の値グラフに入る前にこの有限値の境界を通過します。

`Quantity(D)`、`Key(I)`、`Datetime(S)` は意味論上の記法であり、そのまま記述できるソース構文ではありません。ソース上の表記は次のとおりです。

```text
Quantity(D)    -> D
Key(I)         -> Key<I>
Datetime(UTC)  -> Datetime
Datetime(S)    -> Datetime<S>
```

!!! important "ソース上に `Quantity` コンストラクターはありません"
    数量型は `Length`、`Dimensionless`、`Length / Time` のように、その次元で記述してください。`Quantity<Length>` と裸の `Quantity` は Graphcal の型ではありません。

#### 数量型 { #quantity-types }

数量は、コンパイル時に既知の**次元**を持つ binary64 浮動小数点の大きさです。単位は値／表示のメタデータであり、型の一部ではありません。

```
param mass: Mass = 1200.0 kg;           // quantity of dimension Mass
param ratio: Dimensionless = 0.85;      // identity-dimension quantity
```

`Dimensionless` は単位元となる次元の数量型です。同じ次元の 2 つの数量を除算すると、結果の型は `Dimensionless` になります。

浮動小数点演算は IEEE 754 binary64 の規則に従います。実行時に NaN と無限大を検出し、報告します。

物理次元を持つのは数量だけです。`Key(I)`、`Int`、`Bool`、`Datetime(S)` は別のプリミティブ型群であり、数量ではありません。

#### インデックスキー { #index-keys }

`Key<I>` は軸 `I` の要素キーの ValueType です。`Dim`/`Quantity(D)` のパターンに対応し、型レベルの軸を項レベルに反映したものです。`I` は、名前付き軸、座標軸、`Fin` 軸、必須軸のいずれでもかまいません。キーは軸の同一性と要素位置を格納するため、座標キーを使うと正確に再インデックスできます。

```
param delta_v: Velocity[Maneuver] = { ... };
node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];
```

キーは、修飾付きラベル式（`Maneuver#Departure : Key<Maneuver>`）、ループ変数、`argmax`/`argmin`、明示的な構成子 `key`、`fin_key`、`floor_key`、`ceil_key`、`nearest_key` によって導入されます。キーの除去には、要素アクセス、同じ軸の `==`/`!=`、網羅的な `match`（具体的な名前付き軸）、および `coord`（座標キー）と `to_int`（`Fin` キー）による取り出しを使います。完全なリファレンスは[インデックスキー](indexes.md#index-keys)を参照してください。`Complex` と同様に、キーは実験的なプラグイン ABI を通過できません。

#### Int { #int }

`Int` は 64 ビット符号付き整数です。常に無次元であり、物理次元を持つことはできません。

```
param item_count: Int = 42;
const node seven: Int = 7;
```

整数演算には検査付き演算を使用します。オーバーフローは実行時エラーであり、暗黙に折り返すことはありません。

#### Bool { #bool }

`Bool` は条件と論理式に使用します。

```
param enabled: Bool = true;
node active: Bool = @enabled && @item_count > 0;
```

#### Datetime { #datetime }

`Datetime` は正確な時点を表します。その時点の解釈を決める**時刻系**でパラメーター化されます。裸の `Datetime` は UTC をデフォルトとします。

```
param launch: Datetime = datetime("2024-11-05T12:00:00Z");
param t_tt: Datetime<TT> = epoch<TT>("2024-11-05T12:00:00");
```

サポートする時刻系: `UTC`、`TAI`、`TT`、`TDB`、`ET`、`GPST`、`GST`、`BDT`、`QZSST`。

時刻系名は、`Datetime<S>` と `epoch<S>(...)` が選択する静的な名前空間に属します。同じ表記をグラフ値の名前に使っても曖昧にはなりません。`@UTC` はグラフ参照であり、`Datetime<UTC>` は時刻系を選択します。通常の値式に裸の `UTC` を書いてもグラフ値は参照されず、値が必要な場所で時刻系を使用したものとして拒否されます。

地方民用日時の座標には、日付と明示的な時刻の両方を含める必要があります。日付だけの文字列は拒否されます。真夜中を意図する場合は `T00:00:00` を記述してください。

民用時の `datetime` と科学用途の `epoch<S>` による構築は、意図的に分離されています。`datetime("...")` には明示的な `Z` または数値の UTC オフセットが必要です。`datetime("...", timezone)` はオフセットのない地方民用日時の座標だけを受け入れます。`epoch<S>("...")` は、オフセット・ゾーン・時刻系を含まない暦座標だけを受け入れ、その解釈は静的な `S` で決まります。すべてのリテラル形式はプログラムの検査中に解析されるため、静的な時刻系と実行時の時刻系が食い違うことはありません。タイムゾーンに基づく民用日時の座標は、同梱の tzdb において厳密に 1 つの時点を特定しなければなりません。夏時間移行のギャップにより存在しない時刻と、折り返しにより繰り返される時刻は検査エラーです。ソースで特定の時点を選ぶ必要がある場合は、明示的な数値オフセットを含む 1 引数の `datetime` を使用してください。

名前付き民用タイムゾーンには、引用符で囲んだ文脈依存の `TimezoneLiteral` を使用します。タイムゾーンリテラルは第一級の `String` 値ではありません。検査時に検証し、評価前に不透明な IANA 識別子へ正規化します。IANA データベースは Graphcal ツールチェーンに同梱され、バージョンが固定されています。コンパイル、評価、表示は、ホスト OS の zoneinfo データベースには一切依存しません。無効なタイムゾーンの診断には、同梱の tzdb リリースが表示されます。

日時値は**点とベクトルを区別する**意味論に従います。

| 演算 | 結果 | 備考 |
|-----------|--------|-------|
| `Datetime - Datetime` | `Time` | 両方が同じ時刻系でなければなりません |
| `Datetime + Time` | `Datetime` | 時間幅を加算します |
| `Time + Datetime` | `Datetime` | 時間幅を加算します（交換した形式） |
| `Datetime - Time` | `Datetime` | 時間幅を減算します |
| `Datetime == Datetime` | `Bool` | 等値比較 |
| `Datetime < Datetime` | `Bool` | 順序比較 |

日時値同士の加算、乗算、除算はできず、時間幅の減算の右オペランド（`Time - Datetime`）にも使用できません。

異なる時刻系間の演算は型エラーです。`Datetime<UTC> - Datetime<TT>` はコンパイルできません。先に明示的な時刻系変換関数（`to_utc`、`to_tt` など）を使用してください。

暦の各成分を取り出す関数（`year`、`month`、`day`、`hour`、`minute`、`second`、`weekday`、`day_of_year`）は、値に宣言された `Datetime<S>` の時刻系を使用します。`weekday` は ISO 8601 の番号付けに従い、月曜日を 1、日曜日を 7 とします。`-> "Area/Location"` で適用したタイムゾーンメタデータは表示だけに影響し、これらの計算上のフィールドを変更することはありません。

日時のコンストラクター、変換関数、取り出し関数の全一覧は、[組み込みリファレンス](built-ins.md#datetime-functions)を参照してください。

### 値型（レベル 2） { #value-types-level-2 }

`ValueType` は単一の論理的な値です。プリミティブ、または `type` で宣言した 1 つの公称的な代数的型のインスタンスです。本体を持つすべての `type` 宣言は、1 つ以上のコンストラクターを列挙します。コンストラクターの個数によって「構造体」と「共用体」という別々の型分類が生まれるわけではありません。

コンストラクターのペイロードフィールドは完全な DeclType で宣言します。`Budget(dv: Velocity(min: 0.0 m/s)[Maneuver])` やジェネリックな `Vector(values: D[Fin(N)])` のように、フィールドに値域制約やインデックス軸を持たせることができます。フィールド名は各コンストラクター内で一意でなければなりません。異なるコンストラクターでは、同じフィールド名を独立に使用できます。ValueType に限定されるのは `Type` ジェネリックカインドです。`Velocity[Maneuver]` のようなインデックス付き DeclType は `Type` 引数として渡せないため、`Holder<Velocity[Maneuver]>` は拒否されます。構造化データを軸でパラメーター化するには、ペイロードフィールドに直接インデックスを付けるか、代数的型全体に `Vec3<Velocity, ECI>[Maneuver]` のようにインデックスを付けてください。

#### コンストラクターが 1 つの代数的型 { #algebraic-type-with-one-constructor }

コンストラクターが 1 つの型がレコード形状になるのは、そのコンストラクター名が型名と同じ場合に限ります。これは直接フィールドアクセスを行うための意味論上の要件であり、命名規約ではありません。

```
type Orbit {
    Orbit(sma: Length, ecc: Dimensionless, inc: Angle),
}

type Vec3<D: Dim, Frame: Type> {
    Vec3(x: D, y: D, z: D),
}
```

すべての値が、型と同名の同じコンストラクターと同じペイロード形状を持つため、`@value.field` でフィールドに直接アクセスできます。コンストラクターが 1 つでも、その名前が異なる共用体は有効な代数的型ですが、レコード形状ではないため、`match` で分解する必要があります。

```graphcal
type Box { Make(value: Dimensionless) }
node box: Box = Make(value: 1.0);
node value: Dimensionless = match @box {
    Make(value: value) => value,
};
```

この `Box` では `@box.value` は拒否されます。レコードのフィールドへの直接アクセスを意図する場合は、`type Box { Box(value: ...) }` を使用してください。

#### コンストラクターが複数の代数的型 { #algebraic-type-with-multiple-constructors }

同じ `type` 宣言構文で複数のコンストラクターを列挙できます。各コンストラクターは独自のペイロードを持つか、ペイロードのないユニットコンストラクターになります。

```
type ManeuverKind {
    Impulsive(delta_v: Velocity),
    LowThrust(thrust: Force, duration: Time),
    Coast,
}
```

値構成子のシグネチャ記法では、この宣言は次を導入します。

```text
Impulsive : Velocity -> ManeuverKind
LowThrust : Force × Time -> ManeuverKind
Coast     : ManeuverKind
```

矢印は意味論上の記法です。構築では `Impulsive(delta_v: 3.1 km/s)` のように必ずフィールド名を記述し、コンストラクターは関数値ではありません。

型が複数のコンストラクターを持つ場合、単一のペイロード形状が保証されないため、フィールドアクセスは拒否されます。代わりに `match` で値を分解してください。

#### 再帰的な代数的型 { #recursive-algebraic-types }

ペイロードフィールドは代数的型を直接または相互に参照できるため、有限の再帰的な値は通常の検査済みの値として扱われます。

```graphcal
type List {
    Nil,
    Cons(head: Int, tail: List),
}

node one: List = Cons(head: 1, tail: Nil);
```

型は再帰的ですが、個々の実行時の値は有限のコンストラクターツリーのままです。`graphcal check` と `graphcal eval` はこのような出力を受け入れ、準備済みプロジェクトのパラメーター束縛では、`Cons(head: 1, tail: Cons(head: 2, tail: Nil))` のような完全で有限の値を渡せます。モデル API は、ツリースキーマを無限に展開する代わりに、型付き参照を持つ有限の定義グラフを通じてこれらの型を公開します。再帰的スキーマに対応しない転送形式では、その転送境界で型を明示的に拒否しなければなりません。Tenax スキーマ v2 はそのような転送形式の 1 つです。

### 宣言型（レベル 3） { #declaration-types-level-3 }

DeclType は、必要に応じて制約を付けた ValueType に、任意で 1 つ以上の軸によるインデックスを付けたものです。`param`、`node`、`const node` 宣言、DAG パラメーター、コンストラクターのペイロードフィールドの型注釈が表すものです。

```
param dry_mass: Mass = 1200.0 kg;                         // ValueType
param delta_v: Velocity[Maneuver] = { ... };              // Indexed ValueType
node matrix: Dimensionless[Row, Col] = for r, c { ... };  // Multi-indexed ValueType
type Budget { Budget(dv: Velocity[Maneuver]) }            // Indexed payload field
```

`T[I]` は、ValueType をインデックスラベルから値への全域写像に持ち上げる型コンストラクターです。複数インデックスを持つ `T[I, J]` は、平坦な直積キーの写像です（入れ子ではありません）。軸の順序は重要であり、`T[I, J]` と `T[J, I]` は異なる型です。

## 値域制約 { #domain-constraints }

型式には、有効な値の範囲を宣言する**値域制約**を付けられます。制約は、基本となる型の後に `(min: expr, max: expr)` と記述します。

```
param bus_mass: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;
param thrust: Force(min: 0.01 N) = 0.5 N;           // min only
param efficiency: Dimensionless(max: 1.0) = 0.85;    // max only
param sample_count: Int(min: 1, max: 100) = 10;      // exact Int constraints
param launch: Datetime(
    min: datetime("2025-01-01T00:00:00Z"),
    max: datetime("2025-12-31T23:59:59Z"),
) = datetime("2025-06-01T12:00:00Z");
```

### 構文 { #syntax }

制約句は、基本となる型と、省略可能な `[Index]` 接尾辞の間に置きます。

```
T(min: expr, max: expr)           // both bounds
T(min: expr)                      // lower bound only
T(max: expr)                      // upper bound only
T(min: expr, max: expr)[I]        // constrained indexed type (element-wise)
```

ここで `T` は数量型、`Int`、`Datetime<S>` のいずれかでなければならず、`I` はインデックス軸です。`min` と `max` はともに任意で、片方だけでも両方でも指定できます。境界は、その値を含むコンパイル時定数式です。各境界は `T` が要求する型を持たなければなりません。特に、`Int` の境界は厳密に `Int` であり、日時の境界は対象の `Datetime<S>` と厳密に同じ時刻系でなければなりません。

### サポートする型 { #supported-types }

値域制約は次の型で有効です。

- **数量型**（`Dimensionless` を含む）: `Mass(min: ...)`、`Velocity(max: ...)`、`Dimensionless(min: 0.0, max: 1.0)` など。
- **`Int`**: `Int(min: 1, max: 100)`。境界は `i64` のまま保持され、`Dimensionless` 数量が整数の境界へ暗黙に変換されることはありません。
- **`Datetime<S>`**: `Datetime<TT>(min: epoch<TT>("..."), max: ...)`。裸の `Datetime` は `Datetime<UTC>` なので、その境界にはオフセットを含む `datetime("...Z")` 値を使用します。

値域制約は、`Bool` およびすべての代数的型では、コンストラクターの個数に関係なく**使用できません**。これらの型で制約を使用しようとすると、コンパイルエラーになります。

### インデックス付きの型 { #indexed-types }

インデックス付きの型では、制約は各エントリーに**要素ごと**に適用されます。

```
param delta_v: Velocity(min: 0.0 m/s, max: 10000.0 m/s)[Maneuver] = {
    Maneuver#Departure: 3200.0 m/s,
    Maneuver#Correction: 500.0 m/s,
    Maneuver#Insertion: 1800.0 m/s,
};
```

インデックス付きの値の各エントリーは、制約の境界に照らして独立に検査されます。インデックス付きの `Int` と `Datetime<S>` 値も対象です。

### 日時の制約 { #datetime-constraints }

日時の境界には、制約対象の値に宣言された時刻系を使用します。

```graphcal
const node TT_START: Datetime<TT> = epoch<TT>("2025-01-01T00:00:00");

param observation: Datetime<TT>(
    min: @TT_START,
    max: epoch<TT>("2025-12-31T23:59:59"),
) = epoch<TT>("2025-06-01T12:00:00");
```

`Datetime<TT>` の境界は、それ自体が `Datetime<TT>` でなければなりません。比較可能な物理的時点を表していても UTC の境界は拒否されます。`to_tt(datetime("...Z"))` のような明示的な変換を記述してください。`min` と `max` は境界値を含み、`min > max` はコンパイル時に拒否されます。`-> "Area/Location"` による表示専用のタイムゾーンメタデータは、比較や制約検査に影響しません。

### コンストラクターのペイロードフィールド制約 { #constructor-payload-field-constraints }

コンストラクターのペイロードフィールド型にも制約を注釈できます。

```
type SatelliteSpec {
    SatelliteSpec(
        mass: Mass(min: 100.0 kg, max: 2000.0 kg),
        altitude: Length(min: 200.0 km),
        commissioned: Datetime(
            min: datetime("2025-01-01T00:00:00Z"),
        ),
    ),
}

pub type ManeuverResult {
    Burn(dv: Velocity(min: 0.0 m/s, max: 10.0 km/s), duration: Time(min: 0.0 s)),
    Coast,
}
```

フィールド制約は、その型を定義するモジュールで解決されます。そのため、境界が定義元モジュールの独自の単位や次元を使用していても、エクスポートされた型をインポートするだけで十分です。利用側で、これらの実装上の依存項目を重複してインポートする必要はありません。

型にジェネリックな `Dim` または `Nat` パラメーターを含むフィールドは、そのフィールドを所有する型の具体的な適用ごとに、コンパイル時の検証義務を生じさせます。コンパイラーは、明示的な引数、入れ子の引数、デフォルト引数を代入してから、境界の型と、境界式内の静的な `Fin` キーやインデックスを検査します。

```graphcal
type Box<D: Dim> { Box(value: D(min: 0.0 m)) }

node length: Box<Length> = Box<Length>(value: 1.0 m); // valid
node time: Box<Time> = Box<Time>(value: 1.0 s);       // compile error
```

同じ検査は、インポートした型やモデルポートのスキーマを通じても適用されます。記号的な `key(Fin(N), position)` がジェネリック定義で受け入れられるのは、すべての具体的な使用箇所で `0 <= position < N` が証明される場合に限ります。`Fin(0)` は常に拒否されます。

フィールド制約は、各 `Ctor(field: ...)` 呼び出しの**構築時**に適用されます。

- 値がコンストラクター呼び出しである `const node` では、違反はコンパイル時に `DomainViolation` として検出されます。
- 実行時に値を構築する `param` または `node` では、違反はコンストラクターとフィールドに対応付けたノードごとの `EvalFailed` エラーとして報告されます（例: `field SatelliteSpec.mass above maximum (2000 kg)`）。

### ジェネリック型引数 { #generic-type-arguments }

ジェネリック型引数には値域制約を**指定できません**。型消去後に制約を強制する場所がなく、意味論が曖昧になるためです。この規則は、インライン DAG 内で宣言した型を含め、入れ子の型引数とジェネリックパラメーターのデフォルト値にも適用されます。代わりに、コンストラクターのペイロードフィールドに制約を付けてください。

```
// REJECTED at compile time:
pub type Vec3<D: Dim> { Vec3(x: D, y: D, z: D) }
param p: Vec3<Length(min: 0.0 m)> = ...;

// Use a non-generic field constraint instead:
pub type SignedLength { SignedLength(value: Length(min: 0.0 m)) }
```

### 実行時検査 { #runtime-checking }

`param` と `node` 宣言の値域制約は、評価後の**実行時**に検査されます。違反するとノードごとのエラーが発生し、下流ノードには `DependencyFailed` が伝わります。制約は、インデックス付きの値には要素ごとに、制約付きペイロードフィールドには構築時に適用されます。`const node` 宣言の制約と、`const` 内で構築されるコンストラクターのペイロードフィールドの制約は、値が静的に既知なのでコンパイル時に検査されます。

制約対象の宣言が `param`、`node`、`const node` のどれであっても、すべての境界式はコンパイル時定数です。境界は `const node` 値を読み取れますが、パラメーターや実行時ノードを読み取ることはできません。

### コンパイル時の検証 { #compile-time-validation }

制約がどこにあるか（トップレベル宣言かコンストラクターのペイロードフィールドか）に関係なく、次の問題は常にコンパイル時に検出されます。

- **無効な対象型**: サポートされない型への制約（例: `Bool(min: 0)`）
- **無効なキー**: 未知の制約キー（例: `Mass(step: 10)`。有効なのは `min` と `max` だけです）
- **最小値が最大値を超える**: 両方の境界が指定され、`min > max` の場合
- **次元／型の不一致**: 数量の境界が誤った次元を持つ場合、または `Int` の境界が厳密に `Int` ではない場合
- **日時の時刻系の不一致**: 境界が対象の `Datetime<S>` と厳密に一致しない場合。異なる時刻系の境界には明示的な変換が必要です
- **非定数の境界**: 境界がパラメーターまたは実行時ノードを参照する場合
- **ジェネリック型引数の制約**: `Vec3<Length(min: 0.0 m)>` のような `TypeApplication` 引数に置かれた制約

### ユースケース { #use-cases }

値域制約は、次の用途に役立ちます。

- **パラメータースイープ／サンプリング**: 設計空間の探索に有効な範囲を宣言します
- **入力検証**: 明らかに誤ったパラメーター値がグラフ内に伝播する前に検出します
- **ドキュメント化**: 有効な範囲を型注釈に明示し、LSP のホバーで確認できるようにします

## インデックスとインデックス付きの型 { #indexes-and-indexed-types }

インデックスは、`T[I]` で使用できる有限で順序付きのコレクション軸を宣言します。Graphcal には、名前付きインデックス、座標インデックス、構造的な有限インデックスがあります。各種類には固有の**インデックス順序**があります。名前付きインデックスはラベルの宣言順、座標インデックスは `start` から `end` に向かう座標列の順、`Fin(N)` は位置の昇順です。インデックス付きの値が独自の順序を持つことはありません。すべての構築はインデックス順に正規化され、順序に依存する利用箇所（`scan`、表示出力、外部関数のバッファー）はその順序に従います。複数軸の外部関数バッファーは、軸ごとの順序の辞書式の行優先直積を使用し、すべての軸の長さを明示的に保持します。

### 名前付きインデックス { #named-index }

名前付きインデックスは、コレクション軸として使用できる有限で順序付きのラベル集合を宣言します。`index` キーワードは次を宣言します。

1. **軸マーカー**: `Maneuver` を `T[Maneuver]` で使用して、インデックス付きの型を作成できます。
2. 順序付きの閉じた**インデックスラベル**集合: `Maneuver#Departure` はその軸上の 1 つの位置を識別します。ラベルの宣言順が軸のインデックス順序になるため、`index` 宣言のラベルの順序を変更すると、`scan` のような順序に依存する利用箇所では意味論上の変更になります。

```
index Maneuver = { Departure, Correction, Insertion };
```

名前付きインデックスのラベルは修飾構文（`Maneuver#Departure`）を使い、裸の構文（`Nominal`）を使う代数的型のコンストラクターと区別されます。これは実際の意味論上の違いを反映しています。ラベルはコレクション軸内の位置を識別し、コンストラクターは代数的型の値を構成します。

修飾付きラベルは**それ自体で型が定まる定数**です。式としての `Maneuver#Departure` は `Key<Maneuver>` 型を持つ通常の値であり、ノードに格納し、`==` で比較し、DAG パラメーターを通じて渡し、コンストラクターのペイロードで使用できます（[インデックスキー](indexes.md#index-keys)を参照）。コンストラクター名が式とパターンの両方になるのと同様に、ラベルの表記は、パターンに類する位置では引き続き*構文上のセレクター*としても使われます。

- インデックス付きの型の軸では、インデックス自体を名前で指定します: `Velocity[Maneuver]`
- 要素アクセス: `@delta_v[Maneuver#Departure]`（ラベルを定数キーとして使用）
- マップとテーブルのキー: `{ Maneuver#Departure: 2.46 km/s, ... }`（セレクター）
- `for` 束縛と、そのループ変数を介したインデックスアクセス: `for m: Maneuver { @delta_v[m] }`
- `Key<Maneuver>` 型の照合対象に対する `match` パターン: `match m { Maneuver#Departure => ..., ... }`

ユニットコンストラクターだけを持つ代数的型（例: `type Foo { A, B }`）が、自動的にインデックスになることはありません。`index` キーワードは、列挙を `T[I]` で使用できるものとして明示的に指定し、マーカー型を誤ってコレクション軸に使用することを防ぎます。

### 座標インデックス { #coordinate-index }

座標インデックスは、1 つの次元に属する数量値の有限列です。

```
index TimeStep = range(0.0 s, 100.0 s, step: 0.1 s);
index Samples = linspace(0.0 s, 100.0 s, points: 1001);
```

`range` では正確な刻み幅が基準になり、`linspace` では正確な点数が基準になります。引数は静的かつ有限です。座標のループ変数は `Key<C>` 型のキーであり、そのままインデックスアクセスや `==` による比較に使用できます。一方、算術演算や順序比較に使う数量座標は、`coord(t)` で明示的に取り出します。

### 構造的な有限インデックス { #structural-finite-index }

`Fin(N)` は、無名の位置軸を明示的に構築します。

```
param vector: Dimensionless[Fin(3)] = for i: Fin(3) { 0.0 };
```

そのループ変数は `Key<Fin(N)>` 型のキーです。直接インデックスアクセスでき、境界を追跡する加算演算（静的な Nat `c` に対する `i + c : Key<Fin(N + c)>`）をサポートし、`to_int(i)` を通じて整数位置を明示的に取り出せます。Nat `N` と Index `Fin(N)` は異なる種別のままなので、裸の `D[3]` は無効です。

### インデックスの機能 { #index-capabilities }

| 機能 | 名前付き（`Maneuver`） | 座標（`TimeStep`） | 有限（`Fin(3)`） |
|-----------|----------------------|--------------------------|-------------------|
| ループ変数の型 | `Key<Maneuver>` | `Key<TimeStep>` | `Key<Fin(3)>` |
| インデックスアクセス: `@x[i]` | はい | はい | はい |
| 明示的なマップキー | はい | いいえ | いいえ |
| 等値比較 | はい（同じ軸） | はい（同じ軸） | はい（同じ軸） |
| パターンマッチング | はい（修飾付きラベル） | いいえ | いいえ |
| 算術演算 | いいえ | `coord(t)` による数量演算 | 加算 `i + c`（境界を追跡）。整数は `to_int(i)` で取得 |
| DAG パラメーターへの受け渡し | はい（キーとして） | はい（キーとして） | はい（キーとして） |

すべてのループ変数は、その軸のキーです。すべてのキーは、同じ軸でのインデックスアクセスと等値比較に使用でき、いずれにも順序はありません。それ以外にキーが提供する機能は軸の種類ごとに異なります。名前付きキーは網羅的な `match` に使えますが、それ以外では不透明です。座標キーは `coord(t)` を通じて数量を公開し、`Fin` キーは `to_int(i)` を通じて位置を公開するとともに、上記の加算演算を備えます。[インデックスキー](indexes.md#index-keys)を参照してください。

### インデックス付きの値の構築 { #construction-of-indexed-values }

**マップリテラル**（全域的。すべてのラベルが必要です）:

```
param delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.05 km/s,
    Maneuver#Insertion: 1.48 km/s,
}
```

**複数軸のマップリテラル**（全域的。すべてのラベルタプルが必要です）:

```
param delta_v_budget: Velocity[Phase, Maneuver] = {
    (Phase#Launch, Maneuver#Departure): 2.46 km/s,
    (Phase#Launch, Maneuver#Correction): 0.0 m/s,
    (Phase#Launch, Maneuver#Insertion): 0.0 m/s,
    (Phase#Cruise, Maneuver#Departure): 0.0 m/s,
    (Phase#Cruise, Maneuver#Correction): 0.05 km/s,
    (Phase#Cruise, Maneuver#Insertion): 0.0 m/s,
    (Phase#Arrival, Maneuver#Departure): 0.0 m/s,
    (Phase#Arrival, Maneuver#Correction): 0.0 m/s,
    (Phase#Arrival, Maneuver#Insertion): 1.48 km/s,
}
```

単一軸のマップリテラルでは裸のキー（`Maneuver#Departure: ...`）を使い、複数軸のマップリテラルではタプルキー（`(Phase#Launch, Maneuver#Departure): ...`）を使います。

マップ（および `table`）リテラルのエントリー順序は重要ではありません。構築した値はインデックス順に正規化されるため、`Maneuver#Insertion` を最初に列挙したリテラルも、宣言順に記述したものと同一です。

**`for` 内包表記**（ラベルごとに 1 つの値）:

```
node fuel: Mass[Maneuver] = for m: Maneuver {
    @dry_mass * (exp(@delta_v[m] / @v_exhaust) - 1.0)
}
```

**複数軸の `for`**（ラベルタプルごとに 1 つの値）:

```
node matrix: Dimensionless[Row, Col] = for r: Row, c: Col {
    @A[r, c] + @B[r, c]
}
```

### インデックス付きの値の利用 { #consumption-of-indexed-values }

**インデックスアクセス** — すべてのインデックスラベルを指定して、単一の要素を取り出します。

```
@delta_v[Maneuver#Departure]                // Velocity[Maneuver] -> Velocity
@matrix[Row#R1, Col#C2]                    // Dimensionless[Row, Col] -> Dimensionless
```

部分的なインデックスアクセスはできず、すべての軸を指定する必要があります。1 つの軸に沿った「スライス」を取り出すには、明示的な `for` を使用してください。

```
node row1: Dimensionless[Col] = for c: Col { @matrix[Row#R1, c] }
```

**集約** — 厳密に 1 軸だけを縮約します。複数軸の直接集約は `D021` で拒否されます。まず明示的な `for` で 1 つの軸を選択してください。

```
sum(for m: Maneuver { @fuel[m] })        // Mass[Maneuver] -> Mass
product(for i: Fin(3) { @edge[i] })       // Length[Fin(3)] -> Volume
rss(for m: Maneuver { @sigma[m] })        // Mass[Maneuver] -> Mass
maximum(for m: Maneuver { @delta_v[m] }) // Velocity[Maneuver] -> Velocity
count(for m: Maneuver { @fuel[m] })      // Mass[Maneuver] -> Int
```

数量の縮約には数量要素が必要です。`product` は、要素が次元を持つ場合、その軸の要素数が結果の次元指数になるため、具体的な軸の要素数を必要とします。`rss` は要素の次元を保持します。`count` はインデックスのない任意の要素型を受け入れ、要素数を `Int` として保持します。

**スキャン** — 順序に従う累積です。

```
scan(
    for m: Maneuver { @delta_v[m] },
    0.0 m/s,
    |acc, item| acc + item,
)   // Velocity[Maneuver] -> Velocity[Maneuver]
```

`scan` はインデックス順に畳み込みます。`Maneuver` のような名前付き軸では、ラベルの宣言順です。入力元は厳密に 1 軸を持ち、複数軸の値から軸を暗黙に選択することはありません。各出力要素は、その位置自身を含む、その位置までの累積結果です。累積値は独立に固定の状態軸を持つことができ、結果では入力元の軸の後に配置されます。

### 暗黙のブロードキャストはありません { #no-implicit-broadcasting }

算術演算子と比較演算子には、インデックスのないオペランドが必要です。インデックス付きの値を要素ごとに処理するには、明示的な `for` を使用しなければなりません。比較演算子 `==`、`!=`、`<`、`<=`、`>`、`>=` は、インデックス付きのオペランドを `D019` で拒否します。これは意図的な安全性のための判断です。

```gcl
// ERROR: neither arithmetic nor comparison broadcasts indexed values
node bad_sum: Velocity[Maneuver] = @delta_v + @extra_dv;
node bad_limit: Bool[Maneuver] = @delta_v < 3.0 km/s;

// CORRECT: explicit element-wise operations
node combined: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] + @extra_dv[m]
};
node below_limit: Bool[Maneuver] = for m: Maneuver {
    @delta_v[m] < 3.0 km/s
};
```

これにより、NumPy や Excel でよく見られる、暗黙のブロードキャストに起因するバグを防ぎます。そうしないと、選択した軸や要素の組み合わせを明示しないまま、形状の不一致が解消されてしまう可能性があります。

## 型変換 { #type-conversions }

Graphcal には**暗黙の型変換はありません**。明示的な変換関数を使用する必要があります。

| 関数 | 変換元 | 変換先 | 例 |
|----------|------|----|---------|
| `to_float(x)` | `Int` | `Dimensionless` | `to_float(42)` は `42.0` になります |
| `to_int(x)` | `Dimensionless` | `Int` | `to_int(3.0)` は `3` になります。`to_int(3.7)` はエラーになります |
| `to_int(k)` | `Key(Fin(N))` | `Int` | `Fin` キーの位置 |
| `coord(k)` | `Key(C)` | `Quantity(D)` | 座標軸キーの座標 |
| `to_utc(x)` | `Datetime(S)` | `Datetime(UTC)` | 時刻系の変換 |
| `to_tai(x)` | `Datetime(S)` | `Datetime(TAI)` | 時刻系の変換 |
| `to_tt(x)` | `Datetime(S)` | `Datetime(TT)` | 時刻系の変換 |

既存の `to_float` という名前は、浮動小数点表現への変換を表しており、ソースレベルの `Float` 型を指すものではありません。`to_int` は、`Int` の範囲内にある、有限かつ整数値の binary64 入力を要求します。丸め方針を選択することはありません。`to_int(trunc(x))`、`to_int(floor(x))`、`to_int(ceil(x))`、`to_int(round(x))` を使って、その方針を明示してください。キーでは `to_int` と `coord` だけが内容を取り出す操作です。`to_int` は `Fin` キーだけを、`coord` は座標軸キーだけを受け入れます。名前付きキーは不透明であり、数量からキーへの逆変換はありません（明示的なキー構成子を使用してください）。時刻系変換関数（`to_utc`、`to_tai`、`to_tt`、`to_tdb`、`to_et`、`to_gpst`、`to_gst`、`to_bdt`、`to_qzsst`）は、物理的な時点を変えずに時刻系間を変換します。

## 次元代数 { #dimension-algebra }

次元は、基本次元上の代数を構成するコンパイル時の型です。次元と単位の宣言に関する完全なリファレンスは、[次元と単位](dimensions-and-units.md)を参照してください。この節では、コンパイラーが強制する代数規則を説明します。

### 表現 { #representation }

内部的には、次元は基本次元を有理数乗したものの積です。

$$
D = L^{a_1} \cdot T^{a_2} \cdot M^{a_3} \cdot \ldots
$$

ここで各指数 $a_i$ は有理数です（既約分数として格納します）。`Dimensionless` は、すべての指数がゼロの場合です。

### 算術規則 { #arithmetic-rules }

コンパイラーは、各算術演算の結果の次元を決定します。

| 演算 | 次元規則 | 制約 |
|-----------|---------------|------------|
| `a + b` | オペランドと同じ | `dim(a)` と `dim(b)` が等しい必要があります |
| `a - b` | オペランドと同じ | `dim(a)` と `dim(b)` が等しい必要があります |
| `a * b` | 積 | `dim(a) * dim(b)` — 指数を加算します |
| `a / b` | 商 | `dim(a) / dim(b)` — 指数を減算します |
| `a ^ n` | べき乗 | `dim(a) ^ n` — 次元を持つ底には、正確な整数／有理数の構文が必要です |

例:

- `Length * Length` = `Length^2`
- `Length / Time` = `Velocity`（プレリュードの組立次元）
- `Length / Length` = `Dimensionless`
- `(Mass * Length / Time^2) * (Length / Time)` = `Mass * Length^2 / Time^3`

### 等価性 { #equivalence }

2 つの次元式が等価であるのは、同じ標準形（基本次元の指数の集合が同じ）に簡約される場合に限ります。名前付き次元は透過的です。`Velocity` が `Length / Time` として定義されていれば、`Velocity` と `Length / Time` は同じ型です。

### 組み込み関数の次元規則 { #built-in-function-dimension-rules }

組み込みの数学関数には、それぞれ特定の次元制約があります。

| 関数 | 引数の次元 | 結果の次元 |
|----------|--------------------|------------------|
| `sqrt(x)` | 任意の `D` | `D^(1/2)`（指数を半分にします） |
| `sin(x)`、`cos(x)`、`tan(x)` | `Angle` | `Dimensionless` |
| `asin(x)`、`acos(x)` | `Dimensionless` | `Angle` |
| `atan2(y, x)` | 両方とも同じ `D` | `Angle` |
| `exp(x)` | `Dimensionless` または `Complex<Dimensionless>` | 入力と同じ実数／複素数の種別で、無次元 |
| `ln(x)`、`log10(x)` | `Dimensionless` | `Dimensionless` |
| `abs(x)` | 任意の実数 `D` または `Complex<D>` | 実数 `D` |
| `least(a, b)`、`greatest(a, b)` | 両方とも同じ `D` | `D` |
| `floor(x)`、`ceil(x)`、`round(x)`、`trunc(x)` | `Dimensionless` | `Dimensionless` |

丸め関数は `Dimensionless` 引数だけを受け入れます。次元を持つ数量にとって「整数に丸める」ことには、単位に依存しない意味がないためです。数量を丸めるには、たとえば `floor(@x / 1.0 cm) * 1.0 cm` のように、明示的な刻み幅で除算してください。[組み込みリファレンス](built-ins.md#math-functions)を参照してください。

## 単位変換 { #unit-conversion }

`->` 演算子は、同じ次元の単位間を変換します。結合の優先順位は最も低くなっています。複素数値では、1 つの表示単位で両成分をスケーリングします。

```
node speed_kmh: Velocity = @speed -> km/h;
node displacement_cm: Complex<Length> = @displacement -> cm;
```

型キャスト演算子はありません。値のファントム型パラメーターを変更する場合（たとえば参照座標系のラベルを付け替える場合）は、新しいインスタンスを構築し、各フィールドを明示的に代入してください。

```
node pos_body: Vec3<Length, Body> = Vec3<Length, Body>(
    x: @pos_eci.x,
    y: @pos_eci.y,
    z: @pos_eci.z,
);
```

この冗長さは意図的です。すべてのラベルの付け替えを、呼び出し箇所で確認できる、フィールドごとの意図的な操作にします。

## 式の型付け規則 { #typing-rules-for-expressions }

この節では、各式形式の型と、コンパイラーが強制する制約を列挙します。

### リテラル { #literals }

| 式 | 型 | 備考 |
|-----------|------|-------|
| `42` | `Int` | 小数点や指数を含みません。範囲は `-9223372036854775808` から `9223372036854775807` です |
| `3.14` | `Dimensionless` | 単位のない浮動小数点リテラル |
| `400.0 km` | 単位の次元（`Length`） | 単位付きの浮動小数点リテラル。整数リテラルには単位を付けられません |
| `true`、`false` | `Bool` | |

### 参照 { #references }

| 式 | 型 |
|-----------|------|
| `@name` | パラメーター／ノード／定数ノード `name` の宣言型 |
| `BUILTIN_NAME` | 組み込み定数（`PI`、`E`、`TAU` など）の型 |
| `local_var` | ループ変数または match 束縛の型 |

### 算術演算子 { #arithmetic-operators }

| 式 | 結果の型 | 制約 |
|-----------|-------------|------------|
| `a + b` | `a` の型 | 数値の種別と次元が同じ |
| `a - b` | `a` の型 | 数値の種別と次元が同じ |
| `a * b` | `dim(a) * dim(b)` に持ち上げた数値の種別 | 実数×実数、実数×複素数、複素数×実数、複素数×複素数をサポートします |
| `a / b` | `dim(a) / dim(b)` に持ち上げた数値の種別 | 実数÷実数、実数÷複素数、複素数÷実数、複素数÷複素数をサポートします |
| `a % b` | `Int` | 両オペランドとも `Int` でなければなりません。数量の剰余は定義されていません |
| `a ^ n` | `dim(a) ^ n` | 実数量または整数のみ。`n` は無次元であり、次元を持つ底には正確な整数または丸括弧で囲んだ有理数が必要です |
| `-a` | `a` の型と次元 | 実数量、複素数量、または整数 |

複素数の加算と減算には、同じ `D` を持つ 2 つの `Complex<D>` オペランドが必要です。Graphcal が実数量を暗黙に昇格することはありません。昇格を意図する場合は `to_complex(x)` を使用してください。複素数の剰余とべき乗はサポートしていません。

底が `Dimensionless` の実数である場合、`n` は実行時の `Dimensionless` 数量でもかまいません。それ以外の次元を持つ数量では、`n` は `2` や `-2` のような整数、または `(3/2)` のような丸括弧で囲んだ有理数として正確に記述しなければなりません。小数構文では次元指数を決定できません。`0.25` の代わりに `(1/4)`、`2.0` の代わりに `2` を使用してください。コンパイラーは、その有理数を binary64 から復元するのではなく、検査と評価を通じて正確に保持します。

### 比較演算子と論理演算子 { #comparison-and-logical-operators }

| 式 | 結果の型 | 制約 |
|-----------|-------------|------------|
| `a == b`、`a != b` | `Bool` | `a` と `b` はインデックスなしで同じ型でなければなりません。複素数の等値比較は両成分を厳密に比較します。キーの等値比較には同じ軸が必要です |
| `a < b`、`a > b`、`a <= b`、`a >= b` | `Bool` | オペランドはインデックスなしで、両方が `Int`、同じ次元の実数量、または同じ時刻系の日時でなければなりません。複素数値とキーには順序がありません。キーの内容を順序比較するには `coord(t)` / `to_int(i)` で取り出してください |
| `a && b`、`a \|\| b` | `Bool` | `a` と `b` は `Bool` でなければなりません |
| `!a` | `Bool` | `a` は `Bool` でなければなりません |

比較は連鎖できません。`a < b < c` は解析エラーです。`a < b && b < c` と記述してください。

### 条件式 { #conditional }

```
if condition { then_expr } else { else_expr }
```

- `condition` は `Bool` でなければなりません。
- `then_expr` と `else_expr` は同じ型でなければなりません。
- 結果の型は分岐の型です。

### 関数呼び出し { #function-call }

```
function_name(arg1, arg2, ...)
```

- 引数は位置に基づいて関数のパラメーター型と照合されます。
- 組み込みまたは外部シグネチャ内のジェネリックな次元とインデックスは、通常の値引数から推論されます。
- `epoch<S>(civil_literal)` は、明示的な静的時刻系引数を 1 つ受け取ります。`sqrt<3>(x)` のような、それ以外の空でない関数ジェネリック引数は、暗黙に無視されず拒否されます。
- 結果の型は、ジェネリック置換後の関数の戻り値型です。

### フィールドアクセス { #field-access }

```
expr.field_name
```

- `expr` はレコード形状の代数的型でなければなりません。すなわち、型と同名でペイロードフィールドを持つコンストラクターが厳密に 1 つ必要です。
- `field_name` は、そのコンストラクターのペイロードフィールドでなければなりません。
- 結果の型は、ジェネリックパラメーターを置換した、宣言されたフィールド型です。

### インデックスアクセス { #index-access }

```
expr[Index#Variant]        // access a specific element
expr[loop_var]              // access with a for-binding variable
expr[Index1#V1, Index2#V2] // multi-dimensional access
```

- `expr` はインデックス付きの型 `T[I]`（多次元の場合は `T[I1, I2]`）でなければなりません。
- すべての軸を指定する必要があります（部分的なインデックスアクセスはできません）。
- 各引数は、その軸位置に対応する `Key<I>` 項です。修飾付きラベル（定数キーの表記）、ループ変数、または `@x[argmax(@x)]` のような計算された任意のキーを使用できます。軸の同一性が基準になります。`Key<Fin(N)>` は `N <= M` のときに `Fin(M)` の位置へ拡大できます。それ以外の軸をまたぐアクセスはありません。
- `Fin` 軸の、静的に検証済みの定数 `Int` 位置（`@x[2]`）は引き続き有効です。実行時の `Int` や数量が暗黙にインデックスになることはありません（`fin_key` または座標検索を使用してください）。
- 結果の型は要素型 `T` です。

### 代数的な値の構築 { #algebraic-value-construction }

```
ConstructorName(field1: expr1, field2: expr2)
ConstructorName<Arg1, Arg2>(field1: expr1, field2: expr2)
ConstructorName                                   // unit constructor
```

- コンストラクターによって、それを包含する 1 つの代数的な結果型が決まります。コンストラクターが別の型を導入することはありません。
- 各フィールド式は、そのペイロードフィールドの宣言型と一致しなければなりません。
- すべてのペイロードフィールドは `field: expr` と記述する必要があります。短縮形の `field` はサポートしていません。
- グラフノードを渡す場合は、明示的な `field: @node_name` を使用してください。
- ジェネリック引数は、包含する代数的型のパラメーターを具体化します。

### インデックスラベル { #index-label }

```
IndexName#VariantName
```

- 名前付きインデックスの特定のラベルを参照します。
- 式としては、`Key<IndexName>` 型の、それ自体で型が定まる定数です。他の値と同様に格納、比較、受け渡しができます。
- パターンに類する位置、すなわちマップ／テーブルのキー、`#[expected_fail(...)]` のキー、include のインデックス束縛、`match` パターンでは、同じ表記が構文上のセレクターになります。これは、コンストラクター名が式とパターンの両方になることに対応します。

### マッチ式 { #match-expression }

```
match scrutinee {
    VariantA(field1: field1, field2: binding) => expr_a,
    VariantB => expr_b,
}
```

- `scrutinee` は代数的型の値、または具体的な名前付き軸 `X` の `Key<X>` 値（ループ変数、格納されたキー、`argmax`/`argmin` の結果など）でなければなりません。座標軸、`Fin` 軸、必須軸のキーは照合できません。
- `match` は、閉じた有限の選択肢に対する網羅的な場合分けのためのものです。通常の真偽値の述語や比較には `if` を使用してください。
- すべてのメンバー／ラベルを網羅する必要があります（網羅性検査）。
- 照合対象が代数的型の場合、アームはコンストラクターパターン（裸の名前またはモジュール修飾付き）を使用し、`field: variable` または `field: _` でペイロードフィールドを明示的に束縛できます。
- 照合対象が名前付き軸のキーの場合、アームは修飾付きインデックスラベルパターン（`Index#Label` または `module::Index#Label`）を使用し、フィールドは束縛できません。
- すべてのアームの式は同じ型でなければなりません。
- 結果の型は、アームに共通する型です。

### マップリテラル { #map-literal }

```
{ Index#Variant1: expr1, module::Index#Variant2: expr2, ... }
```

- インデックスのすべてのバリアントを網羅する必要があります。
- すべての値式は同じ型 `T` でなければなりません。
- 結果の型は `T[Index]` です。インデックスをインポートした場合、その所有者は単なる末端の表記ではなく、解決されたモジュール項目です。

複数軸のマップリテラルにはタプルキーを使用してください。

```
{ (I1#V1, I2#V2): expr1, (I1#V1, I2#V3): expr2, ... }
```

- すべてのラベルタプルが必要です。
- 結果の型は `T[I1, I2]` です。

### for 内包表記 { #for-comprehension }

```
for var: IndexName { body_expr }
for v1: Index1, v2: Index2 { body_expr }
```

- `var` はインデックスの各要素に順番に束縛され、ノードごとの `Key<IndexName>` 型の定数キーになります。
- 名前付きインデックスでは、`Key<X>` ループ変数はインデックスアクセス（`@x[var]`）、他の `Key<X>` 値との `==` 比較、網羅的な `match` に使用できます。
- 座標インデックスでは、`Key<C>` ループ変数はインデックスアクセスと比較に使用できます。数量座標は `coord(var)` で取り出します。
- `Fin(N)` では、`Key<Fin(N)>` ループ変数はインデックスアクセス（加算による `var + c` アクセスを含む）と比較に使用でき、`to_int(var)` で整数位置を取り出せます。
- `body_expr` は束縛ごとに評価され、その型は `T` です。
- 結果の型は `T[IndexName]`（複数の束縛では `T[Index1, Index2]`）です。

### スキャン { #scan }

```
scan(source, init, |acc, item| body)
```

シグネチャ記法では、`scan : T[I] × U[J̄] × (U[J̄] × T -> U[J̄]) -> U[I, J̄]` です。

- `source` は厳密に 1 軸を持ち、型は `T[I]` でなければなりません。`scan` は軸を暗黙に選択しないため、複数軸の入力元は拒否されます。
- `init` の累積値型は `U[J̄]` であり、`J̄` は空、1 軸、または複数の固定軸です。
- `acc` は型 `U[J̄]` に、`item` は型 `T` に束縛されます。
- `body` は、同じ軸を同じ順序で含む、厳密に `U[J̄]` を返さなければなりません。
- 結果の型は `U[I, J̄]` です。入力元の軸が累積値の軸の先頭に追加されます。

たとえば、スカラー入力 `Dimensionless[Step]` を初期ベクトル `Dimensionless[Element]` でスキャンすると、`Dimensionless[Step, Element]` が得られます。各反復では、次のベクトルを構築する前に、前のベクトル全体を読み取ります。

`|acc, item| body` は特殊構文であり、関数値ではありません。

### アンフォールド { #unfold }

```
unfold(index, init, |prev_state, prev_i, i| body)
```

次元 `D` の座標インデックス `I` に対して、シグネチャ記法では `unfold : I × U[J̄] × (U[J̄] × Key(I) × Key(I) -> U[J̄]) -> U[I, J̄]` です。

- `index` は座標インデックス `I` への明示的な参照であり、値式ではありません。
- `init` は状態型 `U[J̄]` を持ち、最初の座標での結果になります。`J̄` は空、1 軸、または複数の固定軸です。
- `prev_state` は、前の状態全体 `U[J̄]` に束縛されます。
- `prev_i` と `i` は、連続する要素キー `Key<I>` に束縛されます。それらが位置する座標の数量は、`coord(prev_i)` と `coord(i)` で取り出します。
- `body` は、同じ軸を同じ順序で含む、厳密に `U[J̄]` を返さなければなりません。
- 結果の型は `U[I, J̄]` です。座標軸が状態軸の先頭に追加されます。たとえば、ベクトル状態 `U[Element]` からは `U[Time, Element]`、行列状態 `U[Row, Column]` からは `U[Time, Row, Column]` が得られます。

座標 `i₀, i₁, …` に対して、`result[i₀] = init` となり、それ以降の各値は `body(result[iₖ₋₁], iₖ₋₁, iₖ)` です。本体が次の状態を構築している間、前の状態全体は固定されるため、成分の反復順序は意味論に影響しません。`|prev_state, prev_i, i| body` 形式は特殊構文であり、関数値ではありません。`prev_state` を直接使用してください。定義中の宣言を明示的に参照すると、その参照のインデックスに使用する座標に関係なく、通常の依存サイクルになります。

## DAG ブロック { #dag-blocks }

DAG ブロックは、名前付きの再利用可能なサブ DAG です。値ではなく宣言です。型システムに関数型はありません。

DAG ブロックのパラメーターとノードには、トップレベル宣言と同じ DeclType（ValueType またはインデックス付きの型）を使用します。

```
dag hohmann_transfer {
    param gm: Length^3 / Time^2;
    param r1: Length;
    param r2: Length;
    node dv1: Velocity = ...;
    node total_dv: Velocity = @dv1 + ...;
}
```

DAG ブロックは `include` でインスタンス化し、そのノードを包含する計算グラフに埋め込みます。

```
include hohmann_transfer(
    gm: @gm_earth,
    r1: @r_earth + @parking_alt,
    r2: @r_earth + @target_alt,
)::{ total_dv };
```

## ジェネリクス { #generics }

代数的型は、4 つの型レベルの領域に属する値でパラメーター化できます。ジェネリックパラメーターの注釈は、その**カインド**です。有効な引数と、宣言内でのパラメーターの使用方法を決定します。

### ジェネリックパラメーターのカインド { #generic-parameter-kinds }

| カインド | 構文 | パラメーターが動く領域 | 有効な用途 |
|------|--------|-----------------------|------------|
| `Dim` | `<D: Dim>` | `Length` や `Length / Time` などの任意の次元 | 実数量のフィールド、`Complex<D>`、`D^2` のような次元式 |
| `Type` | `<T: Type>` | `Bool`、`Length`、`Vec3<Length, Eci>` などの任意の `ValueType` | ペイロードフィールドの型、またはどのペイロードにも使われない場合はファントム／タグパラメーター |
| `Index` | `<I: Index>` | 有限で順序付きのインデックス軸 | `D[I]` のようなインデックス付きの型の軸、またはキー型 `Key<I>` |
| `Nat` | `<N: Nat>` | 型レベルのサイズ演算で使用する非負の自然数 | 空でない軸の規則に従う、`D[Fin(N)]` のような有限の要素数 |

これらのカインド自体は `ValueType` ではありません。たとえば、`Index` はコレクション軸の領域を表します。軸が項レベルに現れるのは、その反映型 `Key<I>` を通じてだけであり、その値は軸の要素キーです。同様に、`Type` は `ValueType` の領域を表し、裸のインデックス軸と、`Velocity[Maneuver]` のようなインデックス付き `DeclType` の両方を除外します。

`Type` パラメーターは本質的にファントムなのではありません。コンストラクターのペイロードで使用せず、公称的な区別のために宣言が保持する場合にのみファントムになります。たとえば `Vec3<D, Frame>` では、`D` がフィールド型を決定し、`Frame` はファントムパラメーターです。

### ジェネリック引数とデフォルト値 { #generic-arguments-and-defaults }

型注釈とコンストラクターは、同じジェネリック引数構文を使用します。山括弧には、常に少なくとも 1 つの引数またはパラメーターが必要です。引数を使用しない場合や非ジェネリック型を宣言する場合は、山括弧自体を省略してください。`Vec3<>` や `type Marker<> { Marker }` のような空の形式は無効です。各引数は、宣言された `Dim`、`Index`、`Nat`、`Type` カインドに位置ごとに照らして検査されます。表記や識別子の大文字・小文字がカインドを決めることはありません。通常の関数はこれらの引数を宣言も受理もしないため、`sqrt<3>(x)` のような空でない適用は、暗黙に無視されず拒否されます。

`Nat` 引数には、整数リテラル、スコープ内の `Nat` パラメーター、または `+` と `*` を使う多項式を指定できます。

```gcl
type Fixed<N: Nat> {
    Fixed(value: Dimensionless),
}

type Matrix<M: Nat, N: Nat> {
    Matrix(value: Fixed<M * N + 1>),
}

param matrix: Matrix<2, 3> = Matrix<2, 3>(
    value: Fixed<7>(value: 1.0),
);
```

`N` や `M * N` のような裸の名前や積は、ソース構文では意図的に曖昧です。コンパイラーは、対応するパラメーターのカインドが判明してから解決します。

すべてのカインドのジェネリックパラメーターにデフォルト値を指定できます。

```gcl
index Component = { X, Y, Z };
type Unframed { Unframed }

type Vec3<
    D: Dim,
    I: Index = Component,
    F: Type = Unframed,
    N: Nat = 3,
> {
    Vec3(x: D, y: D, z: D),
}

// The omitted arguments are Component, Unframed, and 3.
param pos: Vec3<Length> = Vec3<Length>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
```

デフォルト値を持つパラメーターは、末尾の連続した部分を形成しなければなりません。各デフォルト値が参照できるのは、リスト内でそれより前に宣言されたパラメーターだけです。引数を省略できるのは、この末尾のデフォルト付き部分だけです。すべてのパラメーターにデフォルト値がある場合、山括弧のリストを伴わない型名とコンストラクター名は、すべてのデフォルト値を適用します。

`Nat` と `Index` は異なる種別です。裸の Nat が Index に持ち上げられることはありません。`Index` 引数には `Fin(3)` を、`Nat` 引数には `3` を使用してください。そのため `D[3]` は拒否され、`D[Fin(3)]` と記述するよう提案されます。

### ジェネリックパラメーターの単一化 { #generic-parameter-unification }

ジェネリック型を使用する際、型パラメーターは文脈から単一化されます。

```
param pos: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 1.0 km, y: 0.0 km, z: 0.0 km);
```

コンパイラーは**単一化**を行います。宣言された型パラメーター（ジェネリック変数を含む場合があります）を具体的な型と照合し、各ジェネリック変数の束縛を決定します。

ジェネリック変数が複数回現れる場合、すべての出現箇所は同じ具体的な型に単一化されなければなりません。

### ジェネリクス内の次元式 { #dimension-expressions-in-generics }

ジェネリックな次元パラメーターは、型定義内の複合次元式に使用できます。

### 構造的な有限インデックス { #structural-finite-indexes }

`Fin(N)` は、整数位置 `0` から `N - 1` を持つ Index を明示的に構築します。

```
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };
param mat: Dimensionless[Fin(2), Fin(3)] =
    for i: Fin(2), j: Fin(3) { 1.0 };
node transposed: Dimensionless[Fin(3), Fin(2)] =
    for j: Fin(3), i: Fin(2) { @mat[i, j] };
```

2 つの有限の構造的インデックスが等しいのは、正規化された要素数の式が等しい場合に限ります。Nat 式は加算と乗算をサポートし、`*` は `+` より強く結合します。減算はサポートしません。大きい側を加算で表現してください。たとえば、入力に `D[Fin(N + 1)]` を、その小さい出力に `D[Fin(N)]` を使用します。

`Fin(0)` は無効です。`for i: Fin(N)` のループ変数は `Key<Fin(N)>` 型のキーであり、その有限軸を持つ値へのインデックスアクセスに使えます。これには境界を追跡する加算演算（`i + c : Key<Fin(N + c)>`）を介したアクセスも含まれます。整数としての使用は明示的です。`to_int(i)` は通常の `Int` を生成します。

## 型の等価性 { #type-equivalence }

2 つの型は、次の場合に等価です。

- **数量**: 標準形で同じ次元を持ちます。名前付き次元は透過的です（例: `Velocity` は `Length / Time` と等価です）。
- **Int**: 両方とも `Int` です。
- **Bool**: 両方とも `Bool` です。
- **Datetime**: 同じ時刻系です。`Datetime<UTC>` と `Datetime<TT>` は異なる型です。
- **代数的型**: 所有者で修飾された宣言型名が同じで、ジェネリック引数が等価です。コンストラクターの個数とペイロード形状は、その公称的な宣言に属するものであり、別の構造的な型同一性を生むことはありません。
- **インデックス付き**: 要素型が同じで、同じインデックスが同じ順序で並びます。`T[I, J]` と `T[J, I]` は異なる型です。

一般的な部分型関係はありません。`Length` は `Dimensionless` に代入できず、`Vec3<Length, ECI>` は、両方が同じフィールドを持っていても `Vec3<Length, Unframed>` に代入できません。唯一の構造的な例外は `Fin` キーの拡大です。`N <= M` の場合、`Key<Fin(M)>` が期待される位置に `Key<Fin(N)>` が受け入れられます。名前付きキーと座標キーは公称的です。`Key<Maneuver>` と `Key<Phase>` は決して単一化されず、キーとその内容型も単一化されません（`Key<TimeStep>` は `Time` ではありません。`coord` で取り出してください）。

修飾付きラベル式は、その軸のキー型（`Maneuver#Departure : Key<Maneuver>`）を持つため、キーは他のプリミティブと同様に型の等価性に関与します。パターン位置では同じ表記が引き続きセレクターになります。場合分けには `Key<X>` 値に対する `match` を使用してください。

## 名前の名前空間とパス構文 { #name-namespaces-and-path-syntax }

Graphcal は、ソースの各位置を、3 つの名前空間のうち厳密に 1 つで解決します。

- **Static** — 公称型、次元、インデックス、ジェネリックな型レベルの束縛変数。
- **Term** — パラメーター、ノード、定数、コンストラクター、関数、DAG／プラグイン／インスタンスの別名、インデックスラベル、レキシカルな束縛。
- **Unit** — プレリュードの単位、基本単位、定数で倍率が決まる単位、実行時に倍率が決まる単位。

各名前空間には、スコープごとに 1 つの衝突領域があります。名前空間内の分類ごとに独自のフォールバックテーブルが用意されることはありません。同名の 2 つのフラットな Term は競合しますが、Static、Term、Unit は意図的に同じ表記を共有できます。組み込みの実体は厳密に 1 つの名前空間に属します。名前解決は構文に従い、選択した名前解決が失敗しても、別の名前空間を探索することはありません。

パスの区切り記号は、その選択を保持します。

- `.` は、境界の前ではドット区切りの DAG パスをたどり、式の後では実行時のフィールドを射影します。
- `::` は、モジュール、DAG、プラグイン、または設定済みインスタンスから 1 つのメンバーへ境界を越えます（`lib::Orbit`、`demo::lerp(...)`、`@stage::delta_v`）。
- `#` は、明示的なインデックス所有者からラベルを選択します（`Phase#Burn`、`mission::Phase#Burn`）。

DAG の入力束縛も同じ規則に従います。マーカーのない束縛は厳密に `param` を意味します。Static の入力には `type`、`dim`、`index` が必要です。`param` マーカーはありません。レキシカルな Term の束縛変数は、別の可視なローカル Term やフラットな Term を隠蔽できません。また、Static のジェネリック束縛変数は、可視な Static 名を隠蔽できません。

## 実体の完全な対応表 { #complete-entity-map }

| 実体 | 型か | 第一級の値か | DAG パラメーターにできるか | DAG ノードにできるか | 式での出現箇所 |
|--------|-----------|---------------------|------------|-----------|----------------------|
| 数量値 | ValueType | はい | はい | はい | はい |
| Int 値 | ValueType | はい | はい | はい | はい |
| Bool 値 | ValueType | はい | はい | はい | はい |
| Datetime 値 | ValueType | はい | はい | はい | はい |
| 代数的な値 | ValueType | はい | はい | はい | はい |
| 代数的コンストラクター | いいえ | いいえ | いいえ | いいえ | 構築と match パターン |
| 名前付きインデックスのラベル | 式としては ValueType（`Key<X>`） | はい（定数キー） | はい（キーとして） | はい | 式とインデックスアクセス。match パターンとマップ／テーブルのキーではセレクター |
| インデックス付きの値 | DeclType | はい | はい | はい | `for` を介して |
| 座標インデックスのラベル | ValueType（`Key<C>`） | はい（キーとして） | はい（キーとして） | はい | インデックスアクセス、等値比較、`coord` による取り出し |
| `Fin` の位置 | ValueType（`Key<Fin(N)>`） | はい（キーとして） | はい（キーとして） | はい | インデックスアクセス、等値比較、加算によるキー演算、`to_int` による取り出し |
| 組み込み定数（`PI`、`E`、`TAU`） | ValueType（`Dimensionless`） | はい | はい | はい | `@` を付けない裸の参照 |
| 組み込み関数 | いいえ | いいえ | いいえ | いいえ | 呼び出しのみ |
| 外部関数 | いいえ | いいえ | いいえ | いいえ | 修飾付き呼び出しのみ（`ns::fn(...)`） |
| DAG モジュール（ファイルのルートまたはインラインブロック） | いいえ | いいえ | いいえ | いいえ | `include` によるインスタンス化とインラインの `@d(args)::out` 呼び出し |
| インポートされたモジュールの別名 | いいえ | いいえ | いいえ | いいえ | 直接の DAG 呼び出し（`@m(args)::out`）と修飾（`m::item`、`@m.child(args)::out`） |
| 次元 | いいえ。`Dim` の要素です | いいえ | ジェネリック `<D: Dim>` として | ジェネリックとして | 数量型構文内 |
| 時刻系 | いいえ。意味論上の `TimeScale` の要素です | いいえ | いいえ | いいえ | `Datetime<TT>` 形式の型構文内 |
| 単位 | いいえ（コンパイル時） | いいえ | いいえ | いいえ | リテラルと変換先 |
| タイムゾーン | いいえ（表示メタデータ） | いいえ | いいえ | いいえ | `datetime(...)` の引数と `-> "Area/Location"` の変換先 |
| インデックス軸 | いいえ。`Index` の要素です | いいえ | ジェネリック `<I: Index>` として | ジェネリックとして | インデックス付きの型構文内 |
| 自然数 | いいえ。`Nat` の要素です | いいえ | ジェネリック `<N: Nat>` として | ジェネリックとして | Nat 式と `Fin(N)` の要素数 |
| 文字列リテラル | いいえ（`String` 型はありません） | いいえ | いいえ | いいえ | 単一行の境界構文: 日時の引数、タイムゾーンの変換先、プロットのプロパティ、プラグインのパス |
| 属性 | いいえ | いいえ | いいえ | いいえ | 宣言メタデータのみ |

名前付きインデックスのラベルは所有者修飾構文（`Maneuver#Departure`）を使い、代数的コンストラクターは裸の構文またはモジュール修飾構文（`Nominal` または `module::Nominal`）を使います。どちらも Term ですが、ラベルはその正規のインデックスに関連付けられた Term スコープに属し、コンストラクターはモジュールのフラットな Term スコープに属します。そのため、異なるインデックスでラベルを再利用でき、分類固有の名前解決を導入せずに、ラベルがフラットなコンストラクターと表記を共有することもできます。Static と Term は異なる名前空間なので、プライマリーコンストラクターも Static の型と表記を共有できます。

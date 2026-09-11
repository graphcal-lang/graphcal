---
icon: material/format-list-numbered
---

# インデックス { #indexes }

インデックスは、値のコレクションに用いられる有限のラベル集合です。複数の関連する値に対して、型付きで次元安全な操作を可能にします。

## 有限ラベルインデックス { #finite-label-indexes }

名前付きラベルを持つ有限インデックスを宣言します:

```
index Maneuver = { Departure, Correction, Insertion };
```

ラベルは慣例として `PascalCase` を用い、インデックスによって名前空間化されます: `Maneuver#Departure`。

名前付きラベルはインデックス軸上の位置を識別します。式としては、所有者修飾されたラベルは
`Key<Maneuver>` 型を持つ自己型付きの定数[インデックスキー](#index-keys)です。`#` は
正規のインデックス所有者を直接選択します。ルックアップが、同じラベルを持つフラットな
コンストラクターや別のインデックスを探索することはありません。したがって、異なるインデックスが
同じラベルを再利用でき、ラベルがフラットなコンストラクターと同じ綴りを共有することもできます。
同じラベルの綴りは、map/table のキー、expected-fail キー、および `match` パターンにおける
セレクターとしても機能します。

ラベルリストは**順序付き**です。ラベルが宣言された順序が、その軸のインデックス順序になります。
すべてのインデックス種別はこのような固有の順序を持ちます。ここではラベルの宣言順、
[座標インデックス](#coordinate-indexes)では座標の順序、
[`Fin(N)`](#structural-finite-indexes)では昇順の位置です。順序に依存する操作、特に
[`scan`](#scan-cumulative-fold) はこの順序に従います。また、この順序はレンダリング順序や
[extern 関数バッファー](extern-functions.md)の要素順序も決定します。

!!! warning "ラベルの順序は意味を持ちます"
    `{ Departure, Correction, Insertion }` と
    `{ Insertion, Correction, Departure }` は*異なる*インデックスを宣言します。
    `scan` は宣言されたラベル順に累積するため、`index` 宣言のラベルを並べ替えると
    下流の結果が変わります。軸の順序そのものを変更すべき場合にのみ、ラベルを
    並べ替えてください。

!!! note "空のインデックスは存在しません"
    すべてのインデックスは少なくとも 1 つの要素を持ちます。名前付きインデックスはラベルを
    宣言しなければならず、`Fin(0)` は無効であり、座標コンストラクターは少なくとも 1 つの
    座標を生成しなければなりません。したがって、完成したインデックス付き値は常に空では
    ありません。

!!! note "文脈依存の構文名"
    `scan` と `unfold` は、`(` が続く裸の呼び出しヘッドとしてのみ再帰構文を選択します。
    `range`、`linspace`、`step`、`points` は座標インデックス宣言においてのみ特別であり、
    `Fin` は Index 位置においてのみ特別です。それ以外の場所では、これらの綴りは通常の
    識別子のままです。

## インデックス付き値 { #indexed-values }

型に `[IndexName]` を注釈して、インデックス付き値を作成します:

```
node delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.12 km/s,
    Maneuver#Insertion: 1.83 km/s,
};
```

`param` と `node` のどちらもインデックス付きにできます。

各軸は最大 1,000,000 エントリーまで含むことができ、1 つの即時に実体化される値に含まれる
すべての具体的な軸の積も最大 1,000,000 個のスカラーリーフでなければなりません。この合計の
上限は評価前にチェックされます。`Nat` および必須インデックスの束縛がインスタンス化された後も
含めてチェックされるため、コンパクトな多軸式が無制限の割り当てを要求することはできません。
大きすぎる具体的な形状は `D035` コンパイルエラーになります。

インデックス付き値は**全域写像**です。インデックスのすべてのラベルがちょうど 1 回ずつ
現れなければならず、コンパイラーはエントリーの欠落や重複を拒否します。map エントリーの
記述順序は表示上のものにすぎません。構築される値はインデックス順序に正規化されるため、
`Maneuver#Insertion` を最初に記述してもまったく同じ値が生成されます。順序を決定するのは
`index` 宣言だけであり、map や table リテラルがそれを上書きすることはできません。

## 要素アクセス { #element-access }

`[Index#Label]` で特定の要素にアクセスします:

```
node departure_dv: Velocity = @delta_v[Maneuver#Departure];
```

または、反復対象の軸のキーであるループ変数を使います:

```
node doubled: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

構造的な `Fin` インデックスでは、定数 `Int` 位置(`@v[2]`)は静的にチェックされ、
`Fin` ループ変数は加算的な [Fin キー算術](#fin-key-arithmetic)をサポートします:

```
param v: Dimensionless[Fin(4)] = for i: Fin(4) { 1.0 };
node shifted: Dimensionless[Fin(3)] = for i: Fin(3) { @v[i + 1] };
```

`Key` 型の任意の式も同じ方法で要素を選択します。
[キーによる要素アクセス](#key-based-element-access)を参照してください。

## `for` 内包表記 { #for-comprehensions }

インデックス付き値の各要素を変換します:

```
node doubled: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

結果は同じインデックスを持つ新しいインデックス付き値です。

多軸の内包表記では、本体を明示的なタプルキーマーカーで始めることができます。これらの名前は
純粋な糖衣構文であり、`for` の束縛と順序どおりに正確に一致しなければなりません。追加の変数を
導入することはありません:

```
node v: Velocity[Maneuver, TimeStep] = for m: Maneuver, t: TimeStep {
    (m, t) => @accel[m] * coord(t)
};
```

## 集約関数 { #aggregation-functions }

集約関数は現在、ちょうど 1 つのインデックス軸のみを受け付けます。数値集約には量(quantity)
要素が必要です。ほとんどは要素の次元を保持しますが、`product` は次元を具体的な軸の濃度
(cardinality)乗にします。`count` はインデックス付きでない任意の要素型を受け付け、正確な
濃度を `Int` として返します。`argmax` と `argmin` は、極値の値ではなくその*位置*を
[インデックスキー](#index-keys)として返します。

| 関数 | 説明 | 結果の型 |
|----------|-------------|-------------|
| `sum(values)` | 全要素の合計 | 要素と同じ次元 |
| `product(values)` | 全要素の積 | 要素の次元を軸の濃度乗したもの |
| `maximum(values)` | 最大の要素 | 要素と同じ次元 |
| `minimum(values)` | 最小の要素 | 要素と同じ次元 |
| `argmax(values)` | 最大要素のキー | `Key<I>` |
| `argmin(values)` | 最小要素のキー | `Key<I>` |
| `mean(values)` | 算術平均 | 要素と同じ次元 |
| `rss(values)` | 二乗和平方根 | 要素と同じ次元 |
| `count(values)` | 要素数 | `Int` |

同点の場合、`argmax` と `argmin` はインデックス順序で最初の極値を返すため、恒等式
`@x[argmax(@x)] == maximum(@x)` が保証されます(軸は決して空になりません)。

```
node total: Velocity = sum(for m: Maneuver { @delta_v[m] });
node largest: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node combined_sigma: Velocity = rss(for m: Maneuver { @delta_v[m] });
node n: Int = count(for m: Maneuver { @delta_v[m] });
node normalized: Velocity = sum(@delta_v) / to_float(count(@delta_v));
node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];
```

`Int` と量の算術が暗黙的に混在することはありません。スカラー計算で濃度が必要な場合は
`to_float(count(...))` を使用してください。多軸の値は、まず明示的な `for` 内包表記で
単一の軸に射影しなければなりません(`D021`)。全軸および部分軸の集約はまだ定義されていません。

## インデックスキー { #index-keys }

すべての軸要素は、項レベルでは `Key<I>` 型の**キー**として反映されます。ここで `I` は軸
(名前付き、座標、`Fin`、または必須)です。これは `Dim`/`Quantity(D)` のパターンを
反映したものです。軸は静的な型レベルの実体のままであり、その要素は通常の実行時の値になります。

`Key<I>` は通常の値型です。キーは `node` および `const node` 宣言に格納でき、DAG
パラメーターに渡すことができ、コンストラクターのペイロードフィールドに保持でき、それ自体を
インデックス付きにすることもできます(`Key<TimeStep>[Maneuver]`)。キーはその軸の識別子と
要素位置として表現されます。座標キーは浮動小数点の座標ではなく位置を保持するため、キーを
介した再インデックスは正確であり、浮動小数点のルックアップになることは決してありません。

```
index Maneuver = { Departure, Correction, Insertion };
param delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.12 km/s,
    Maneuver#Insertion: 1.83 km/s,
};

node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];
```

### ラベルはキー定数です { #labels-are-key-constants }

修飾されたラベルは自己型付きの定数式です:
`Maneuver#Departure : Key<Maneuver>`。コンストラクター名が式でもありパターンでもあるのと
まったく同様に、ラベルの綴りはパターン的な位置(`match` のアーム、map/table のキー、table の
ヘッダーとスライス)ではセレクターとしての役割を保ち、それ以外のすべての場所では `Key` 型の
項になります:

```
node fallback: Key<Maneuver> = Maneuver#Correction;
```

軸を持たない綴りが暗黙的にキーになることは決してありません。`node k: Key<Fin(3)> = 1;` は
拒否されます。注釈は型をチェックするものであり、整数をキーに変換するものではありません。
以下の明示的な導入形式を使用してください。

### ループ変数はキーです { #loop-variables-are-keys }

ループ変数はちょうど 1 つの型、すなわちその軸のキー型を持ちます。

| 束縛 | ループ変数の型 | 内容の抽出 |
|---------|--------------------|--------------------|
| `for m: Maneuver` | `Key<Maneuver>` | 不透明(等値比較と `match` のみ) |
| `for t: TimeStep` | `Key<TimeStep>` | `coord(t)` による座標 |
| `for i: Fin(3)` | `Key<Fin(3)>` | `to_int(i)` による整数 |

二重の用法はありません。座標の算術と比較は明示的な抽出 `coord(t)` を通じて行い、`Fin`
キーの整数としての利用は `to_int(i)` を通じて行います。特に `t == 0.5 s` は型エラーです。
`coord(t) == 0.5 s` と書いてください。座標に対する `unfold` クロージャーの束縛子
(`prev_t`、`t`)も同じ規則に従います。

### 等値比較と `match` { #equality-and-match }

同じ軸のキーは `==` と `!=` で比較できます。異なる軸のキーが比較されることは決してなく、
キーには順序もありません。順序が必要な場合は、まず内容を抽出してください
(`coord(t1) < coord(t2)`、`to_int(i) < to_int(j)`)。

```
node critical: Key<Maneuver> = argmax(@delta_v);
node is_critical: Bool[Maneuver] = for m: Maneuver { m == @critical };
```

具体的な名前付き軸のキーは網羅的な `match` を駆動します。名前付きインデックスのループ変数に
用いるのと同じラベルパターンを使用します:

```
node contingency: Dimensionless = match @critical {
    Maneuver#Departure  => 1.10,
    Maneuver#Correction => 1.50,
    Maneuver#Insertion  => 1.25,
};
```

必須(`pub(bind)`)軸のキーはインデックスアクセスと等値比較のみをサポートします。
`match` には具体的なラベル集合が必要です。

### キーによる要素アクセス { #key-based-element-access }

角括弧は各軸位置に対して任意の `Key` 型の項を受け付け、ラベル、ループ変数、計算されたキーを
自由に混在させることができます:

```
node peak_per_maneuver: Key<TimeStep>[Maneuver] = for m: Maneuver {
    argmax(for t: TimeStep { @v[m, t] })
};
node v_at_peak: Velocity[Maneuver] = for m: Maneuver {
    @v[m, @peak_per_maneuver[m]]
};
```

アクセスは軸の識別子によって統制されます。`Key<Maneuver>` で `Phase` 軸をインデックスする
ことはできません。唯一の拡大(widening)は構造的なものです。`N <= M` のとき、`Key<Fin(M)>` が
期待される場所で `Key<Fin(N)>` が受け付けられます。構造的な `Fin` 軸はその濃度形式を直接
保持します。宣言された軸は、識別子のみの比較に黙ってフォールバックするのではなく、チェック済みの
意味論的定義を保持しなければなりません。

!!! note "評価の粒度"
    コンパイル時定数のキー(ラベル、ループ変数、または `key(...)`)によるアクセスは、
    要素ごとの細粒度な DAG エッジを保ちます。実行時キーによるアクセスは gather であり、
    エッジはインデックス付き宣言全体を覆います。

### 抽出: `coord` と `to_int` { #extractions-coord-and-to_int }

キーの抽出は次のものだけです:

| 関数 | シグネチャ | 適用対象 |
|----------|-----------|------------|
| `coord(k)` | `Key<C> -> Quantity(D)` | 次元 `D` 上の座標軸 |
| `to_int(k)` | `Key<Fin(N)> -> Int` | `Fin` 軸のみ |

`Fin` キーでは位置が意味論的な内容です。座標キーでは座標のみが公開されます。位置を公開すると
プログラムがグリッド定義に結合してしまうためです。名前付きキーは不透明です。等値比較と
`match` をサポートし、それ以外は何もサポートしません。

```
node peak: Key<TimeStep> = argmax(@altitude);
node peak_time: Time = coord(@peak);
```

### キーの導入 { #introducing-keys }

すべての導入形式は軸を明示的に指定します:

| 形式 | チェック | 失敗しうるか |
|------|----------|-----------|
| ラベル式 `Maneuver#Departure` | コンパイル時 | いいえ |
| ループ変数 | コンパイル時 | いいえ |
| `argmax(v)` / `argmin(v)` | — | いいえ(軸は空ではありません) |
| 静的な `c` を持つ `key(Fin(N), c)` | コンパイル時の範囲チェック | いいえ |
| 実行時の `Int` `e` を持つ `fin_key(Fin(N), e)` | 実行時の範囲チェック | はい |
| `floor_key(C, q)` / `ceil_key(C, q)` | 実行時 | はい(軸の範囲外の場合) |
| `nearest_key(C, q)` | 実行時 | いいえ(全域) |

`key(Fin(N), c)` は静的定数を取り、チェック中に完全に解消されます。範囲外の位置は
コンパイルエラーです。`fin_key(Fin(N), e)` は実行時の `Int` を受け付け、評価中に範囲チェックを
行います。失敗はノードごとの評価エラーとなり、下流のノードは `DependencyFailed` を
受け取ります。名前付き軸には位置による導入形式は不要であり(ラベルを書いてください)、座標軸には
量からキーへの正確なキャストはありません。代わりに明示的な検索ポリシーでグリッド点を選択して
ください。

```
param readings: Pressure[Fin(8)] = for i: Fin(8) { 100.0 Pa };
node second_key: Key<Fin(8)> = key(Fin(8), 1);
param requested: Int = 5;
node channel: Key<Fin(8)> = fin_key(Fin(8), @requested);
node selected: Pressure = @readings[@channel];
```

座標検索は軸の次元を持つ量を取り、クエリの下側(`floor_key`)、上側(`ceil_key`)、または
最も近い(`nearest_key`)グリッド点を返します。`floor_key` と `ceil_key` はクエリが軸の
範囲外にある場合に失敗します。`nearest_key` は全域であり、中間点での同点は軸の始点側に
解決されます。

```
index TimeStep = range(0.0 s, 10.0 s, step: 0.1 s);
param event_time: Time = 3.47 s;
node before_event: Key<TimeStep> = floor_key(TimeStep, @event_time);
node near_event: Key<TimeStep> = nearest_key(TimeStep, @event_time);
```

!!! warning "暗黙的な実行時インデックスアクセスはありません"
    実行時の `Int` が `Fin` 軸を暗黙的にインデックスすることは決してなく(`@x[@n]` は
    拒否されます。`fin_key(Fin(N), @n)` と書いてください)、量が座標軸をルックアップすることも
    決してありません(`@x[@t]` は拒否されます。`nearest_key` / `floor_key` / `ceil_key` で
    ポリシーを選択してください)。`@x[2]` のような静的に解消される定数位置は引き続き
    合法です。

### Fin キー算術 { #fin-key-arithmetic }

`Fin` キーに静的な Nat 定数を加えたものは再び `Fin` キーであり、その上限は型において
正確に追跡されます:

```text
i : Key<Fin(N)>,  c a static Nat  ⟹  i + c : Key<Fin(N + c)>
```

このシフトは正確で失敗しません。上限が型の中で増加するため、実行時チェック、ラップアラウンド、
クランプはありません。インデックスアクセスや `Fin` の拡大と直接合成できます:

```
param values: Velocity[Fin(5)] = for i: Fin(5) { 1.0 m/s };
node diffs: Velocity[Fin(4)] = for i: Fin(4) {
    @values[i + 1] - @values[i]   // i + 1 : Key<Fin(5)> — exact
};
node smoothed: Velocity[Fin(3)] = for i: Fin(3) {
    (@values[i] + @values[i + 1] + @values[i + 2]) / 3.0
};
```

この断片は意図的に厳密です。1 つのキー、`+`、静的な Nat 定数。それ以外はすべて、明示された
理由により除外されています:

- `i - 1` は、位置 0 で失敗しうるうえ、Nat には減算がありません。加算的に構造を
  組み直してください(`T[Fin(N + 1)]` の入力を取り、`T[Fin(N)]` の出力を生成します)。
- `i * 2`、`i + j` は、緩い静的上限しか存在せず、自然な対象軸へ拡大できません。
- `i % c`、`i / c` は、ラップアラウンドと切り捨てがデータポリシーであって、インデックス算術
  ではないためです。
- 実行時の `@k` を用いた `i + @k` は、実行時オフセットがいかなる静的上限からも逃れてしまいます。
  `to_int(i) + @k : Int` と書き、`fin_key` を通じて再び入ってください。

名前付きキーと座標キーには算術はまったくありません。名前付きキーでは実行時の動作をラベルの
宣言順序に結合してしまい、座標キーでは曖昧(位置か座標か?)だからです。代わりに `coord(t)` と
量の算術を使用してください。

!!! note "レンダリングと境界"
    キーは出力境界において、ラベル、位置、または座標としてレンダリングされます(表示上のみ)。
    座標キーはその軸の表示スケールと単位を使用します。たとえば、秒で宣言された軸上のキーは
    位置 `2` ではなく `2 s` としてレンダリングされます。JSON および境界入力も同様に、
    座標キーを位置ではなく座標でエンコードします。境界入力は名前付きキーをラベルで、
    `Fin` キーを位置でエンコードします。キーは実験的なプラグイン ABI を越えません
    (`Complex` と同じ姿勢です)。

## `scan`(累積畳み込み) { #scan-cumulative-fold }

`scan` はインデックス順序に沿った累積を計算します:

```
node cumulative: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
```

引数:

1. スキャン対象のインデックス付き値
2. アキュムレーターの初期値
3. アキュムレーターと各要素を結合するクロージャー `|acc, item| expr`

結果は、各要素がその要素までの(その要素を含む)累積結果であるインデックス付き値です。

ソースはちょうど 1 つの軸を持たなければなりません。`scan` が多軸ソースから暗黙的に軸を
選択することは決してありません。まず明示的な `for` 内包表記で各ランク 1 の系列を選択して
ください。アキュムレーター自体がインデックス付きであってもかまいません。その軸は結果において
ソース軸の後に保持されます:

```
index Element = { A, B };
node increments: Dimensionless[Maneuver] = for maneuver: Maneuver { 1.0 };
node initial: Dimensionless[Element] = for element: Element { 0.0 };
node state: Dimensionless[Maneuver, Element] = scan(
    @increments,
    @initial,
    |previous, increment| for element: Element {
        previous[element] + increment
    }
);
```

本体は、すべての状態軸とその順序を含め、正確にアキュムレーターの型を返さなければなりません。
各反復は、次のアキュムレーターを構築する前に、前のアキュムレーター全体を読み取ります。

累積の順序はインデックス順序であり、これはインデックスに固有のものです。特定の値がどのように
書かれたかには決して依存しません:

- **名前付きインデックス**: ラベルの宣言順序(上記の `Maneuver` 軸では `Departure`、次に
  `Correction`、次に `Insertion`)。
- **座標インデックス**: `start` から `end` へ向かう座標の順序(ステップが負の場合は降順)。
- **`Fin(N)`**: 昇順の位置 `#0` から `#(N-1)` まで。

インデックス付き値はインデックス順序に正規化されるため、`Maneuver#Insertion` を最初に列挙した
map リテラルをスキャンしても、やはり `Departure` が最初に累積されます。

## 多重インデックス付き値 { #multi-indexed-values }

値はタプルキーを用いて複数のラベルインデックスでインデックス付けできます:

```
index Phase = { Launch, Cruise, Arrival };

node spacecraft_mass: Mass[Phase, Maneuver] = {
    (Phase#Launch, Maneuver#Departure): 5000.0 kg,
    (Phase#Launch, Maneuver#Correction): 0.0 kg,
    (Phase#Launch, Maneuver#Insertion): 0.0 kg,
    (Phase#Cruise, Maneuver#Departure): 0.0 kg,
    (Phase#Cruise, Maneuver#Correction): 4500.0 kg,
    (Phase#Cruise, Maneuver#Insertion): 0.0 kg,
    (Phase#Arrival, Maneuver#Departure): 0.0 kg,
    (Phase#Arrival, Maneuver#Correction): 0.0 kg,
    (Phase#Arrival, Maneuver#Insertion): 4000.0 kg,
};
```

複数のインデックス引数で要素にアクセスします:

```
node launch_dep: Mass = @spacecraft_mass[Phase#Launch, Maneuver#Departure];
```

## ラベルインデックスと座標インデックスの混在 { #mixed-label-and-coordinate-indexes }

値は、カテゴリカルなラベル軸と時間などの座標軸を組み合わせることができます。

### `for` 内包表記による構築 { #construction-via-for-comprehension }

混在インデックス値を作成する最も一般的な方法は、複数束縛の `for` 内包表記です:

```
index Maneuver = { Departure, Correction, Insertion };
index TimeStep = range(0.0 s, 1.0 s, step: 0.5 s);

node accel: Acceleration[Maneuver] = {
    Maneuver#Departure: 10.0 m/s^2,
    Maneuver#Correction: 5.0 m/s^2,
    Maneuver#Insertion: -3.0 m/s^2,
};

node v: Velocity[Maneuver, TimeStep] = for m: Maneuver, t: TimeStep {
    @accel[m] * coord(t)
};
```

座標ループ変数 `t` は `TimeStep` 軸のキーです。それが指す量としての座標は `coord(t)` で
明示的に抽出します([インデックスキー](#index-keys)を参照)。

### `for` 値を持つ map リテラルによる構築 { #construction-via-map-literal-with-for-values }

各名前付きラベルのエントリーが座標インデックスに対する内包表記を含む map リテラルを使用する
こともできます:

```
node v: Velocity[Maneuver, TimeStep] = {
    Maneuver#Departure: for t: TimeStep { @accel[Maneuver#Departure] * coord(t) },
    Maneuver#Correction: for t: TimeStep { @accel[Maneuver#Correction] * coord(t) },
    Maneuver#Insertion: for t: TimeStep { @accel[Maneuver#Insertion] * coord(t) },
};
```

### 混在軸の要素アクセス { #mixed-axis-element-access }

ラベルと範囲変数の両方を指定して要素にアクセスします:

```
node departure_v: Velocity[TimeStep] = for t: TimeStep {
    @v[Maneuver#Departure, t]
};
```

### 集約 { #aggregation }

どちらの軸に対しても独立に集約できます:

```
// Sum over the label axis for each time step
node total_v: Velocity[TimeStep] = for t: TimeStep {
    sum(for m: Maneuver { @v[m, t] })
};

// Max over the time axis for each maneuver
node max_v: Velocity[Maneuver] = for m: Maneuver {
    maximum(for t: TimeStep { @v[m, t] })
};
```

## Table リテラル { #table-literals }

多重インデックス付き値に対して、`table` 式はより読みやすいスプレッドシートのようなレイアウトを
提供します:

### 1D テーブル { #1d-table }

```
param delta_v: Velocity[Maneuver] = table[Maneuver] {
    Departure:  2.46 km/s;
    Correction: 0.12 km/s;
    Insertion:  1.83 km/s;
};
```

`table[...]` 内の名前付き軸は、`mission::Maneuver` のようなモジュール修飾パスを含め、インデックス付き型と同じ完全なパスを受け付けます。通常の table 本体におけるラベルは裸(`Maneuver#Departure` ではなく `Departure`)です。`table[...]` が各行軸または列軸に対してちょうど 1 つの所有者を明示的に与えるためです。行は `;` で終端します。

名前付き軸を使用する table は少なくとも 1 つのデータ行を含まなければなりません。空の本体、または後続のデータ行を持たない 2D 列ヘッダーはパースエラーです。すべての 3D 以上のスライスセクションも同様に、少なくとも 1 つのデータ行を含まなければなりません。

### 2D テーブル { #2d-table }

```
param m: Mass[Phase, Maneuver] = table[Phase, Maneuver] {
    : Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;
};
```

最後のインデックスが列になり、最後から 2 番目のインデックスが行になります。ヘッダー行は `:` で始まり列ラベルを列挙し、その後に `RowLabel: value, value, ...;` の形式のデータ行が続きます。カンマはセルを区切ります。セミコロンは明示的な行(第 2 次元)区切りであるため、その前に冗長な末尾カンマを付けないでください。

### 3D 以上のテーブル { #3d-table }

3 つ以上のインデックスに対しては、修飾ラベルを持つスライスセクションを使用します:

```
param m: Mass[Time, Phase, Maneuver] = table[Time, Phase, Maneuver] {
    [Time#T1]
    : Departure, Correction, Insertion;
    Launch:  5000.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4500.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    4000.0 kg;

    [Time#T2]
    : Departure, Correction, Insertion;
    Launch:  4800.0 kg, 0.0 kg,    0.0 kg;
    Cruise:     0.0 kg, 4300.0 kg, 0.0 kg;
    Arrival:    0.0 kg, 0.0 kg,    3800.0 kg;
};
```

各 `[SliceLabel]` セクションは、それ自身のヘッダー行とデータ行を含みます。名前付きスライス
ラベルは `Index#Variant` 構文(インデックスがインポートされている場合は
`module::Index#Variant`)を使用し、`Fin` スライスラベルは `#N` を使用します。

### 有限インデックスのテーブル { #finite-index-tables }

位置ベースのベクトルや行列には明示的な `Fin(N)` 軸を使用します。それらのラベルは
`#0, #1, ...` であり、通常の行と列では省略されます:

```
// 1D Fin axis
param v: Dimensionless[Fin(3)] = table[Fin(3)] {
    1.0;
    2.0;
    3.0;
};

// 2D, both axes finite
param m: Dimensionless[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0, 2.0, 3.0;
    4.0, 5.0, 6.0;
};

// 2D, mixed: named columns, finite rows
param mixed: Dimensionless[Fin(2), Maneuver] = table[Fin(2), Maneuver] {
    : Departure, Correction;
    1.0, 2.0;
    3.0, 4.0;
};

// 3D with a finite slice axis
param m3d: Dimensionless[Fin(2), Phase, Maneuver] = table[Fin(2), Phase, Maneuver] {
    [#0]
    : Departure, Correction;
    Launch: 1.0, 2.0;
    Cruise: 3.0, 4.0;

    [#1]
    : Departure, Correction;
    Launch: 5.0, 6.0;
    Cruise: 7.0, 8.0;
};
```

スライスラベル(最後の 2 つを除くすべての軸)は常に明示的なマーカーを必要とします。
名前付き軸では `[Index#Variant]`、`Fin` 軸では `[#N]` です。同じ規約が多重宣言の共有軸にも
適用されます。`Fin` 行軸はラベルなしの行を持ち、`Fin` スライス軸は `[#N]` セクションを
使用します。

`table` 式は純粋な糖衣構文です。パース時に map リテラルへ脱糖されます。

## 多重宣言 { #multi-declarations }

**多重宣言**は、同じ行軸を共有する N 個の並列な `param` / `node` / `const node` 宣言を導入する単一の表層形式です。同じ行に属する値を揃えて記述できます:

```
pub index Component = { ComponentA, ComponentB };

param      power_consumption: Power[Component],
param      duty_cycle:        Dimensionless[Component],
const node mass_per_unit:     Mass[Component]
  = table[Component, (_, _, _)] {
      :           _,       _,    _;
      ComponentA: 10.0 W,  0.5,  2.5 kg;
      ComponentB: 12.0 W,  1.0,  3.1 kg;
  };
```

- 左辺の各スロットは完全な宣言です。種別(`param` / `node` / `const node`)、名前、型注釈からなります。
- `table[SharedAxis, (…)]` の角括弧は、行軸に続けて括弧で囲まれたスロットタプルを宣言します。タプルの前のカンマは必須です。タプルの各エントリーは `_`(`T[SharedAxis]` 型の 1-D スロット)か、モジュール修飾軸を含む名前付き軸(`T[SharedAxis, ExtraAxis]` 型の 2-D スロット)のいずれかです。
- ヘッダー行 `: …;` は列ごとにちょうど 1 つのセルを持ちます。1-D スロットではセルは `_` でなければなりません。すべての 2-D スロットのセルは裸の文脈依存ラベルを使用し、対応するスロットタプルの軸がその一意な所有者を与えます。
- データ行のラベルは裸のままです。共有行軸が table 接頭辞で明示されているためです。

1-D / 2-D スロットの混在:

```
pub index Component = { ComponentA, ComponentB };
pub index OperationMode = { Safe, Nominal };

param      power_consumption:  Power[Component],
param      n_installed:        Int[Component],
const node mass_per_unit:      Mass[Component],
param      power_mode_active:  Bool[Component, OperationMode]
  = table[Component, (_, _, _, OperationMode)] {
      :            _,       _, _,      Safe, Nominal;
      ComponentA:  10.0 W,  1, 2.5 kg,               true,                  true;
      ComponentB:  12.0 W,  2, 3.1 kg,              false,                  true;
  };
```

追加の軸を持てるスロットは最大 1 つです。

### スライスセクションを持つ N-D { #n-d-with-slice-sections }

共有軸の接頭辞が複数の軸を持つ場合、本体はスライスセクションを使用します。各スライスセクションは、**最後の軸を除く**(最後の軸は行軸になります)すべての共有軸を覆う `[Axis#Variant, …]` ラベルで始まり、その後に通常どおりヘッダー行とデータ行が続きます。

```
pub index Phase = { Launch, Cruise };
pub index Component = { ComponentA, ComponentB };
pub index OperationMode = { Safe, Nominal };

param      power_consumption: Power[Phase, Component],
param      power_mode_active: Bool[Phase, Component, OperationMode]
  = table[Phase, Component, (_, OperationMode)] {
      [Phase#Launch]
      :            _,       Safe, Nominal;
      ComponentA:  5.0 W,                 true,                 false;
      ComponentB:  6.0 W,                false,                 false;

      [Phase#Cruise]
      :            _,       Safe, Nominal;
      ComponentA:  10.0 W,                true,                  true;
      ComponentB:  12.0 W,               false,                  true;
  };
```

スライスラベルは、単一宣言の 3D 以上のテーブルで用いられる規約に合わせて、宣言された順序で各共有軸を修飾しなければなりません(裸の `Launch` ではなく `Phase#Launch`)。

- 多重宣言は**純粋な糖衣構文**です。各スロットは、それ自身の `table[SharedAxis] { … }` 初期化子を持つ通常の宣言に脱糖されます。スロット間の参照は、他の任意の宣言とまったく同様に機能します(`@other_slot[Variant]`)。
- 属性(`#[…]`)は多重宣言には許可されません。可視性はスロットごとに記述し、通常の宣言規則に
  従います(`pub node` は有効ですが、`pub param` と `pub(bind) node` は無効です)。

## 座標インデックス { #coordinate-indexes }

座標インデックスは静的に既知の量としての座標を保持します。境界と間隔は、同じ次元を持つ
有限のコンパイル時の量でなければなりません。実行時入力や整数(`Int`)値は受け付けられません。

### 正確なステップの `range` { #exact-step-range }

増分が基準となる場合は `range` を使用します:

```
index TimeStep = range(0.0 s, 1.0 s, step: 0.25 s);
index Countdown = range(1.0 s, -1.0 s, step: -0.5 s);
```

ステップは非ゼロでなければならず、`start` から `end` へ向かう方向でなければなりません。
浮動小数点の検証許容誤差の範囲内で正確に終点に到達しなければなりません。Graphcal はクリップも
オーバーシュートも**行いません**。たとえば、
`range(0.0 s, 1.0 s, step: 0.6 s)` は拒否されます。内部の座標は `start + position * step` から
直接導出されるため累積的なドリフトを回避でき、検証済みの最終座標は宣言された終点を保持します。

### 点数ベースの `linspace` { #count-based-linspace }

点の数が基準となる場合は `linspace` を使用します:

```
index Samples = linspace(0.0 s, 1.0 s, points: 5);
```

これはちょうど 5 つの座標を生成し、宣言された両端点を保持します。`points` は静的な正の
`Nat` でなければなりません。単一要素は両端点が同一の場合にのみ有効です:

```
index Origin = linspace(0.0 m, 0.0 m, points: 1);
```

どちらのコンストラクターも昇順および降順の座標をサポートします。偶発的な巨大な割り当てを
防ぐため、具体的なインデックスの濃度は 1,000,000 要素に制限されています。

## 構造的有限インデックス { #structural-finite-indexes }

`Fin(N)` は整数位置 `0` から `N - 1` を持つ明示的な構造的インデックスです。これは `Nat` 値
`N` とは異なります。Graphcal が裸の Nat を暗黙的に Index へ変換することは決してありません。

```
// A 3-element vector
param v: Dimensionless[Fin(3)] = for i: Fin(3) { 1.0 };

// A 2-by-3 matrix
param m: Dimensionless[Fin(2), Fin(3)] =
    for i: Fin(2), j: Fin(3) { 1.0 };
```

インデックス付き型、`for` 束縛、table、Index ソートのジェネリック引数、必須離散インデックス
ポートの束縛を含め、すべての Index 位置で `Fin(N)` と書いてください。`Dimensionless[3]`、
`for i: range(3)`、`table[3]` のような廃止された形式は、`Fin(3)` の提案とともに拒否されます。
`Fin(0)` および実用上の上限を超える濃度は無効です。

### ジェネリックのソートは区別されたままです { #generic-sorts-stay-distinct }

`Nat` 引数は Nat のままであり、`Fin(...)` は Index 引数です:

```
type Vector<N: Nat, D: Dim> {
    Vector(values: D[Fin(N)]),
}

type IndexedVector<I: Index, D: Dim> {
    IndexedVector(values: D[I]),
}

param a: Vector<3, Dimensionless>;
param b: IndexedVector<Fin(3), Dimensionless>;
```

したがって、`Vector<Fin(3), Dimensionless>` と
`IndexedVector<3, Dimensionless>` は暗黙的な変換ではなくソートエラーになります。

### Nat 算術と反復 { #nat-arithmetic-and-iteration }

`Fin(...)` 内の濃度には、`Fin(N + 1)` や `Fin(Rows * Cols)` のように Nat の加算と乗算を
使用できます。乗算は加算より強く結合します。Nat 式は正規多項式形式に正規化されます。
減算は意図的にサポートされていません。大きい側を加算的に表現してください。たとえば、入力に
`D[Fin(N + 1)]` を、より小さい出力に `D[Fin(N)]` を使用します。

`Fin(N)` ループ変数は `Key<Fin(N)>` 型のキーです([インデックスキー](#index-keys)を参照)。
対応する値をインデックスでき、その整数位置は `to_int(i)` で明示的に抽出します:

```
node doubled: Dimensionless[Fin(3)] =
    for i: Fin(3) { @v[i] * 2.0 };
node positions: Dimensionless[Fin(3)] =
    for i: Fin(3) { to_float(to_int(i)) };
```

加算的な [Fin キー算術](#fin-key-arithmetic)は型において上限を追跡するため、シフトされた
アクセスは静的にチェックされます:

```
param values: Velocity[Fin(4)] = for i: Fin(4) { 1.0 m/s };
node diffs: Velocity[Fin(3)] =
    for i: Fin(3) { @values[i + 1] - @values[i] };
```

`Fin` 軸は名前付きインデックスと合成できます:

```
index Phase = { Launch, Cruise };
node data: Dimensionless[Fin(3), Phase] =
    for i: Fin(3), p: Phase { 1.0 };
```

## 必須インデックス { #required-indexes }

インデックスは、ラベルや座標を指定**せずに**宣言することができます。これらは
**必須インデックス**であり、DAG がライブラリとして使用される際に
[パラメーター化されたインクルード](multi-file.md#index-bindings)を通じて束縛されなければ
なりません。

### 必須離散インデックス { #required-discrete-index }

```
pub(bind) index Axis;
```

これは制約のない離散インデックスポート `Axis` を宣言します。呼び出し側は、これを具体的な
名前付きインデックスに束縛するか、別の互換性のある必須離散インデックスを転送するか、`Fin(3)` の
ような構造的な軸を直接与えることができます。座標軸には代わりに以下の次元制約付きの形式を
使用します。

必須インデックスはライブラリの束縛可能なインターフェースを形成し、`pub(bind)` を付けなければ
なりません([可視性、束縛可能性、入力ポート](multi-file.md#visibility-bindability-and-input-ports)を参照)。
注釈を省略した場合、または単なる `pub` を書いた場合はエラー `V002` です。

### 必須座標インデックス { #required-coordinate-index }

```
pub(bind) index Step: Time;
```

これは次元 `Time` に制約された座標インデックス `Step` を宣言します。インクルードする DAG は、
これを同じ次元を持つ `range` または `linspace` インデックスに束縛しなければなりません。
ジェネリックな外側の DAG は代わりに、それ自身の互換性のある必須座標インデックスを転送することも
できます。その外側の入力は最終的に具体的な軸に束縛されなければなりません。

### 必須インデックスの使用 { #using-required-indexes }

必須インデックスは具体的なインデックスとまったく同じように使用されます。型注釈、`for` 内包表記、インデックスアクセス、`match`、map/table リテラルにおける軸として使用できます:

```
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

このファイルは単独では評価できません。互換性のあるインデックスを与える束縛とともに
インクルードされなければなりません([インデックス束縛](multi-file.md#index-bindings)を参照)。

## `unfold`(漸化式) { #unfold-recurrence-relations }

`unfold` は、各値が前の状態に依存する明示的な座標インデックス上の値を計算します:

```
node x: Dimensionless[TimeStep] = unfold(
    TimeStep,
    @x0,
    |prev_x, prev_t, t| prev_x * (1.0 + @rate * (coord(t) - coord(prev_t)))
);
```

最初の座標は `@x0` を受け取ります。それ以降のすべての座標では、本体は前の値とともに、前の
座標キーと現在の座標キー(`Key<TimeStep>`)を受け取ります。それらの量としての座標は、座標
ループ変数の場合とまったく同様に `coord(...)` で抽出します。軸は式に属します。それを選択する
ために囲んでいる宣言の型が参照されることはないため、`unfold` はネストしたり直ちに消費したり
できます:

```
node total: Dimensionless = sum(
    unfold(TimeStep, @x0, |prev_x, prev_t, t| prev_x + @rate * (coord(t) - coord(prev_t)))
);
```

次のステップを計算する際は `prev_x` 束縛を直接使用してください。それ自身の初期化子における
`@x` への参照は、`@x[prev_t]`、`@x[t]`、および将来の座標の形式を含め、通常の依存関係
サイクルです。これは時間ステップシミュレーションや離散動的システムに有用です。

### インデックス付きの漸化状態 { #indexed-recurrence-state }

状態は 1 つ以上の固定軸でインデックス付けできます。`unfold` はその座標軸をそれらの状態軸の
前に付加します:

```text
initial:    T[Element]
trajectory: T[TimeStep, Element]

initial:    T[Row, Column]
trajectory: T[TimeStep, Row, Column]
```

これは、次のすべての成分が前の完全なスナップショットを読み取る連成系をサポートします。
たとえば、行列ベクトル漸化式 `x(next) = coupling * x(previous)` は次のようになります:

```
pub index Element = { A, B };
pub index TimeStep = range(0.0 s, 2.0 s, step: 1.0 s);

param initial: Dimensionless[Element] = {
    Element#A: 1.0,
    Element#B: 2.0,
};
param coupling: Dimensionless[Element, Element] = {
    (Element#A, Element#A): 1.0,
    (Element#A, Element#B): 1.0,
    (Element#B, Element#A): 1.0,
    (Element#B, Element#B): 0.0,
};

node trajectory: Dimensionless[TimeStep, Element] = unfold(
    TimeStep,
    @initial,
    |previous, previous_time, time| for i: Element {
        sum(for j: Element { @coupling[i, j] * previous[j] })
    }
);
```

1 つの座標に対するすべての成分は同じ凍結された `previous` 値から構築されるため、ループの
順序が同時更新をインプレース更新に変えてしまうことはありません。本体は初期状態の要素型、軸、
軸の順序を正確に返さなければなりません。これは通常の多軸構文であり、`T[Element][TimeStep]` の
ようなネストした型ではありません。

インデックス付きの `unfold` と `scan` の例については、完全な実行可能フィクスチャ
[`indexed_state_recurrence.gcl`](https://github.com/graphcal-lang/graphcal/blob/main/tests/fixtures/valid/indexed_state_recurrence.gcl)
を参照してください。

## 任意のインデックスに対する集約 { #aggregation-over-any-index }

インデックス付き値に対して集約関数を直接使用します:

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
```

組み込みの集約関数(`sum`、`minimum`、`maximum`、`argmin`、
`argmax`、`mean`、`count`)は任意のインデックス型で機能します。

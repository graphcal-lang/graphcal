---
icon: material/package-variant
---

# 組み込みリファレンス { #built-in-reference }

このページでは、Graphcal のプレリュードが提供するすべての次元、単位、定数、関数を一覧します。これらはすべての `.gcl` ファイルで、`import` 宣言なしに利用できます。

修飾なしで呼び出せる関数の集合は閉じています。ユーザーコードからこの集合に追加することはできません。外部から提供される関数は `import plugin` ブロックで宣言し、そのエイリアスで修飾して呼び出します。[Extern 関数（プラグイン）](extern-functions.md)を参照してください。

## 組み込み定数 { #built-in-constants }

| 名前 | 型 | 値 |
|------|------|-------|
| `PI` | `Dimensionless` | 3.14159265358979... |
| `E` | `Dimensionless` | 2.71828182845904... |
| `TAU` | `Dimensionless` | 6.28318530717958... (2*PI) |
| `SQRT2` | `Dimensionless` | 1.41421356237309... |
| `LN2` | `Dimensionless` | 0.69314718055994... |
| `LN10` | `Dimensionless` | 2.30258509299404... |

## 組み込み関数 { #built-in-functions }

### 数学関数 { #math-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `sqrt(x)` | `D -> D^(1/2)` | 平方根（次元は 1/2 倍になる） |
| `cbrt(x)` | `D -> D^(1/3)` | 立方根（次元は 1/3 倍になる） |
| `abs(x)` | `D -> D` または `Complex<D> -> D` | 絶対値または複素数の大きさ |
| `sign(x)` | `D -> Dimensionless` | 値の符号（-1.0、0.0、1.0 のいずれか） |
| `round(x)` | `Dimensionless -> Dimensionless` | 最も近い整数への丸め |
| `trunc(x)` | `Dimensionless -> Dimensionless` | ゼロ方向への切り捨て |
| `floor(x)` | `Dimensionless -> Dimensionless` | 負の無限大方向への丸め |
| `ceil(x)` | `Dimensionless -> Dimensionless` | 正の無限大方向への丸め |
| `clamp(x, min, max)` | `(D, D, D) -> D` | 値を範囲内に制限 |
| `hypot(a, b)` | `(D, D) -> D` | 斜辺の長さ（sqrt(a^2 + b^2)） |
| `exp(x)` | `Dimensionless -> Dimensionless` または `Complex<Dimensionless> -> Complex<Dimensionless>` | 実数または複素数の指数関数 |
| `expm1(x)` | `Dimensionless -> Dimensionless` | exp(x) - 1（小さな x に対して数値的に安定） |
| `ln(x)` | `Dimensionless -> Dimensionless` | 自然対数 |
| `log1p(x)` | `Dimensionless -> Dimensionless` | ln(1 + x)（小さな x に対して数値的に安定） |
| `log(x, base)` | `(Dimensionless, Dimensionless) -> Dimensionless` | 任意の底の対数 |
| `log2(x)` | `Dimensionless -> Dimensionless` | 底 2 の対数 |
| `log10(x)` | `Dimensionless -> Dimensionless` | 底 10 の対数 |

丸め関数（`round`、`trunc`、`floor`、`ceil`）は次元付きの引数を拒否します。
「整数に丸める」という操作は、物理量に対して単位に依存しない意味を持たないためです。
メートル単位で整数の値が、センチメートル単位でも整数になるとは限りません。
物理量を丸めるには、明示的な粒度で割り、無次元の比を丸めてから、スケールを
戻します。

```graphcal
// Round down to whole centimeters.
node whole_cm: Length = floor(@x / 1.0 cm) * 1.0 cm;
```

### 複素数関数 { #complex-functions }

`Complex<D>` は次元 `D` 上の組み込み複素数型です。両成分は SI 基本単位で
格納され、同じ次元を持つ必要があります。構築は明示的に行います。
`a + bi` のようなリテラル構文はありません。

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `complex(re, im)` | `(D, D) -> Complex<D>` | 直交座標成分から構築 |
| `polar(magnitude, phase)` | `(D, Angle) -> Complex<D>` | 非負の大きさと位相から構築 |
| `to_complex(x)` | `D -> Complex<D>` | 実数量を虚部ゼロの複素数に昇格 |
| `re(z)` | `Complex<D> -> D` | 実部を取り出す |
| `im(z)` | `Complex<D> -> D` | 虚部を取り出す |
| `abs(z)` | `Complex<D> -> D` | ユークリッド距離としての大きさを返す |
| `phase(z)` | `Complex<D> -> Angle` | `atan2(im, re)` による主値の位相を返す |
| `conj(z)` | `Complex<D> -> Complex<D>` | 複素共役を返す |
| `exp(z)` | `Complex<Dimensionless> -> Complex<Dimensionless>` | `e^z` を返す。次元付きの入力は拒否される |

```graphcal
node displacement: Complex<Length> = complex(3.0 m, 4.0 m);
node same_polar: Complex<Length> = polar(5.0 m, 53.130102 deg);
node magnitude: Length = abs(@displacement); // 5 m
node direction: Angle = phase(@displacement);
node shifted: Complex<Length> = @displacement + to_complex(1.0 m);
```

`polar` は負の大きさを拒否し、位相を暗黙に π だけ回転させることはありません。
算術演算および複素数関数の結果は有限でなければなりません。複素数のゼロ除算は
評価エラーです。除算は、正確な binary64 入力成分の商を計算し、各結果成分を
一度だけ丸めます。これにより、中間積をオーバーフローさせることなく、表現可能な
符号付き非正規化数が保持されます。ただし、これによって他の複素数演算が正確に
なるわけではありません。実数と複素数を混在させた演算の組み合わせについては、
[複素数演算](expressions.md#complex-arithmetic)を参照してください。

### 三角関数 { #trigonometric-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `sin(x)` | `Angle -> Dimensionless` | 正弦 |
| `cos(x)` | `Angle -> Dimensionless` | 余弦 |
| `tan(x)` | `Angle -> Dimensionless` | 正接 |
| `asin(x)` | `Dimensionless -> Angle` | 逆正弦 |
| `acos(x)` | `Dimensionless -> Angle` | 逆余弦 |
| `atan(x)` | `Dimensionless -> Angle` | 逆正接 |
| `atan2(y, x)` | `(D, D) -> Angle` | 2 引数の逆正接 |

### 双曲線関数 { #hyperbolic-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `sinh(x)` | `Dimensionless -> Dimensionless` | 双曲線正弦 |
| `cosh(x)` | `Dimensionless -> Dimensionless` | 双曲線余弦 |
| `tanh(x)` | `Dimensionless -> Dimensionless` | 双曲線正接 |
| `asinh(x)` | `Dimensionless -> Dimensionless` | 逆双曲線正弦 |
| `acosh(x)` | `Dimensionless -> Dimensionless` | 逆双曲線余弦 |
| `atanh(x)` | `Dimensionless -> Dimensionless` | 逆双曲線正接 |

### 選択関数 { #selection-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `least(a, b)` | `(D, D) -> D` | 2 つの値のうち小さい方 |
| `greatest(a, b)` | `(D, D) -> D` | 2 つの値のうち大きい方 |

これらのセレクターは常にちょうど 2 つの値を取ります。インデックス付きの値を
縮約するには、代わりに `minimum(values)` または `maximum(values)` を使用してください。

### 型変換関数 { #type-conversion-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `to_float(x)` | `Int -> Dimensionless` | 整数を binary64 の `Dimensionless` 量に変換 |
| `to_int(x)` | `Dimensionless -> Int` | 整数値の `Dimensionless` 量を正確に変換。小数部を持つ値、非有限値、範囲外の値は拒否される |
| `to_int(k)` | `Key<Fin(N)> -> Int` | `Fin` インデックスキーの整数位置を取り出す。名前付きキーと座標キーは拒否される |

`to_int` が丸め方針を暗黙に選ぶことはありません。丸めが意図した方針である
場合は、変換の前に `trunc`、`floor`、`ceil`、`round` を適用してください。

```graphcal
node exact: Int = to_int(3.0);
node toward_zero: Int = to_int(trunc(3.7));
node nearest: Int = to_int(round(3.7));
```

整数性の検査は、表現されている binary64 値に対して行われます。その値を
計算する過程で既に失われた精度を、変換で検出することはできません。

### インデックスキー関数 { #index-key-functions }

これらの関数は、型 `Key<I>` の[インデックスキー](indexes.md#index-keys)を
構築および消費します。`C` は次元 `D` 上の座標軸、`c` は静的な Nat 定数、
`e` は実行時の `Int`、`q` は次元 `D` の量です。

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `key(Fin(N), c)` | `(Fin(N), static Nat) -> Key<Fin(N)>` | 静的な `Fin` キー。位置はプログラムのコンパイル時に範囲検査される |
| `fin_key(Fin(N), e)` | `(Fin(N), Int) -> Key<Fin(N)>` | 実行時の `Fin` キー。範囲外の位置はノード単位の評価エラーとなる |
| `floor_key(C, q)` | `(C, D) -> Key<C>` | `<= q` を満たす最大の格子点。`q` が軸の範囲より下の場合は失敗する |
| `ceil_key(C, q)` | `(C, D) -> Key<C>` | `>= q` を満たす最小の格子点。`q` が軸の範囲より上の場合は失敗する |
| `nearest_key(C, q)` | `(C, D) -> Key<C>` | 最も近い格子点。全域的で、中点の同点は軸の始点側に解決される |
| `coord(k)` | `Key<C> -> D` | 座標軸キーの量座標を取り出す |
| `to_int(k)` | `Key<Fin(N)> -> Int` | `Fin` キーの整数位置を取り出す |

名前付き軸には構築関数は不要です。`Maneuver#Departure` のような修飾ラベルは、
それ自体が型 `Key<Maneuver>` の定数です。また、取り出し関数もありません。
名前付きキーは、等値比較と `match` を除いて不透明です。座標軸に対して、
量からキーへの正確なキャストは存在しません。代わりに、明示的な探索方針で
格子点を選択してください。

```graphcal
index TimeStep = range(0.0 s, 10.0 s, step: 0.1 s);

param event_time: Time = 3.47 s;
node before_event: Key<TimeStep> = floor_key(TimeStep, @event_time);
node event_coord: Time = coord(@before_event);

param readings: Pressure[Fin(8)] = for i: Fin(8) { 100.0 Pa };
param requested: Int = 5;
node channel: Key<Fin(8)> = fin_key(Fin(8), @requested);
node selected: Pressure = @readings[@channel];
```

### 日時関数 { #datetime-functions }

#### コンストラクター { #constructors }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `datetime("...")` | `OffsetDateTimeLiteral -> Datetime` | 明示的な `Z` または数値オフセット付きの RFC 3339 を解析 |
| `datetime("...", "tz")` | `(CivilDateTimeLiteral, TimezoneLiteral) -> Datetime` | ローカルの暦時刻を IANA タイムゾーンで解釈 |
| `epoch<S>("...")` | `CivilDateTimeLiteral -> Datetime<S>` | 時刻系を持たない暦座標を静的な時刻系 `S` で解釈 |
| `from_jd(x)` | `Dimensionless -> Datetime` | ユリウス日（UTC）から構築 |
| `from_mjd(x)` | `Dimensionless -> Datetime` | 修正ユリウス日（UTC）から構築 |
| `from_unix(x)` | `Dimensionless -> Datetime` | 秒単位の Unix タイムスタンプから構築 |

各コンストラクターの解釈元はちょうど 1 つです。

- 1 引数の `datetime` は RFC 3339 形式
  `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` を要求します（RFC 3339 では小文字の
  `t` と `z` も許されます）。数値オフセットの時と分は、それぞれ `00..=23` と
  `00..=59` の範囲内でなければなりません。
- タイムゾーンベースの `datetime` は、オフセット、タイムゾーン、時刻系の接尾辞を
  含まないローカルの暦座標を要求します。
- `epoch<S>` は、オフセット、タイムゾーン、時刻系の接尾辞を含まないローカルの
  暦座標を要求し、`S` はサポートされている単独の時刻系のいずれかでなければ
  なりません。

日時リテラルは検査時に解析されます。無効な暦日付や、解釈元の欠落または矛盾は、
実行時の失敗ではなくコンパイルエラーです。ローカルの暦時刻も、検査時に
Graphcal に同梱された tzdb に対して解決されます。夏時間の切り替えの隙間に
存在しない時刻や、夏時間の重複によって 2 回現れる時刻は、暗黙にずらしたり
選択したりせず、拒否されます。特定の瞬間を選ぶには、明示的な数値オフセット
付きの 1 引数 `datetime` を使用してください。

#### 時刻系の変換 { #time-scale-conversions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `to_utc(x)` | `Datetime(any) -> Datetime<UTC>` | UTC に変換 |
| `to_tai(x)` | `Datetime(any) -> Datetime<TAI>` | TAI に変換 |
| `to_tt(x)` | `Datetime(any) -> Datetime<TT>` | 地球時（TT）に変換 |
| `to_tdb(x)` | `Datetime(any) -> Datetime<TDB>` | 太陽系力学時（TDB）に変換 |
| `to_et(x)` | `Datetime(any) -> Datetime<ET>` | 暦表時（ET）に変換 |
| `to_gpst(x)` | `Datetime(any) -> Datetime<GPST>` | GPS 時刻に変換 |
| `to_gst(x)` | `Datetime(any) -> Datetime<GST>` | Galileo システム時刻に変換 |
| `to_bdt(x)` | `Datetime(any) -> Datetime<BDT>` | BeiDou 時刻に変換 |
| `to_qzsst(x)` | `Datetime(any) -> Datetime<QZSST>` | QZSS 時刻に変換 |

#### 数値への変換 { #numeric-conversions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `to_jd(x)` | `Datetime -> Dimensionless` | ユリウス日（UTC 日数）に変換 |
| `to_mjd(x)` | `Datetime -> Dimensionless` | 修正ユリウス日（UTC 日数）に変換 |
| `to_unix(x)` | `Datetime -> Dimensionless` | Unix タイムスタンプ（1970-01-01 からの秒数）に変換 |

#### 抽出関数 { #extraction-functions }

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `year(x)` | `Datetime<S> -> Int` | 年を取り出す |
| `month(x)` | `Datetime<S> -> Int` | 月を取り出す（1-12） |
| `day(x)` | `Datetime<S> -> Int` | 日を取り出す（1-31） |
| `hour(x)` | `Datetime<S> -> Int` | 時を取り出す（0-23） |
| `minute(x)` | `Datetime<S> -> Int` | 分を取り出す（0-59） |
| `second(x)` | `Datetime<S> -> Int` | 秒を取り出す（0-59） |
| `weekday(x)` | `Datetime<S> -> Int` | ISO 曜日（1=月曜日、7=日曜日） |
| `day_of_year(x)` | `Datetime<S> -> Int` | 年内通算日（1-366） |

すべての暦抽出関数は、入力に宣言された時刻系を使用します。たとえば、
`hour(epoch<TT>("2024-11-05T12:00:00"))` は `12` です。同じ瞬間の UTC 座標が
異なっていても同様です。`->` で適用された表示用タイムゾーンは書式設定の
メタデータにとどまり、抽出関数の結果を変えることはありません。

#### Datetime のドメイン制約 { #datetime-domain-constraints }

`Datetime<S>` は、両端を含む `min:` および `max:` 制約をサポートします。境界は
コンパイル時定数であり、制約対象とまったく同じ時刻系を持つ必要があります。

```graphcal
param event: Datetime<TT>(
    min: epoch<TT>("2025-01-01T00:00:00"),
    max: epoch<TT>("2025-12-31T23:59:59"),
) = epoch<TT>("2025-06-01T12:00:00");
```

異なる時刻系間の境界には、`to_tt(datetime("...Z"))` のような明示的な変換が
必要です。制約は、インデックス付きの日時値および制約付きの代数的ペイロード
フィールドに要素ごとに適用されます。
[型システム — ドメイン制約](type-system.md#domain-constraints)を参照してください。

#### タイムゾーン表示 { #timezone-display }

`->` 演算子を使うと、内部の値を変更せずに、日時を特定のタイムゾーンで表示できます。

```
node meeting_ny: Datetime = @meeting -> "America/New_York";
```

タイムゾーン名には、計算された文字列ではなく引用符付きのリテラルを指定します。
Graphcal は同梱の IANA タイムゾーンデータベースを使うため、同じリリースでは、
サポートされるすべてのプラットフォームで同じタイムゾーン規則が適用されます。

### 集約関数（インデックス付きの値） { #aggregation-functions-indexed-values }

これらの関数は、ランク 1 の `for` 内包表記またはインデックス付きの値に対して
動作します。`D` は量の次元、`T` はインデックス付きでない任意の値型です。

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `sum(values)` | `D[I] -> D` | 全要素の和 |
| `product(values)` | `D[I] -> D^\|I\|` | 全要素の積。濃度が結果の次元を決定する |
| `maximum(values)` | `D[I] -> D` | 最大要素 |
| `minimum(values)` | `D[I] -> D` | 最小要素 |
| `argmax(values)` | `D[I] -> Key<I>` | 最大要素のキー。同点はインデックス順で最初の極値に解決される |
| `argmin(values)` | `D[I] -> Key<I>` | 最小要素のキー。同点はインデックス順で最初の極値に解決される |
| `mean(values)` | `D[I] -> D` | 算術平均 |
| `rss(values)` | `D[I] -> D` | 二乗和平方根。スケーリングした累積で計算される |
| `count(values)` | `T[I] -> Int` | 正確な要素数 |

`mean` は binary64 の入力値を正確に累積し、最終的な商のみを丸めます。
したがって、大きな中間和が表現可能な平均をオーバーフローさせることはなく、
桁落ちによってはるかに小さい残りの項が消えることもありません。入力リテラル
自体は通常の binary64 精度を持ちます。これは 10 進演算ではありません。

`argmax` と `argmin` は、極値の値ではなく、その*位置*を
[インデックスキー](indexes.md#index-keys)として返します。そのため、結果を
使ってソースや同じ軸上の他の任意の値を再びインデックス参照できます。軸は
決して空にならず、同点の解決は決定的であるため、恒等式
`@x[argmax(@x)] == maximum(@x)` は常に成り立ちます。

```graphcal
index Maneuver = { Departure, Correction, Insertion };
param delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.12 km/s,
    Maneuver#Insertion: 1.83 km/s,
};
param fuel_margin: Dimensionless[Maneuver] = {
    Maneuver#Departure: 1.1,
    Maneuver#Correction: 1.2,
    Maneuver#Insertion: 1.3,
};

node critical: Key<Maneuver> = argmax(@delta_v);
node critical_dv: Velocity = @delta_v[@critical];        // 2.46 km/s
node critical_margin: Dimensionless = @fuel_margin[@critical];
```

`product` は、深くネストした二項乗算を使わずに連鎖的な縮約を明示的に
表現します。次元付きの積では、出力の指数が判明するように具体的な軸の濃度が
必要です。無次元の積では不要です。`rss` は、不確かさのバジェットや
ユークリッド成分の合成のための単位安全な二乗和平方根演算であり、回避可能な
中間のオーバーフロー／アンダーフローを防ぎます。バジェットの決定論的な
平均値と統計的なシグマは別々の値として保持し、それぞれの意味論に従って
合成してください。

```graphcal
index BudgetLine = { Structure, Payload };
param means: Mass[BudgetLine] = {
    BudgetLine#Structure: 100.0 kg,
    BudgetLine#Payload: 50.0 kg,
};
param sigmas: Mass[BudgetLine] = {
    BudgetLine#Structure: 3.0 kg,
    BudgetLine#Payload: 4.0 kg,
};
node budget_mean: Mass = sum(@means);   // 150 kg
node budget_sigma: Mass = rss(@sigmas); // 5 kg
```

量の算術で無次元スカラーが必要な場合は、`sum(@values) / to_float(count(@values))`
のように、個数を明示的に変換してください。複数軸に対する直接の集約は拒否
されます。`product(for row: Row { product(for column: Column { @a[row, column] }) })`
のように、明示的な `for` を使って選択した 1 つの軸ずつ縮約してください。

### フレームタグ付きベクトルと姿勢 { #frame-tagged-vectors-and-attitudes }

参照フレームはプロジェクト固有の名目型（nominal type）であるため、Graphcal は
グローバルな `Eci` や `Body` などの名前のフレーム値を提供しません。フレームと
ラッパーをプロジェクトの語彙で定義し、その数値カーネルには配列の線形代数
組み込み関数を使用してください。

```graphcal
type Eci { Eci }
type Body { Body }
type Vec3<D: Dim, Frame: Type> { Vec3(x: D, y: D, z: D) }
type Rotation<From: Type, To: Type> {
    Rotation(elements: Dimensionless[Fin(3), Fin(3)])
}
type Quaternion<From: Type, To: Type> {
    Quaternion(w: Dimensionless, x: Dimensionless, y: Dimensionless, z: Dimensionless)
}

node r: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 1.0 m, y: 2.0 m, z: 3.0 m);
node q: Quaternion<Eci, Body> =
    Quaternion<Eci, Body>(w: 1.0, x: 0.0, y: 0.0, z: 0.0);
```

ファントム型引数 `Frame` / `From` / `To` により、型検査時にフレームの誤った
再解釈を防ぎます。
[`linear_algebra_ergonomics.gcl`](https://github.com/graphcal-lang/graphcal/blob/main/tests/fixtures/valid/linear_algebra_ergonomics.gcl)
フィクスチャは、明示的な行列ベクトル積によるフレーム遷移を示しています。
プロジェクトでは、数値成分の演算を、必須の型レベル入力を持つ再利用可能な
DAG としてパッケージ化できます。
[DAG ブロック](functions.md#type-level-inputs-for-reusable-math)を参照してください。

### 線形代数（インデックス付きの量） { #linear-algebra-indexed-quantities }

線形代数の組み込み関数は、ランク 1 のベクトルとランク 2 の行列に対して
動作します。軸は型の一部であり、縮約には要素数が同じであるだけでなく、同じ
型付き軸が必要です。`Fin(N)` 軸は構造的に一致し、名前付き軸は宣言の同一性に
よって一致します。

| 関数 | シグネチャ | 説明 |
|----------|-----------|-------------|
| `dot(a, b)` | `(D1[I], D2[I]) -> D1 * D2` | 内積 |
| `matmul(a, b)` | `(D1[I, J], D2[J, K]) -> (D1 * D2)[I, K]` | `J` に関する行列積 |
| `transpose(a)` | `D[I, J] -> D[J, I]` | 行列の軸を入れ替える |
| `trace(a)` | `D[I, I] -> D` | 型付き軸の正方行列の対角成分の和 |
| `norm(v)` | `D[I] -> D` | ユークリッドベクトルノルム |
| `cross(a, b)` | `(D1[I], D2[I]) -> (D1 * D2)[I]` | 3 成分のクロス積。`I` はちょうど 3 つの要素を持つ必要がある |
| `outer(a, b)` | `(D1[I], D2[J]) -> (D1 * D2)[I, J]` | 外積（テンソル積） |
| `solve(a, b)` | `(D1[I, I], D2[I]) -> (D2 / D1)[I]` | 正方連立方程式 `a * x = b` を解く |
| `inverse(a)` | `D[I, I] -> D^-1[I, I]` | 逆行列 |
| `det(a)` | `D[I, I] -> D^\|I\|` | 行列式。軸の濃度が次元の指数になる |

```graphcal
param A: Length[Fin(2), Fin(3)] = table[Fin(2), Fin(3)] {
    1.0 m, 2.0 m, 3.0 m;
    4.0 m, 5.0 m, 6.0 m;
};
param B: Dimensionless[Fin(3), Fin(2)] = table[Fin(3), Fin(2)] {
    7.0, 8.0;
    9.0, 10.0;
    11.0, 12.0;
};

node AB: Length[Fin(2), Fin(2)] = matmul(@A, @B);
node A_t: Length[Fin(3), Fin(2)] = transpose(@A);
```

暗黙の軸選択やブロードキャストはありません。たとえば、`matmul` は常に第 1
引数の第 2 軸と第 2 引数の第 1 軸を縮約します。濃度が等しくても異なる
名前付き軸は拒否されます。

`solve` と `inverse` はスケーリング付き部分ピボット LU 分解を使用し、特異行列や
数値的に悪条件の行列を拒否し、結果を返す前に残差を検証します。これらの失敗は
明示的な評価エラーであり、`NaN` や無限大を返すことは決してありません。`det` は
特異行列に対しても定義されており、ゼロを返します。行列式の物理次元は `D` の
行列次数乗であるため、`det` には具体的な軸の濃度が必要です。計算された LU の
ピボットは、最終的な行列式を丸める前に正確に乗算されるため、中間積によって
表現可能な結果が失われることはありません。LU 分解自体は浮動小数点演算の
ままであり、正確な行列式アルゴリズムではありません。非特異な分解であっても、
行列式がオーバーフローするか完全にゼロにアンダーフローする場合は評価エラーが
報告されます。表現可能な非正規化数の行列式は保持されます。

密行列カーネルは、宣言の評価 1 回あたり推定 10,000,000 回の算術演算という
検査付きの予算を共有します。過大な行列積や 3 乗オーダーの分解などの処理は、
高コストなカーネルが実行される前に明示的に失敗します。

## プレリュードの基本次元 { #prelude-base-dimensions }

| 次元 | 説明 |
|-----------|-------------|
| `Length` | 空間的な距離 |
| `Time` | 時間の長さ |
| `Mass` | 質量 |
| `Temperature` | 熱力学温度 |
| `ElectricCurrent` | 電流 |
| `Amount` | 物質量 |
| `LuminousIntensity` | 光度 |
| `Angle` | 平面角 |
| `Dimensionless` | 物理次元なし |

## プレリュードの派生次元 { #prelude-derived-dimensions }

| 次元 | 定義 |
|-----------|-----------|
| `Velocity` | `Length / Time` |
| `Acceleration` | `Length / Time^2` |
| `Force` | `Mass * Length / Time^2` |
| `Energy` | `Mass * Length^2 / Time^2` |
| `Power` | `Mass * Length^2 / Time^3` |
| `Pressure` | `Mass / (Length * Time^2)` |
| `Frequency` | `1 / Time` |
| `Area` | `Length^2` |
| `Volume` | `Length^3` |

## プレリュードの単位 { #prelude-units }

### 長さ { #length }

| 単位 | 定義 |
|------|-----------|
| `m` | 基本単位（メートル） |
| `km` | 1000 m |
| `cm` | 0.01 m |
| `mm` | 0.001 m |

### 時間 { #time }

| 単位 | 定義 |
|------|-----------|
| `s` | 基本単位（秒） |
| `min` | 60 s |
| `h` | 3600 s |

`h` は、時間の単位「時」に対するプレリュード唯一の綴りです。`hour(value)` は
日時の抽出関数のままです。

### 質量 { #mass }

| 単位 | 定義 |
|------|-----------|
| `kg` | 基本単位（キログラム） |
| `g` | 0.001 kg |

### 温度 { #temperature }

| 単位 | 定義 |
|------|-----------|
| `K` | 基本単位（ケルビン） |

### 電流 { #electric-current }

| 単位 | 定義 |
|------|-----------|
| `A` | 基本単位（アンペア） |

### 物質量 { #amount-of-substance }

| 単位 | 定義 |
|------|-----------|
| `mol` | 基本単位（モル） |

### 光度 { #luminous-intensity }

| 単位 | 定義 |
|------|-----------|
| `cd` | 基本単位（カンデラ） |

### 角度 { #angle }

| 単位 | 定義 |
|------|-----------|
| `rad` | 基本単位（ラジアン） |
| `deg` | pi/180 rad |

### 力 { #force }

| 単位 | 定義 |
|------|-----------|
| `N` | 1 kg*m/s^2 |
| `kN` | 1000 N |

### エネルギー { #energy }

| 単位 | 定義 |
|------|-----------|
| `J` | 1 N*m |
| `kJ` | 1000 J |

### 仕事率 { #power }

| 単位 | 定義 |
|------|-----------|
| `W` | 1 J/s |
| `kW` | 1000 W |

### 圧力 { #pressure }

| 単位 | 定義 |
|------|-----------|
| `Pa` | 1 N/m^2 |
| `kPa` | 1000 Pa |
| `MPa` | 1000000 Pa |

### 周波数 { #frequency }

| 単位 | 定義 |
|------|-----------|
| `Hz` | 1/s |

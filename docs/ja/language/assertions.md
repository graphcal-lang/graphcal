---
icon: material/check-decagram
---

# アサーションと属性 { #assertions-and-attributes }

アサーションは、不変条件や期待値を検証する評価後のチェックです。
属性は宣言に付与するメタデータ注釈です。これらを組み合わせることで、
言語内でのテストと工学的な前提の追跡が可能になります。

## Assert 宣言 { #assert-declarations }

`assert` 宣言は、計算グラフ全体が評価された後に真偽条件をチェックします。

```
assert fuel_positive = @fuel_mass > 0.0 kg;
```

### 主な特性 { #key-properties }

- assert の名前は、`param` や `node` と同様に、慣例として `lower_snake_case` を使用します。
- assert の本体は、任意の `@param` や `@node`、および定数を参照できます。
- アサーションは**葉ノード**です。どの宣言も `@` でアサーションを参照することは
  できません。`@my_assert` と書くとコンパイルエラー（A003）になります。
- アサーションはグラフ全体の評価の**後**に、宣言順で評価されます。
- アサーションが失敗すると、ゼロ以外の終了コードが返されます。

### 真偽値アサーション { #boolean-assertions }

最も単純な形式は、`Bool` を生成しなければならない式を評価します。

```
param pressure: Pressure = 8.5 MPa;
param max_pressure: Pressure = 10.0 MPa;

assert pressure_safe = @pressure < @max_pressure;
```

本体が `false` に評価されると、アサーションは
"assertion evaluated to false" というメッセージで失敗します。

### インデックス付き真偽値アサーション { #indexed-boolean-assertions }

本体が `Bool[SomeIndex]` に評価される場合、各バリアントが個別にチェック
されます。失敗したバリアントは診断に列挙されます。

```
index Stage = { First, Second, Third };

param thrust: Force[Stage] = ...;
param min_thrust: Force = 100.0 kN;

assert all_stages_ok = for stage: Stage {
    @thrust[stage] > @min_thrust
};
// If First and Third fail:
//   FAIL  (failed at Stage#First, Stage#Third)
```

比較演算子はインデックスなしのオペランドを要求し、ブロードキャストは決して
行いません。上記のように明示的な `for` 内包表記を使い、要素ごとに `Bool[I]` を
生成してください。インデックス付きのコレクションを比較演算子に直接渡すと
コンパイルエラー（`D019`）になります。明示的に記述することで、アサーションの
軸と要素の対応が明確になります。

### 許容誤差アサーション { #tolerance-assertions }

近似的な等値チェックには、`~=` と `+/-` の構文を使用します。

```
assert fuel_budget = @fuel_mass ~= 2847.0 kg +/- 5.0 kg;
```

これは意味的に `abs(@fuel_mass - 2847.0 kg) <= 5.0 kg` と等価ですが、
実際の値、期待値、許容誤差、差分を示す、より詳細な失敗診断を
生成します。

#### 絶対許容誤差 { #absolute-tolerance }

```
assert name = <actual> ~= <expected> +/- <tolerance>;
```

3 つのオペランドはすべて任意の式です。`@param` や `@node`、定数を参照したり、
関数を呼び出したり、算術演算を使用したりできます。`node` の式で有効なものは
すべて使用できます。次元の規則は次のとおりです。

- `actual` と `expected` は**同じ次元**を持つ必要があります。
- `tolerance` も `actual` および `expected` と**同じ次元**を持つ必要があります。
- `tolerance` は**非負**でなければなりません。`-0.0` を含め、負符号付きの
  リテラル許容誤差はコンパイルエラー（`A015`）です。実行時に計算された許容誤差が
  負になった場合、アサーションは `ERROR` を報告します。符号なしの
  リテラルゼロは有効です。実行時には、どちらの符号付きゼロも完全一致の
  意味論を持ちます。

チェックは `abs(actual - expected) <= tolerance` のときに成功します。

例:

```
// All three operands can be graph references
assert mass_check = @computed_mass ~= @expected_mass +/- @mass_tolerance;

// Or mix literals, constants, and references
assert velocity_ok = @v_final ~= 3000.0 m/s +/- 10.0 m/s;
```

#### 相対許容誤差の表現 { #expressing-relative-tolerance }

許容誤差アサーションは絶対的な意味論のみを持ちます。相対的な
パーセンテージは、その絶対許容誤差を明示的に計算して表現してください。

```gcl
assert efficiency = @eta ~= 0.85 +/- abs(0.85) * 0.05;
// Passes if eta is within [0.8075, 0.8925]

assert velocity_approx = @velocity ~= 49.5 m/s +/- abs(49.5 m/s) * 0.05;
// Passes if velocity is within [47.025, 51.975] m/s
```

`+/-` の後の式は、通常の完全な式です。`%` は二項の剰余演算子専用であり、
アサーション固有の後置形式はありません。

#### インデックス付き許容誤差アサーション { #indexed-tolerance-assertions }

許容誤差アサーションは独自の要素ごとの意味論を持ちます。通常の比較演算子とは
異なり、インデックス付きのオペランドを受け付けます。アサーションのインデックス
形状は `actual` から決まります。`expected` と `tolerance` はそれぞれ、
インデックスなし（すべてのキーに適用される）か、まったく同じ軸に同じ順序で
インデックス付けされている（そうでなければ `D011`）必要があります。

```
index Case = { A, B };

node actual: Length[Case] = { Case#A: 1.0 m, Case#B: 2.0 m };
node expected: Length[Case] = { Case#A: 1.0 m, Case#B: 2.5 m };
node tol: Length[Case] = { Case#A: 0.01 m, Case#B: 0.6 m };

// Unindexed tolerance applied to every key:
assert close = @actual ~= @expected +/- 0.1 m;
//   FAIL  (failed at Case#B (actual 2, expected 2.5 +/- 0.1, off by 0.5))

// Per-key tolerance over the same axes:
assert per_key = @actual ~= @expected +/- @tol;

// Derive per-key relative tolerances explicitly as absolute quantities:
node relative_tol: Length[Case] = for case: Case {
    abs(@expected[case]) * 0.25
};
assert relative = @actual ~= @expected +/- @relative_tol;
```

失敗した各キーは、それぞれの実際の値／期待値／差分の詳細とともに報告されます。
バリアント単位の `#[expected_fail(Case#B)]` は、インデックス付き真偽値
アサーションとまったく同様に、インデックス付き許容誤差アサーションでも機能します。

## 属性 { #attributes }

属性は、宣言の前に `#[name]` または `#[name(args)]` の構文で記述する
メタデータ注釈です。

```
#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;
```

### 構文 { #syntax }

```
#[name]                                     // no arguments
#[name(arg1)]                               // one argument
#[name(arg1, arg2, arg3)]                   // multiple arguments
#[name(Index#Variant)]                     // qualified path argument
#[name((Idx#A, Idx#B), (Idx#C, Idx#D))] // tuple key arguments
```

複数の属性を積み重ねることができます。

```
#[lazy]
#[assumes(pressure_safe)]
node expensive: Dimensionless = heavy_computation(@data);
```

未知の属性名はコンパイルエラー（A007）です。

### `#[assumes(...)]` { #assumes }

`#[assumes(...)]` 属性は、宣言の値が、指定されたアサーションが成立する場合に
のみ有効であることを文書化します。グラフの依存関係を作成することは**ありません**。

```
assert pressure_safe = @pressure < 10.0 MPa;

#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;
```

`pressure_safe` が失敗すると、診断には `safety_factor` が無効である可能性が
あることが記載されます。

```
Assertions:
  pressure_safe  FAIL  (assertion evaluated to false)
                       affected: safety_factor
```

#### 規則 { #rules }

- 引数は `assert` 宣言を参照する必要があります。`param`、`node`、
  `const node`、または存在しない名前を参照するとコンパイルエラー（A005）になります。
- `node` および `param` 宣言に対して有効です。`const node` に `#[assumes]` を
  使用するとエラー（A006）になります。定数は実行時の値に依存しないためです。
- 各 `#[assumes(...)]` には少なくとも 1 つのアサーション名を含める必要があります（A020）。
- 複数の異なるアサーションを一度に列挙できます: `#[assumes(a, b, c)]`。
  リスト内で名前を繰り返すとエラー（A021）になり、同じ宣言に 2 つ目の
  `#[assumes(...)]` を積み重ねるとエラー（A019）になります。
- ファイルをまたぐアサーションは、明示的な `include` インスタンスから選択する
  必要があります。その結果得られるインスタンスローカルのエイリアスを
  `#[assumes]` で参照できます。単なる `import` ではアサーションを公開できません（`M024`）。

### `#[expected_fail]` { #expected_fail }

`#[expected_fail]` 属性は、失敗が予期されるアサーションをマークします。
これは、評価全体を失敗させることなく、工学計算における既知の失敗を
文書化するのに役立ちます。

`#[expected_fail]` でマークされたアサーションが失敗した場合、成功として扱われます。
`#[expected_fail]` でマークされたアサーションが成功した場合、失敗
（"unexpected pass"）として扱われます。既知の問題が解決された可能性があり、
属性を削除すべきだからです。

```
// This assertion fails (10 > 20 is false), but it's a known issue
#[expected_fail]
assert x_greater = @x > @y;
```

#### 制約 { #constraints }

- `assert` 宣言に対してのみ有効です。`param`、`node`、`const node` などに
  `#[expected_fail]` を使用するとコンパイルエラー（A008）になります。
- 評価エラー（ゼロ除算など）が反転されることは決してありません。
  `#[expected_fail]` の有無にかかわらず、エラーのままです。
- 引数なしの `#[expected_fail]` は、インデックスなしのアサーションに対してのみ
  有効です。インデックス付きのアサーションでは、失敗が予期されるキーを正確に
  列挙する必要があります（A011）。
- バリアント単位のキーは、インデックス付きのアサーションに対してのみ有効です（A010）。
- 各アサーションまたは選択的 include 項目は、最大 1 つの `#[expected_fail]`
  属性を受け付けます（A019）。キー単位の期待はすべてその 1 つの属性に
  まとめてください。後の属性が前のメタデータを上書きすることはありません。
- 属性内の各 expected-fail キーは一意でなければなりません（A012）。
- 単一インデックスのキーは、アサーションのインデックスに属している必要があります。
  複数インデックスのタプルキーは、アサーションの軸順ですべての軸を含む必要が
  あります（A013、A014）。

#### 包括形式 { #blanket-form }

引数なしで使用した場合、アサーション全体が失敗することが予期されます。

```
#[expected_fail]
assert known_issue = @actual == @expected;
```

#### バリアント単位の形式（単一インデックス） { #per-variant-form-single-index }

インデックス付きのアサーションでは、特定のインデックスバリアントを予期される
失敗としてマークし、他のバリアントには引き続き成功を要求できます。

```
index Mode = { Normal, Eco, Boost };

#[expected_fail(Mode#Boost)]
assert power_ok = for m: Mode { @power_use[m] < @power_gen[m] };
```

ここでは、`Mode#Boost` は失敗が予期され（失敗した場合は成功として扱われ）、
`Mode#Normal` と `Mode#Eco` は引き続き通常どおり成功する必要があります。

#### タプルキー単位の形式（複数インデックス） { #per-tuple-key-form-multi-index }

複数インデックスのアサーションでは、タプルキーによって特定のインデックスの
組み合わせを指定します。

```
index Mode = { Normal, Eco, Boost };
index Phase = { Launch, Cruise };

#[expected_fail((Mode#Normal, Phase#Cruise), (Mode#Boost, Phase#Launch))]
assert within_limits = for m: Mode, p: Phase { @actual[m, p] < @threshold[m, p] };
```

#### 構造的有限軸（`#N` キー） { #structural-finite-axes-n-keys }

`Fin(N)` 軸でインデックス付けされたアサーションは、`table` 式で使用される
スライスラベル構文と同じ、位置指定の `#N` キーを使用します。

```
#[expected_fail(#1)]
assert steps_ok = for i: Fin(3) { @residual[i] < @tolerance };
```

複数軸のタプルキーでは、各成分はその軸のキー形式を使用します。名前付き軸は
`Index#Variant`、`Fin` 軸は `#N` を、アサーションの軸順で記述します。

```
index Mode = { Normal, Boost };

#[expected_fail((Mode#Boost, #2))]
assert grid_ok = for m: Mode, i: Fin(4) { @value[m, i] < @limit };
```

位置は範囲内でなければなりません。`Fin(size)` 軸上の `#N` は
`N < size` を要求します（A016）。裸の整数（`#[expected_fail(1)]`）は構文解析
エラーです。位置指定キーは常に `#` 接頭辞を含みます。

### `#[lazy]` { #lazy }

予約された構文ですが、遅延評価は実装されていません。意味検査は、引数の有無に
かかわらず、すべての宣言および選択的 include 項目に対する `#[lazy]` を拒否します
（A023）。パーサーとフォーマッターは引き続きこれを認識するため、将来の実装で、
型付きの遅延メタデータを実行計画に持ち込む前に、正確な対象と引数の形状を
定義できます。

## 複数ファイルプロジェクトにおけるアサーション { #assertions-in-multi-file-projects }

`import` は、コンパイル時の名前解決のためにモジュールのブループリントを
読み込みます。実行時インスタンスを作成したり評価したりすることはありません。
したがって、ファイルをインポートしてもそのアサーションは実行されず、
アサーションを値としてインポートすることもできません（`M024`）。

アサーションは、明示的な `include` インスタンスごとに実行されます。これにより、
アサーションの結果は、それが検証するのと同じパラメーター束縛および実行時の値に
紐付けられます。include されたアサーションに書かれた `#[expected_fail(Index#Variant)]` は、
そのアサーションを定義しているモジュールで `Index` を解決し、診断はその
モジュールのソースを指します。一方、include の波括弧項目に書かれた
`#[expected_fail(...)]` は、include する側のモジュールで解決されます。

```
// checks.gcl
param limit: Dimensionless = 100.0;
pub assert limit_positive = @limit > 0.0;
```

```
// main.gcl
include checks(limit: 50.0)::{ limit, limit_positive };

#[assumes(limit_positive)]
node ratio: Dimensionless = @limit / 2.0;
```

include の波括弧内でアサーションを選択すると、そのインスタンスローカルの名前が
`#[assumes(...)]` 用に公開されます。アサーションは `@` で参照されません。
ネストした include 内のアサーションも、外側の各具体的インスタンスの一部として
実行されます。したがって、2 つの別々の include は、束縛がたまたま等しい場合でも、
別々の結果を生成します。

## エラーコード { #error-codes }

| コード | 説明 |
|------|-------------|
| A001 | アサーションの失敗（LSP 診断） |
| A002 | 前提としたアサーションの失敗（CLI。影響を受けるノードを列挙） |
| A003 | `@` シジルで assert を参照することはできない |
| A004 | assert の本体は `Bool` に評価されなければならない |
| A005 | `#[assumes(...)]` 内の未知の assert |
| A006 | 無効な宣言種別（`const node` など）に対する `#[assumes]` |
| A007 | 未知の属性名 |
| A008 | 無効な宣言種別（`assert` 以外）に対する `#[expected_fail]` |
| A009 | `#[expected_fail(...)]` 内の無効な引数 |
| A010 | インデックスなしのアサーションに対するバリアント引数付きの `#[expected_fail(...)]` |
| A011 | インデックス付きのアサーションに対するキーなしの `#[expected_fail]` |
| A012 | `#[expected_fail(...)]` 内の重複キー |
| A013 | `#[expected_fail(...)]` のキーのインデックス形状が誤っている |
| A014 | `#[expected_fail(...)]` のキーが誤ったアサーションインデックスを使用している |
| A015 | `~=` アサーションのリテラル許容誤差が負 |
| A016 | `#[expected_fail(...)]` の Fin 位置がインデックス付きアサーションの軸の範囲外 |
| A017 | 無効な宣言種別に対する `#[hidden]` |
| A018 | plot 以外の include 項目に対する `#[hidden]` |
| A019 | 単一指定の `#[assumes]`、`#[expected_fail]`、または `#[hidden]` 属性の繰り返し |
| A020 | `#[assumes]` の引数リストが空 |
| A021 | `#[assumes(...)]` 内のアサーション名の重複 |
| A022 | `#[assumes(...)]` 内の識別子でない引数 |
| A023 | 予約された `#[lazy]` 構文はサポートされていない |

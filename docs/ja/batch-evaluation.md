---
icon: material/transit-connection-variant
---

# バッチ評価 { #batch-evaluation }

サンプリングによる検討、シナリオ探索、最適化では、1つのモデルを数千回以上評価します。Graphcal は検査済みのモデルをこうした解析に提供できます。モデルのコンパイルは1回だけで、クライアントがパラメータ値のバッチを送ると、行ごとに1つの結果が返ります。計算には `graphcal eval` と同じ型付き DAG を使います。

手順は次の3つです。

1. モデルのインターフェースを1回だけ取得する。
2. 変化させるパラメータと読み出す出力を選んで、セッションを開始する。
3. リクエストのバッチを送り、結果を読む。

## 1. モデルを記述する { #1-describe-the-model }

エントリファイルであれば、どのファイルでも使えます。値域（`min`、`max`）が公開されるので、サンプラーや最適化ツールは探索範囲を知ることができます。

```graphcal title="reliability.gcl"
pub index Mode = { Nominal, Degraded };

param load: Force(min: 0.0 N, max: 10_000.0 N);
param cycles: Int(min: 0, max: 100_000) = 10_000;
param mode: Key<Mode>;

assert load_positive = @load > 0.0 N;

pub node failure: Bool =
    @load > 8_000.0 N && @cycles > 50_000 && @mode == Mode#Degraded;
pub node margin: Force = 10_000.0 N - @load;
```

```bash
graphcal model describe reliability.gcl
```

このコマンドは、すべてのパラメータと public なノードを並べた JSON 文書を出力します。

```json
{
  "protocol_version": 2,
  "graphcal_version": "0.0.1-alpha.37",
  "interface_fingerprint": "sha256:…",
  "interface": {
    "parameters": [
      {
        "name": "load",
        "graphcal_type": "Force",
        "wire": { "kind": "quantity", "dimension": { "Length": 1, "Mass": 1, "Time": -2 }, "unit": "m*kg/s^2" },
        "domain": { "kind": "quantity", "min": 0.0, "max": 10000.0 },
        "has_default": false
      },
      …
    ],
    "outputs": [ … ]
  }
}
```

- `wire` は値の受け渡し方です（下の表を参照）。プロトコルで運べない型も一覧には載り、`"kind": "unsupported"` と理由が付きます。黙って除外されることはありません。
- `unit` はそのポートの SI の基準単位です。セッションでそのポートに別の単位を選ばない限り、値はこの単位でやりとりします（後述）。
- `domain` も同じ単位で表されます。インデックス付きのパラメータでは、すべての要素を制約します。値域外の値は、その行だけを失敗させます。

| Graphcal の型 | `kind` | 値 |
|---------------|--------|----|
| 物理量 | `quantity` | `unit` で表した数値 |
| `Complex<D>` | `complex` | `unit` で表した実部と虚部 |
| `Int`、`Bool` | `int`、`bool` | 整数または真偽値 |
| `Key<I>` | `key` | ヴァリアント名、0から数えた `Fin` の位置、または SI の座標 |
| `Datetime<S>` | `datetime` | J2000（2000-01-01T12:00:00）からの秒数。`S` で読む |
| `T[I]` | `indexed` | `I` の要素ごとに1つの `T`。宣言順に並ぶ |
| 構造体型・共用体型 | `algebraic` | 1つのコンストラクタとそのフィールド |

- `interface_fingerprint` はこのインターフェースを識別します。式を変えても指紋は変わりませんが、ポートの追加・削除・型の変更では変わります。

## 2. セッションを開始する { #2-start-a-session }

セッションでは、変化させるパラメータ（`--input`）と読み出す出力（`--output`）を指定します。変化させないパラメータはデフォルト値のままです。そのため、デフォルト値のないパラメータは必ず変化させる対象に含めます。

```bash
graphcal model serve reliability.gcl \
    --input load --input mode --output failure --output margin \
    --expect-fingerprint sha256:…
```

`--expect-fingerprint` を付けると、モデルが記述時から変わっていた場合にサーバは起動を拒否します。手作業でサーバを起動するときは、代わりに `--skip-fingerprint-check` を付けます。どちらか一方の指定が必須です。

物理量を別の単位で送受信するには、そのポートに `--unit NAME=UNIT` を付けます。単位は serve するファイルの中で `->` の後に書いた場合と同じように解決される単位式です。ポートの次元を測る単位で、かつ倍率が固定されている必要があります。基本単位、`const` 単位、プレリュードの単位は使えますが、実行時単位は使えません。セッションの値域は選んだ単位に換算されます。

```bash
graphcal model serve reliability.gcl \
    --input load --input mode --output failure --output margin \
    --unit load=kN --unit margin=kN \
    --expect-fingerprint sha256:…
```

サーバは stdin からリクエストのバッチを読み、stdout に結果を Arrow IPC ストリームとして書きます。端末から直接操作するものではなく、クライアントから駆動します。

## 3. Rust から評価する { #3-evaluate-from-rust }

`graphcal-model-client` crate は2つのコマンドを実行し、Arrow を内部に隠します。ハンドルには型（`Quantity`、`Complex`、`Int`、`Bool`、`Key`、`Datetime`、`Indexed<K>`、`Algebraic`）が付いているので、各列にはそのポートの種類の値しか渡せません。

```rust
use graphcal_model_client::{Bool, GraphcalCommand, Key, Model, Quantity};
use graphcal_model_protocol::{RowOutcome, UnitSpelling};

let model = Model::describe(GraphcalCommand::new("reliability.gcl"))?;
let load = model.input::<Quantity>("load")?;
let mode = model.input::<Key>("mode")?;
let failure = model.output::<Bool>("failure")?;

let mut session = model
    .session()
    .vary_in(&load, UnitSpelling::new("kN")?)
    .vary(&mode)
    .output(&failure)
    .spawn()?;
let degraded = mode.variant("Degraded")?;
let request = session
    .batch()
    .column(&load, [2.0, 9.0])?
    .column(&mode, [degraded.clone(), degraded])?
    .build()?;
let result = session.evaluate(&request)?;
for outcome in result.outcomes() {
    match outcome {
        RowOutcome::Success(row) => println!("failure: {}", row.get(&failure)?),
        RowOutcome::Failure(failure) => println!("failed: {}", failure.message()),
    }
}
session.shutdown()?;
```

[Tenax](https://github.com/shunichironomura/tenax) はこのクライアントを使って Graphcal モデルをサンプリングし、PRIM によるシナリオ探索を行います。

## ラッパーファイルでパラメータを固定する { #fix-parameters-with-a-wrapper-file }

一部のパラメータをデフォルト以外の値に固定したいときは、モデルを include する小さなラッパーを書き、そのラッパーを serve します。別のファイルを `include` するにはパッケージが必要です（[複数ファイルのプロジェクト](language/multi-file.md)を参照）。ここでは、モデルを `plant` という名前のパッケージの `src/plant/reliability.gcl` に置いています。

```graphcal title="src/plant/aging.gcl"
import plant.reliability::{ index Mode };

param load: Force(min: 0.0 N, max: 10_000.0 N);

include plant.reliability(load: @load, cycles: 80_000, mode: Mode#Degraded)
    ::{ pub failure, pub margin };
```

固定値は通常の Graphcal の式なので、単位と型は他の束縛と同じように検査されます。波括弧のリストで `pub` を付けた include の出力は再エクスポートされ、ラッパー自身で宣言した `pub node` と同じく出力になります。

`include plant.reliability(...) as r;` のように、波括弧のリストの代わりに別名を付けて include すると、モデルの public なノードはすべて、別名の下でラッパーの出力になります（`r::failure` と `r::margin`）。

## 行ごとの結果 { #row-outcomes }

リクエストの各行は、リクエストと同じ順序で必ず1つの結果を返します。

| 結果 | 発生する条件 | 出力 |
|------|--------------|------|
| 成功 | 行を評価できた | 選んだすべての出力 |
| アサーション不成立 | `assert` が成り立たなかった。最適化ではその行を実行不可能解として扱える | なし |
| 値域外の入力 | 値がパラメータの `min`/`max` の範囲外 | なし |
| 評価エラー | ゼロ除算など、それ以外の実行時の失敗 | なし |

ストリームの形式不正、セッションと異なるリクエストのスキーマ、NaN や無限大の値、インデックスにないキーはプロトコルエラーです。このときサーバは行の結果を返さず、終了コード `1` で停止します。

## 別のクライアントを書く { #writing-another-client }

他の言語のクライアントは `graphcal model serve` と直接やりとりします。

1. `graphcal model describe` を実行し、ポートを選ぶ。
2. `--input`、`--output`、必要な `--unit`、`--expect-fingerprint` を付けて `graphcal model serve` を起動する。
3. stdin に書き込む前に、stdout から結果ストリームのスキーマを読む。スキーマ全体のメタデータの `si_scales` には、選んだ各単位が SI の基準単位でいくつ分かが載っている。リクエストのスキーマにも同じメタデータが入り、その値域は選んだ単位に換算されている。
4. stdin に1本のリクエストストリームを書く。リクエストのバッチを1つ送るごとに、結果のバッチを1つ読む。
5. リクエストストリームを閉じると、サーバは結果ストリームを終えて終了コード `0` で終了する。

スキーマには `graphcal` というメタデータキーが1つだけあり、その値は JSON です。リクエストストリームには、変化させるパラメータごとに null を許さないフィールドが宣言順に並びます。結果ストリームには2つの struct フィールドがあります。

| フィールド | 子フィールド |
|------------|--------------|
| `outputs` | 読み出す出力ごとに null を許すフィールドが宣言順に並ぶ。失敗した行でだけ null になる |
| `outcome` | `status`（`UInt8`：`0` 成功、`1` アサーション不成立、`2` 値域外の入力、`3` 評価エラー）、`subject`（不成立のアサーション名またはパラメータ名）、`message` |

物理量と日時は `Float64`、`Int` は `Int64`、`Bool` は `Boolean` です。名前付きインデックスのキーは `Dictionary<Int32, Utf8>` で、辞書のコードではなく辞書の値で解決されます。`Fin(N)` のキーは `UInt32` の位置です。座標インデックスのキーは SI の `Float64` 座標で、インデックスの座標のどれかと厳密に一致しなければなりません。複素数は `re` と `im` の struct、インデックス付きの値は軸の要素ごとに1項目を持つ `FixedSizeList` です。構造体型はそのフィールドの struct です。共用体は `constructor` キーと `variants` struct からなる struct で、`variants` はコンストラクタごとに null を許す子フィールドを持ち、その行のコンストラクタの子だけが null になりません。

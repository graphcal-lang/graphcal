---
icon: material/numeric-6-circle
---

# ステップ 6: インデックス付き値 { #step-6-indexed-values }

このステップでは、複数のマヌーバーを含むデルタ V 予算のように、複数の関連する値を扱うためにインデックス付きコレクションを使います。

## インデックスの定義 { #defining-an-index }

`index` 宣言は有限のラベル集合を定義します。

```
index Maneuver = { Departure, Correction, Insertion };
```

## インデックス付き値 { #indexed-values }

インデックス付き値を宣言するには `[IndexName]` を使います。

```
node delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.12 km/s,
    Maneuver#Insertion: 1.83 km/s,
};
```

インデックス内の各ラベルがそれぞれ独自の値を持ちます。

## 要素への直接アクセス { #direct-element-access }

特定の要素には `[Index#Label]` でアクセスします。

```
node departure_dv: Velocity = @delta_v[Maneuver#Departure];
```

## `for` 内包 { #for-comprehensions }

インデックス付き値の各要素を `for` で変換します。

```
node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};
```

これにより、各要素が 2 倍された新しいインデックス付き値が生成されます。

## 集約 { #aggregations }

ランク 1 のインデックス付き量を 1 つの量に縮約します。`count` はインデックス付きでない任意の要素型に対して機能し、`Int` を返します。

```
node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node min_dv: Velocity = minimum(for m: Maneuver { @delta_v[m] });
node mean_dv: Velocity = mean(for m: Maneuver { @delta_v[m] });
node n_maneuvers: Int = count(for m: Maneuver { @delta_v[m] });
node normalized: Velocity = @total_dv / to_float(@n_maneuvers);
```

利用可能な集約関数: `sum`、`maximum`、`minimum`、`mean`、`count`。スカラー量の計算で整数のカウントが必要な場合は、明示的に `to_float` を使ってください。複数軸に対する直接の集約はまだ定義されていません。

## Scan (累積フォールド) { #scan-cumulative-fold }

`scan` はインデックスに沿った累積的な蓄積を計算します。

```
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
```

これは次の結果を生成します。

- `Departure`: 2.46 km/s
- `Correction`: 2.58 km/s (2.46 + 0.12)
- `Insertion`: 4.41 km/s (2.58 + 1.83)

蓄積は、`index Maneuver = { Departure, Correction, Insertion };` でラベルが宣言された順序に従います。マップリテラルがたまたまエントリーを並べた順序ではありません。軸に沿って `scan` する予定がある場合は、意味のある順序でラベルを宣言してください。

`scan` のソースはちょうど 1 つの軸を持ちますが、そのアキュムレーターはそれ自体がインデックス付きのベクトルや行列であっても構いません。その場合、結果ではソースの軸がアキュムレーターの軸の前に付加されます。[インデックス付き漸化状態](../language/indexes.md#indexed-recurrence-state)を参照してください。

## 完全な例 { #complete-example }

```
index Maneuver = { Departure, Correction, Insertion };

node delta_v: Velocity[Maneuver] = {
    Maneuver#Departure: 2.46 km/s,
    Maneuver#Correction: 0.12 km/s,
    Maneuver#Insertion: 1.83 km/s,
};

node double_dv: Velocity[Maneuver] = for m: Maneuver {
    @delta_v[m] * 2.0
};

node total_dv: Velocity = sum(for m: Maneuver { @delta_v[m] });
node max_dv: Velocity = maximum(for m: Maneuver { @delta_v[m] });
node n_maneuvers: Int = count(@delta_v);
node cumulative_dv: Velocity[Maneuver] = scan(@delta_v, 0.0 m/s, |acc, item| acc + item);
node departure_dv: Velocity = @delta_v[Maneuver#Departure];
```

## ブラウザーで試す { #try-it-in-your-browser }

プレイグラウンドではインデックス付き値を展開できます。1 次元の値はキーと値のリスト、2 次元の値は表、3 つ以上の軸を持つ値は先頭の軸で選択される表の順序付きリストとして表示されます。マヌーバーの値を 1 つ変更して、集約と累積の出力が更新されるのを確認してください。

[このサンプルをプレイグラウンドで開く](https://graphcal.org/playground/?example=indexed)か、[ソースを読む](/docs/assets/playground/examples/step-6/main.gcl)ことができます。

期待される初期出力には `total_dv = 4.41 km/s` と 3 つの累積エントリーが含まれます。

## 学んだこと { #what-you-learned }

- 有限のラベル集合のための **`index`** 宣言
- `Type[Index]` 構文による**インデックス付き値**
- 各要素を変換する **`for` 内包**
- **集約**: `sum`、`maximum`、`minimum`、`mean`、`count`
- 累積フォールドのための **`scan`**
- 任意のインデックスで機能する**集約関数**

## 次は？ { #whats-next }

おめでとうございます！チュートリアルを完了しました。これで Graphcal の中核機能を理解できました。

より深く理解するには、すべての機能の正式なドキュメントである[言語リファレンス](../language/index.md)を参照するか、すべてのコマンドラインオプションが載っている [CLI リファレンス](../cli-reference.md)を確認してください。

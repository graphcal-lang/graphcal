---
icon: material/numeric-3-circle
---

# ステップ 3: 代数的型 { #step-3-algebraic-types }

このステップでは、関連する値をレコード形の代数的型にまとめる方法を学びます。

## レコード形の代数的型 { #record-shaped-algebraic-types }

計算が複数の関連する値を生成する場合、それらを 1 つのコンストラクターを持つ型にまとめます。Graphcal の代数的型の概念は 1 つだけです。同じ `type` 宣言が、レコード形のデータのための 1 つのコンストラクターを持つことも、選択肢のための複数のコンストラクターを持つこともできます。レコード形のデータでは、慣例としてコンストラクターに型と同じ名前を付けます。

```
dim GravParam = Length^3 / Time^2;

type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}
```

## 代数的値の構築 { #constructing-an-algebraic-value }

コンストラクターを呼び出して値を構築します。

```
node result: TransferResult = TransferResult(
    dv1: 100.0 m/s,
    dv2: 200.0 m/s,
    total_dv: 300.0 m/s,
    tof: 3600.0 s,
);
```

グラフノードのフィールドは明示的な `@` 参照で渡します。

```
node dv1: Velocity = 100.0 m/s;
node dv2: Velocity = 200.0 m/s;
node total_dv: Velocity = @dv1 + @dv2;

node result: TransferResult = TransferResult(
    dv1: @dv1,
    dv2: @dv2,
    total_dv: @total_dv,
    tof: 3600.0 s,
);
```

コンストラクターのフィールドは常に明示的でなければなりません: `field: expr`。

## フィールドアクセス { #field-access }

レコード形の代数的値のフィールドには `.` 演算子でアクセスします。

```
node total: Velocity = @result.total_dv;
node time_hours: Time = @result.tof -> h;
```

## まとめ: ホーマン遷移 { #putting-it-together-hohmann-transfer }

同一平面上の 2 つの円軌道間のホーマン遷移は、自然に複数の関連する値を生成します。各中間値をそれぞれ独立した `node` として表現し、出力を `TransferResult` にまとめます。

```
dim GravParam = Length^3 / Time^2;

type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}

const node r_earth: Length = 6371.0 km;
const node gm_earth: GravParam = 3.986004418e5 km^3/s^2;

param parking_alt: Length = 200.0 km;
param target_alt: Length = 35786.0 km;

node r1: Length = @r_earth + @parking_alt;
node r2: Length = @r_earth + @target_alt;
node a: Length = (@r1 + @r2) / 2.0;

node v1: Velocity = sqrt(@gm_earth / @r1);
node v2: Velocity = sqrt(@gm_earth / @r2);
node dv1: Velocity = sqrt(2.0 * @gm_earth * @r2 / (@r1 * (@r1 + @r2))) - @v1;
node dv2: Velocity = @v2 - sqrt(2.0 * @gm_earth * @r1 / (@r2 * (@r1 + @r2)));

node transfer: TransferResult = TransferResult(
    dv1: @dv1,
    dv2: @dv2,
    total_dv: @dv1 + @dv2,
    tof: PI * sqrt(@a ^ 3 / @gm_earth),
);

node total_dv: Velocity = @transfer.total_dv;
node tof_hours: Time = @transfer.tof -> h;
```

## ブラウザーで試す { #try-it-in-your-browser }

構造化された `transfer` の値は出力ペインで展開でき、`total_dv` と `tof_hours` はその射影されたフィールドを表示します。

[このサンプルをプレイグラウンドで開く](https://graphcal.org/playground/?example=structs)か、[ソースを読む](/docs/assets/playground/examples/step-3/main.gcl)ことができます。

期待される初期出力には、展開可能な `transfer` の値、`total_dv`、`tof_hours` が含まれます。

すべての中間値は DAG 内の第一級のノードです。そのため、LSP のアウトラインや `graphcal eval` の出力で可視化されます。これが、Graphcal がローカルなブロックの中に中間値を隠すのではなく公開している理由の 1 つです。

[ステップ 4](step4-functions.md) では、このグラフの一部を再利用可能でパラメーター化された `dag` ブロックにまとめる方法を学びます。

## 学んだこと { #what-you-learned }

- 1 つ以上のコンストラクターを持つ **`type`** 宣言
- `ConstructorName(field: value, ...)` による**コンストラクター呼び出し**
- レコード形の単一コンストラクター値に対する `.` による**フィールドアクセス**

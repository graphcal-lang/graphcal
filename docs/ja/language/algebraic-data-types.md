---
icon: material/shape
---

# 代数的データ型 { #algebraic-data-types }

graphcal において、本体を持つすべての `type` 宣言は、1 つ以上のコンストラクターを持つ 1 つの名前的（nominal）な**代数的データ型**を定義します。コンストラクターの数によって別々の型カテゴリーが生まれることはありません。単一コンストラクターのレコード状のデータも、複数コンストラクターによる選択肢も、同じ宣言と型の意味論を用います。

## コンストラクター { #constructors }

型は、波括弧で囲まれた本体の中に**コンストラクター**を列挙します。各コンストラクターは、括弧で囲まれた省略可能なペイロードを持つか、ペイロードを持たないユニットコンストラクターです。波括弧は型本体を区切るものであり、ペイロードの別形式ではありません。

```
type ManeuverKind {
    Impulsive(delta_v: Velocity),
    LowThrust(thrust: Force, duration: Time),
    Coast,
}
```

外側の波括弧は宣言のメンバー本体であるため、`type T { ... }` は `=` も末尾のセミコロンも使いません。これは `index I = { ... };` のような名前付きの右辺定義とは意図的に異なります。そちらは右辺が波括弧で囲まれていても `= rhs;` を使います。

コンストラクターは Term 名前空間に属し、これは型の名前空間とは別のものです。1 つの字句が型とコンストラクターの両方の名前になっても曖昧さは生じません。コンストラクターは、定数 `PI`、`E`、`TAU`、`sum` のような組み込み関数、`scan` のような文脈依存の呼び出し可能要素といった組み込み Term と同じ綴りを再利用することはできません。コンストラクターがペイロードフィールドを持つかどうかにかかわらず、その綴りは組み込みが所有します。`UTC` のようなタイムスケールは Term ではなく Static なアトムであるため、コンストラクターの綴りとして再利用できます。

ペイロードのフィールド名は 1 つのコンストラクター内で一意でなければなりません。各ペイロードスキーマは独立に検査されるため、同じフィールド名を異なるコンストラクターで使うことはできます。

値は、その正準的な名前的型とコンストラクターメンバーの両方を保持します。等値比較と `match` は表示名ではなくこれらの同一性を用います。インポートエイリアスを変更しても同一性は変わらず、異なる名前的型に属する同じ綴りのコンストラクターは区別されたままです。

## レコード状の単一コンストラクター型 { #record-shaped-one-constructor-types }

型がちょうど 1 つのコンストラクターを持ち、そのコンストラクターが型と同じ名前を持つとき、その型はレコード状（record-shaped）です。

```
type TransferResult {
    TransferResult(dv1: Velocity, dv2: Velocity, total_dv: Velocity, tof: Time),
}
```

### 構築 { #construction }

構築は常にコンストラクター呼び出し、すなわち名前付き引数を伴う括弧で行います。

```
node result: TransferResult = TransferResult(
    dv1: 100.0 m/s,
    dv2: 200.0 m/s,
    total_dv: 300.0 m/s,
    tof: 3600.0 s,
);
```

コンストラクターのフィールドは常にフィールド名と値を指定しなければなりません。グラフのノードは `@` で明示的に参照します。

```
node dv1: Velocity = 100.0 m/s;
node result: TransferResult =
    TransferResult(dv1: @dv1, dv2: 200.0 m/s, total_dv: 300.0 m/s, tof: 3600.0 s);
```

### フィールドアクセス { #field-access }

レコード状の値ではフィールドアクセスが機能します。コンストラクターがちょうど 1 つで、その名前が型と一致するため、ペイロードのフィールド集合が一意に定まるからです。

```
node total: Velocity = @result.total_dv;
node time_hours: Time = @result.tof -> h;
```

複数のコンストラクターを持つ型や、コンストラクター名が型名と異なる単一コンストラクター型では、フィールドアクセスは拒否されます。代わりに `match` で分解してください。

公開出力の射影も、すべての代数的値をその正準コンストラクタースキーマを通して解決します。フィールドの次元と表示メタデータはフィールド値とともに保持されます。内部でコンストラクターやフィールドの不一致があった場合は、宣言された単位なしでフィールドを黙って表示するのではなく、エラーとして報告されます。

## ユニットマーカー { #unit-markers }

ユニットマーカーは、コンストラクターがペイロードを取らない単一コンストラクター型です。

```
type Eci { Eci }
type Body { Body }
type Coasting { Coasting }
```

ユニットマーカーはファントム型パラメーター（例えば座標系）として役立ちます。

> **注意**: `type T;`（セミコロンのみで本体なし）はユニットマーカー**ではありません**。これは
> インポート側が束縛しなければならない*必須*の型を宣言します。
> [マルチファイルプロジェクト → 可視性、束縛可能性、入力ポート](multi-file.md#visibility-bindability-and-input-ports)を参照してください。

### 代数的値の構築 { #constructing-algebraic-values }

バリアントはそのコンストラクター名で構築します。別のモジュールが同名のコンストラクターをエクスポートしている場合は、モジュールエイリアスでコンストラクターを修飾します（例えば `rocket::LowThrust(...)`）。ペイロードを持つコンストラクターは名前付き引数を伴う括弧を使います。ユニットコンストラクターはフィールドを持たず、括弧を省略しなければなりません。`Coast()` はコンパイルエラー（`S010`）です。

```
node maneuver: ManeuverKind = LowThrust(thrust: 0.5 N, duration: 3600.0 s);

node coast: ManeuverKind = Coast;
```

## match 式 { #match-expressions }

代数的値の分解には `match` を使います。`match` は閉じたコンストラクター集合に対する網羅的な場合分けのために予約されています。通常の真偽値の述語や比較には `if` を使ってください。

```
node fuel_proxy: Force = match @maneuver {
    Impulsive(delta_v: _) => 0.0 N,
    LowThrust(thrust: thrust, duration: _) => thrust,
};
```

- 各アームはコンストラクターパターン（修飾なし、またはモジュール修飾付き）を使い、そのフィールドを束縛します
- `_` はフィールド値を破棄します
- ペイロードフィールドを持つコンストラクターでは、すべてのフィールド束縛を `field: variable` または `field: _` のように明示しなければなりません。束縛リストの省略は許されません
- ユニットコンストラクターはペイロードを持たず、`Coast` のようにフィールドなしの綴りを使わなければなりません。`Coast()` はコンパイルエラー（`S010`）です
- 名前付きインデックスのラベルも、`Maneuver#Departure` や `mission::Maneuver#Departure` のような修飾付きでフィールドなしのパターンにより網羅的にマッチできます

### 網羅性検査 { #exhaustiveness-checking }

コンパイラーはすべてのコンストラクターが網羅されていることを要求します。

```
type Status {
    Nominal,
    Warning(code: Dimensionless),
}

// ERROR: non-exhaustive -- missing `Warning` arm
node code: Dimensionless = match @status {
    Nominal => 0.0,
};
```

### すべてのアームが一致しなければならない { #all-arms-must-agree }

すべての match アームは同じ型と次元を生成しなければなりません。

```
// ERROR: arms have different dimensions (Force vs Velocity)
node bad: Force = match @maneuver {
    Impulsive(delta_v: delta_v) => delta_v,             // Velocity
    LowThrust(thrust: thrust, duration: _) => thrust,   // Force
};
```

## ジェネリック型 { #generic-types }

型は、次元、値型、インデックス、型レベルの自然数に対する、ソート（sort）を意識したジェネリックパラメーターを持つことができます。パラメーターはペイロードのフィールド型で使うことも、ファントムな区別のためだけに保持することもできます。

```
type Eci { Eci }
type Body { Body }

type Vec3<D: Dim, F: Type> {
    Vec3(x: D, y: D, z: D),
}
```

### ファントム型パラメーターの変更 { #changing-a-phantom-type-parameter }

ファントム型のキャスト演算子はありません。ファントム型パラメーターを変更する（例えば座標系のラベルを付け替える）には、新しいインスタンスを構築して各フィールドを明示的に代入します。

```
node pos_eci: Vec3<Length, Eci> = Vec3<Length, Eci>(x: 7000.0 km, y: 0.0 km, z: 0.0 km);
node pos_body: Vec3<Length, Body> = Vec3<Length, Body>(
    x: @pos_eci.x,
    y: @pos_eci.y,
    z: @pos_eci.z,
);
```

この冗長さは意図的なものです。ラベルの付け替えは、呼び出し箇所で目に見える、フィールドごとの意図的な行為であり、不透明なデータの黙った再解釈ではありません。

### ジェネリックのデフォルトと Nat 引数 { #generic-defaults-and-nat-arguments }

デフォルトはパラメーターの宣言されたソートに対して検査され、末尾の連続した並びを形成しなければならず、先行するパラメーターだけを参照できます。型注釈とコンストラクターは同じ引数構文を共有します。

```gcl
type Unframed { Unframed }

type Vec3<D: Dim, F: Type = Unframed> {
    Vec3(x: D, y: D, z: D),
}

// Equivalent to Vec3<Length, Unframed>
node pos: Vec3<Length> = Vec3<Length>(x: 1.0 m, y: 2.0 m, z: 3.0 m);

type Buffer<N: Nat = 3> {
    Buffer(value: Dimensionless),
}

param buffer: Buffer<3> = Buffer<3>(value: 1.0);
```

Nat 引数にはリテラル、スコープ内の Nat パラメーター、`+`、`*` を使えます。減算は意図的にサポートされていません。`Input<N + 1>` と `Output<N>` のように、大きい側を加算で表現してください。`Nat` 引数と `Index` 引数が暗黙に相互変換されることはありません。

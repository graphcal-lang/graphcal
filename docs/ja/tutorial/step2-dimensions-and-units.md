---
icon: material/numeric-2-circle
---

# ステップ 2: 次元と単位 { #step-2-dimensions-units }

このステップでは、計算に物理次元と単位を追加し、コンパイル時の次元解析を可能にします。

## 次元が重要な理由 { #why-dimensions-matter }

[マーズ・クライメイト・オービター](https://en.wikipedia.org/wiki/Mars_Climate_Orbiter)は、あるチームがヤード・ポンド法の単位を使い、別のチームがメートル法を使ったために失われました。Graphcal はコンパイル時に次元をチェックすることで、この種の誤りを防ぎます。

## 単位付きのロケット方程式 { #the-rocket-equation-with-units }

`rocket.gcl` を作成します。

```
// `Velocity` and `Acceleration` are prelude dimensions.

param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

ロケット方程式を編集し、単位や次元を変更してみてください。次元エラーは該当するソース範囲に対して報告されます。

[このサンプルをプレイグラウンドで開く](https://graphcal.org/playground/?example=units)か、[ソースを読む](/docs/assets/playground/examples/step-2/main.gcl)ことができます。

期待される初期出力には `delta_v = 3778.221 m/s` が含まれます。

```bash
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.221 m/s
```

## 次元の定義 { #defining-dimensions }

Graphcal には 8 つの組み込み基本次元があります。`Length`、`Time`、`Mass`、`Temperature`、`ElectricCurrent`、`Amount`、`LuminousIntensity`、`Angle` です。

プレリュードには `Velocity`、`Acceleration`、`Force`、`Energy` などの一般的な導出次元も用意されています。独自の導出次元は、プロジェクト固有の量の種類に対してのみ定義してください。

```
dim GravParam = Length^3 / Time^2;
dim Jerk = Length / Time^3;
```

## 単位の使用 { #using-units }

プレリュードには一般的な単位が用意されています。数値リテラルに単位を付けます。

```
param altitude: Length = 200.0 km;
param duration: Time = 3600.0 s;
const node speed_of_light: Velocity = 299792458.0 m/s;
```

### 利用可能なプレリュード単位 { #available-prelude-units }

| 次元 | 単位 |
|-----------|-------|
| Length | `m`, `km`, `cm`, `mm` |
| Time | `s`, `min`, `h` |
| Mass | `kg`, `g` |
| Temperature | `K` |
| ElectricCurrent | `A` |
| Amount | `mol` |
| LuminousIntensity | `cd` |
| Angle | `rad`, `deg` |
| Force | `N`, `kN` |
| Energy | `J`, `kJ` |
| Power | `W`, `kW` |
| Pressure | `Pa`, `kPa`, `MPa` |
| Frequency | `Hz` |

## カスタム単位の定義 { #defining-custom-units }

スケールがコンパイル時に確定しているカスタム単位には `const unit` を使います。

```
const unit mile: Length = 1609.344 m;
const unit mph: Velocity = 1.0 mile / h;
```

`h` はすでにプレリュード単位なので、`mph` の定義ではそのまま再利用できます。

## 単位変換 { #unit-conversion }

同じ次元の単位間で変換するには `->` 演算子を使います。

```
param altitude: Length = 200.0 km;
node altitude_in_meters: Length = @altitude -> m;
```

`->` 演算子は、変換元と変換先の単位が同じ次元を共有している場合にのみ機能します。`km` を `s` に変換しようとするとコンパイル時エラーになります。

## 次元チェック { #dimension-checking }

コンパイラーは、すべての式が次元的に整合していることを検証します。たとえば次のコードは、

```
param mass: Mass = 10.0 kg;
param length: Length = 5.0 m;
node bad: Mass = @mass + @length;  // ERROR!
```

`Mass` と `Length` を加算することはできないため、コンパイル時エラーになります。

## ユーザー定義の基本次元 { #user-defined-base-dimensions }

ドメイン固有の量のために、まったく新しい基本次元を定義できます。

```
base dim Information;
base unit bit: Information;
const unit byte: Information = 8.0 bit;
const unit kB: Information = 1000.0 byte;

dim Bandwidth = Information / Time;

node storage: Information = 500.0 kB;
node rate: Bandwidth = 100.0 bit / s;
node transfer_time: Time = @storage / @rate;
```

`base dim Information;` 宣言は新しい基本次元を作成します。その唯一の `base unit` 宣言が正準スケールを確立します。追加の単位はすべて、`const unit` または `unit` で明示的なスケールを定義しなければなりません。

## 学んだこと { #what-you-learned }

- 導出次元およびカスタム基本次元のための **`dim`** 宣言
- 数値リテラルへの**単位注釈** (`1200.0 kg`)
- コンパイル時カスタム単位のための **`const unit`** 宣言、および実行時に依存するスケールのための `unit`
- 単位変換のための **`->`** 演算子
- 単位の不一致を検出する**コンパイル時次元チェック**

## 次のステップ { #next-step }

[ステップ 3](step3-structs-and-blocks.md) では、代数的データ型で関連する値をまとめる方法を学びます。

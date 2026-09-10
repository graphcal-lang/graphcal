---
icon: material/toy-brick
---

# プラグインの作成 { #plugin-authoring }

!!! warning "実験的"
    プラグインシステムは実験的です: ABI、SDK マクロの表面、および
    このページの CLI コマンドは、どのリリースでも変更される可能性があります。
    [問題の報告](https://github.com/graphcal-lang/graphcal/issues)をお願いします。

このガイドでは、`graphcal-plugin` SDK を使って Rust で WASM プラグインを書く手順を、
スキャフォールドから、固定 (ピン留め) されて評価可能なモジュールになるまで順に説明します。
言語側の視点 (extern 関数の宣言と呼び出し、モジュール契約、
信頼ルール) については、
[Extern 関数](language/extern-functions.md)を参照してください。

graphcal プラグインは**純粋でサンドボックス化されたカーネルライブラリ**です: SI 量、
密な配列、レコード形状の結果に対する関数であり、その次元
シグネチャはすべての呼び出し箇所で graphcal コンパイラーによって検査されます。プラグインは、
`dag` ブロックでは表現できない計算 (反復ソルバー、特殊関数、
物性ライブラリ、座標変換) に適しています。プラグインは、その構造上、
ファイルシステムやネットワークに触れることができません。

## 1. スキャフォールド { #1-scaffold }

```bash
graphcal plugin new fluid-props
cd fluid-props
```

これにより、すぐにビルドできる Rust クレートが作成されます:

```text
fluid-props/
├── Cargo.toml            # cdylib + rlib, graphcal-plugin dependency
├── rust-toolchain.toml   # stable + the wasm32-unknown-unknown target
├── justfile              # `just build`, `just test`
├── src/lib.rs            # a plugin! block with sample kernels
└── README.md
```

## 2. 宣言と実装 { #2-declare-and-implement }

すべては 1 つの `plugin!` ブロック内に置かれます。シグネチャは graphcal の
extern 宣言構文で、本体は Rust で記述します:

```rust
graphcal_plugin::plugin! {
    /// Ideal-gas density of dry air.
    fn air_density(p: Pressure, t: Temperature) -> Mass / Volume {
        const R_SPECIFIC: f64 = 287.052874; // J/(kg*K)
        if t <= 0.0 {
            graphcal_plugin::fail!("temperature must be positive, got {t} K");
        }
        p / (R_SPECIFIC * t)
    }

    /// Linear interpolation, polymorphic over the dimension of `a`/`b`.
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D {
        (b - a).mul_add(t, a)
    }

    /// Arrays carry an ordered shape and flattened row-major values.
    fn share<D: Dim, I: Index>(xs: D[I]) -> Dimensionless[I] {
        let total: f64 = xs.iter().sum();
        let values = xs.iter().map(|x| x / total).collect();
        graphcal_plugin::Array::new(xs.shape().to_vec(), values)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }

    /// Result axes may reorder axes that parameters bind.
    fn transpose<D: Dim, I: Index, J: Index>(xs: D[I, J]) -> D[J, I] {
        let [rows, columns] = xs.shape() else {
            graphcal_plugin::fail!("transpose expects rank two");
        };
        let values = (0..*columns)
            .flat_map(|column| {
                (0..*rows).map(move |row| xs.values()[row * columns + column])
            })
            .collect();
        graphcal_plugin::Array::new(vec![*columns, *rows], values)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }

    /// Struct results are declared structurally; the macro generates a
    /// named `SpanOutput` type so same-kind fields cannot swap silently.
    fn span<I: Index>(xs: Pressure[I]) -> { lo: Pressure, hi: Pressure } {
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        SpanOutput { lo, hi }
    }
}
```

この単一の宣言から、マクロは wasm のエクスポート**と**
モジュールに埋め込まれるマニフェストを生成します。アリティ、パラメーターの順序、
次元シグネチャが互いにずれることはなく、シグネチャの行は
そのまま `.gcl` のインポート箇所に貼り付けられます。

### シグネチャの構文 { #signature-syntax }

パラメーターと結果の型は、`Bool`、`Int`、次元式、または
これらのスカラー種のいずれかを 1 つ以上の宣言されたインデックス変数上に並べた配列
(`flags: Bool[I]`、`counts: Int[I]`、`xs: D[I]`、`matrix: D[I, J]`) です。結果は、
波括弧で囲んだ構造体の形状
(`-> { lo: Pressure, hi: Pressure }`) でもかまいません。
次元式は次の語彙から構成されます:

| 語彙 | 名前 |
|------------|-------|
| 次元変数 | `<...>` 内の `D: Dim` 束縛子として関数ごとに宣言 |
| インデックス変数 | `<...>` 内の `I: Index` 束縛子として関数ごとに宣言 |
| プレリュードの基本次元 | `Length`, `Time`, `Mass`, `Temperature`, `ElectricCurrent`, `Amount`, `LuminousIntensity`, `Angle` |
| プレリュードの派生次元 | `Velocity`, `Acceleration`, `Force`, `Energy`, `Power`, `Frequency`, `Pressure`, `Area`, `Volume` |
| 空の積 | `Dimensionless` |

これらを `*`、`/`、括弧、および `^` による指数 (整数 (`^2`、
`^-3`) または括弧で囲んだ有理数 (`^(1/2)`、`^(-1/2)`)) で組み合わせます。派生名は
マニフェスト内で基本次元の指数に展開されるため、`Pressure` と
`Mass * Length^-1 * Time^-2` は同じ契約を宣言します。

2 つのルールがコンパイラーの検査を反映しています (違反はプラグインクレートの
コンパイルエラーとなり、graphcal 側の P005/P016 と同じ意味を
持ちます):

- すべての次元変数は、複合的な使用
  (`D^2`、`D1 * D2`、または結果) の前に、まず**素の**パラメーター型
  (`x: D`、または素の配列要素 `xs: D[I]`) として現れなければなりません。
- すべての結果配列の軸は、いずれかの配列パラメーターをインデックスするインデックス変数を
  再利用しなければならず、宣言されたすべてのインデックス変数はいずれかの配列パラメーターをインデックスしなければなりません。軸の
  並べ替えは可能ですが、プラグインが範囲 (extent) を新たに作り出すことはできません。
- 構造体形状のフィールドは具体的 (`Bool`、`Int`、固定された次元。
  次元変数は不可) であり、一意な名前を持ちます。
- 指数は非ゼロです。

構造体の形状は、Rust と `.gcl` の記述が異なる唯一の
箇所です: プラグインは形状を宣言し (graphcal の型名を
参照できません)、`.gcl` のインポート箇所ではスコープ内のレコード型を名前で指定します。その
フィールドは、名前、順序、種類において形状と一致しなければなりません。

### 本体の中で { #in-the-body }

パラメーターは、宣言された名前と自然な Rust の型で渡されます。
量は `f64`、`Bool` は `bool`、`Int` は `i64`、そして借用された型付き
配列ビュー `ArrayView<'_, f64>`、`ArrayView<'_, bool>`、
`ArrayView<'_, i64>` です。`ArrayView::shape()` はシグネチャの順序で範囲を返し、
`values()`、`get()`、`iter()` はその Rust 型の密な行優先の値を
公開します。配列を返す本体は、対応する検証済みの `Array<f64>`、
`Array<bool>`、`Array<i64>` を返します。その形状は、結果の軸リストによって束縛された
範囲と正確に一致しなければなりません。その他の結果は `f64`、`bool`、
`i64`、または生成された `...Output` 構造体です。配列を移動する関数も
ネイティブでコンパイルされるため、`cargo test` に wasm ツールチェーンは必要ありません。

生成される公開名は、Rust コードが出力される前に検査されます。`foo_bar` の構造体結果は
`FooBarOutput` を使用します。綴りが同じ出力名に潰れてしまう宣言
(たとえば `foo` と `foo_`) は、両方の関数スパンで
拒否されます。`GRAPHCAL_PLUGIN_MANIFEST` と
`GRAPHCAL_PLUGIN_MANIFEST_SECTION_IS_UNIQUE` は、呼び出し元の
モジュール内で予約されています。いずれかのシグネチャがバッファープロトコルを使用する場合、WebAssembly のエクスポート名
`graphcal_alloc`、`graphcal_free`、`memory` も予約されます。プライベートな
ラッパーのパラメーターとローカル変数は衛生的 (hygienic) であるため、通常のパラメーター名が
それらと衝突することはありません。

**量の値は常に SI 基本単位です。** `Pressure` パラメーターは
パスカルであり、`Velocity` の結果はメートル毎秒です。Graphcal はすべての呼び出し箇所で
次元を検査しますが、あなたの数式がパスカルをバールとして扱っているかどうかを
見ることはできません。この残存リスクはプラグインの内部に存在するため、
カーネルの数式は一貫して SI で記述してください。

次元変数はパラメトリックです: 本体は `D` がどの次元に
束縛されたかを知ることはないため、次元多相なカーネルは
次元に対して一様でなければなりません (補間は可、`D` の `sin` は不可)。

たとえば、`Bool[I]` の本体が生の数値バッファーを観測することは決してありません:

```rust
graphcal_plugin::plugin! {
    fn invert<I: Index>(values: Bool[I]) -> Bool[I] {
        let inverted = values.iter().map(|value| !value).collect();
        graphcal_plugin::Array::new(values.shape().to_vec(), inverted)
            .unwrap_or_else(|error| graphcal_plugin::fail!("{error}"))
    }
}
```

ラッパーは、この本体に入る前にすべての要素を検証し、デコードします。
量の要素は有限でなければならず、Bool の要素は数値の 0 または 1 でなければならず
(`-0.0` は false)、Int の要素はスカラーの exact-binary64
ポリシーを満たさなければなりません。結果の要素は同じポリシーの下でエンコードされ、検査されます。

### 失敗とパニック { #failures-and-panics }

ドメインの失敗に対しては `graphcal_plugin::fail!("...")` (または `fail(&str)`) を
呼び出します。メッセージは呼び出しを中断し、失敗したノードの
診断に表示されますが、無関係なノードは評価を続けます。Rust のパニック
(`assert!`、`unwrap`、算術チェックによるもの) は、パニックメッセージとともに同じ
チャネルを通じて転送されるため、匿名のトラップではなく
診断可能なものになります。wasm 以外のターゲットでは、どちらも通常のパニックです。

## 3. ネイティブでテストする { #3-test-natively }

`plugin!` の展開結果は wasm 以外では通常の Rust であるため、カーネルは
他のクレートとまったく同じように `cargo test` で単体テストできます。失敗は
`fail!` のメッセージを伴うパニックとして現れます:

```rust
#[test]
fn density_of_air_at_stp() {
    let rho = super::air_density(101_325.0, 288.15);
    assert!((rho - 1.225).abs() < 1e-3);
}
```

## 4. ビルドと検証 { #4-build-and-validate }

```bash
cargo build --release --target wasm32-unknown-unknown
graphcal plugin test target/wasm32-unknown-unknown/release/fluid_props.wasm \
    --call air_density 101325 288.15
```

`graphcal plugin test` は、読み込み時のすべての ABI 検査 (マニフェスト、インポートの
禁止、エクスポートの型) を実行し、モジュールの SHA-256 と**そのまま貼り付けられる
`import plugin` ブロック**を出力します。`--call` は、デフォルトの
燃料 (fuel) とメモリの制限の下で 1 つの関数を実行します。引数は SI 基本単位で、
`Bool` には `true`/`false`、`Int` には整数、配列には宣言されたランクを持つ
矩形の JSON 配列を指定します。配列の葉はその要素の種類に従います: ブール値
(`[true,false]`)、整数 (`[1,-2,3]`)、または SI の数値
(`[1.0,2.5,3]`、`[[1,2],[3,4]]`)。数値の `0`/`1` は Bool の
JSON 入力として受け付けられず、小数値は Int の入力として受け付けられません。
構造体を返す関数の場合、インポートブロックの
前に推奨されるレコード宣言が出力されます (自由に名前を変更してください。
ローダーは名前ではなく形状を比較します)。

本番のカーネルが正当にデフォルトの 100,000,000 燃料単位を超える量を
必要とする場合は、`graphcal.toml` で最も範囲の狭いプロジェクト/関数の上書きを
設定してください。スタンドアロンの `plugin test --call` は意図的に
ホストのデフォルトのままです。[プロジェクトの燃料ポリシー](language/extern-functions.md#project-fuel-policies)を参照してください。

## 5. ベンダリング、宣言、固定 { #5-vendor-declare-pin }

モジュールを graphcal プロジェクト (たとえば `plugins/`) にコピーし、宣言を
貼り付けて、固定します:

```text
import plugin "plugins/fluid_props.wasm" as fluids {
    fn air_density(p: Pressure, t: Temperature) -> Mass / Volume;
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}

node rho: Mass / Volume = fluids::air_density(@chamber_p, @chamber_t);
```

```bash
graphcal deps lock    # records the module's SHA-256 in graphcal.lock
```

ロックファイルが信頼境界です: プラグインのバイト列は、レビュー可能な
`graphcal.lock` の差分と一緒にしか変更できません。
[信頼: ロックファイルによる固定](language/extern-functions.md#trust-lockfile-pins)を参照してください。

## スコープと制限 (ABI v5) { #scope-and-limits-abi-v5 }

- 値は SI 量、`Bool`、`Int`、3 種類すべてのスカラー種の空でない多軸配列、
  および具体的なフィールドを持つレコード形状の結果です。まだ
  境界を越えられないもの: `Datetime` (明示的な
  `to_jd`/`from_jd` 形式の変換を使用してください)、構造体パラメーター、ジェネリックなレコード、
  および次元変数の構造体フィールド。
- プラグインごとに `plugin!` ブロックは 1 つです (2 つ目のブロックは
  重複シンボルエラーで wasm のリンクに失敗します)。ヘルパー関数はクレート内の
  どこにでも置けます。
- プラグインは何もインポートできません (SDK の失敗チャネルが唯一の
  例外で、自動的に接続されます)。そのため、I/O、スレッド、
  オペレーティングシステムの乱数を取り込むクレートは、読み込み時に拒否されます。モジュール内部で
  実装された純粋なシード付き擬似乱数生成器は引き続き有効です。
- 語彙は `.gcl` 側に置いてください: プラグインは単位、次元、
  型を定義できません。

## SDK を使わずに作成する { #authoring-without-the-sdk }

SDK は利便性のためのものであり、信頼モデルの一部ではありません。プラグインとは、
[モジュール契約](language/extern-functions.md#wasm-plugin-modules)を満たす
任意のコア wasm モジュールです: エクスポートの
wasm 型はそのシグネチャに従います (量/`Bool`/`Int` のスロットごとに 1 つの `f64`、
配列の軸ごとに `i32` ポインターとそれに続く 1 つの `i32` の範囲、そして
配列/構造体の結果に対する末尾の出力ポインター)。配列バッファーは、量には有限の `f64`、
`Bool` には数値の `0.0`/`1.0`、`Int` にはスカラーの損失のない
binary64 ポリシーを使用し、すべての要素は境界で検証されます。
モジュールはまた、バッファーが関係する場合には
`graphcal_alloc`/`graphcal_free` のペアを、そして
`graphcal-manifest` カスタムセクションを提供しなければならず、任意の
`graphcal::fail` 以外のインポートを持ってはなりません。Rust 以外のツールチェーンでは、
マニフェスト JSON を出力し (`graphcal-plugin-abi` クレートがモデルを
ドキュメント化し、ビルドツール向けに `embed_manifest` を提供しています)、結果を
`graphcal plugin test` で検証してください。

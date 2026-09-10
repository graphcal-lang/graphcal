---
icon: material/numeric-5-circle
---

# ステップ 5: 複数ファイルプロジェクト { #step-5-multi-file-projects }

このステップでは、`import` 宣言を使ってプロジェクトを複数のファイルに分割する方法を学びます。

## なぜ複数ファイルなのか { #why-multiple-files }

プロジェクトが大きくなるにつれて、関心事を分離することが役立ちます。

- **定数**を 1 つのファイルに (プロジェクト全体で共有)
- **パラメーター**を別のファイルに (見つけやすく調整しやすい)
- **主要な計算**をエントリーポイントに

## ファイルはパッケージである { #files-are-packages }

Graphcal では、すべての `.gcl` ファイルが**パッケージ**です。`graphcal.toml` マニフェストがない場合、そのファイルは*仮想*パッケージ、つまりスタンドアロンの Graphcal スクリプトです。パッケージはちょうど 1 つのモジュール、すなわちそのファイル自身を含みます。インラインの DAG からそのトップレベル宣言を自己参照することはできますが (たとえば `dynamics.gcl` の中から `import dynamics::{type T};`)、兄弟ファイルを import することは**できません**。どの Graphcal プロジェクトでも、複数ファイル化の最初のステップはマニフェストを追加することです。

言い換えると、仮想 = 1 ファイルです。2 つ目のファイルが欲しくなった時点で `graphcal.toml` を追加し、本物のパッケージに昇格させます。このステップの残りでは、その昇格を最初から最後まで順に説明します。

## プロジェクト構成 { #project-structure }

複数ファイルプロジェクトは、常にルートに `graphcal.toml` マニフェストを持ち、ソースファイルはパッケージのソースディレクトリの下に配置されます。

```text
rocket_project/
  graphcal.toml                # [package] name = "rocket_project"
  src/
    rocket_project/
      constants.gcl
      params.gcl
      main.gcl
```

### 任意のプラグイン実行ポリシー { #optional-plugin-execution-policy }

レビュー済みの WebAssembly カーネルを呼び出すプロジェクトは、マニフェストに上限付きの fuel 予算を設定することもできます。プロジェクト全体の `[plugins].fuel_per_call` の値は、ベンダリングされたすべてのプラグイン関数に適用されます。`[[plugins.function_limits]]` エントリーは、個別に識別された 1 つのプラグインパスと関数に対して上書きできます。優先順位、検証、および上限値については[プロジェクトの fuel ポリシー](../language/extern-functions.md#project-fuel-policies)を参照してください。

### `constants.gcl` { #constantsgcl }

```graphcal
pub const node g0: Acceleration = 9.80665 m/s^2;
```

### `params.gcl` { #paramsgcl }

```graphcal
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
```

### `main.gcl` { #maingcl }

```graphcal
import rocket_project.constants::{g0};
include rocket_project.params()::{dry_mass, fuel_mass, isp};

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

## 複数ファイルプロジェクトをローカルで試す { #try-the-multi-file-project-locally }

[スタンドアロンのプレイグラウンド](https://graphcal.org/playground/)は 1 つの `.gcl` ファイルにしか対応していません。この複数ファイルのレッスンには CLI を使ってください。別のブラウザー用サンプルとして平坦化されてはいません。

[エントリーソース](/docs/assets/playground/examples/step-5/src/rocket_project/main.gcl)、[定数](/docs/assets/playground/examples/step-5/src/rocket_project/constants.gcl)、[パラメーター](/docs/assets/playground/examples/step-5/src/rocket_project/params.gcl)、および[パッケージマニフェスト](/docs/assets/playground/examples/step-5/graphcal.toml)を読んでください。上に示したとおりに配置し、以下のコマンドを実行します。

期待される出力には、`g0` から `delta_v` までの 7 つの射影された値がすべて含まれます。

`::{...}` の前のパスは、パッケージルートからの絶対パスです。最初のセグメントはパッケージ名 (`graphcal.toml` から取得) で、後続のセグメントは `source_dir` の下のディレクトリツリーをたどります。

`params` には `import` ではなく `include` を使っている点に注意してください。`params.gcl` は `param` (実行時の値) を公開しており、実行時の値がファイルの境界を越えるのは DAG のインスタンス化を通じてのみです。`import` はコンパイル時の名前だけを持ち込みます。

## 複数ファイルプロジェクトの実行 { #running-a-multi-file-project }

`graphcal eval` にエントリーファイルを指定します。

```bash
$ graphcal eval rocket_project/src/rocket_project/main.gcl
g0         = 9.80665 m/s^2
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.221 m/s
```

Graphcal は各 `import` をパッケージツリーに対して解決します。

## `import` 文 { #the-import-statement }

3 つの形式があります。スコープに持ち込みたいものに合った形式を選んでください。

```graphcal
import rocket_project.constants;                  // brings module `constants`
import rocket_project.constants as c;             // brings module under alias `c`
import rocket_project.constants::{g0, g_mars};     // brings only `g0` and `g_mars`
```

実際には波括弧形式が最もよく使われます。インポートされるすべての名前が明示的になるためです。

## インポートのエイリアス { #import-aliasing }

2 つのファイルが同じ名前をエクスポートしている場合は、`as` で一方または両方を改名します。

```graphcal
import rocket_project.file_a::{velocity as velocity_a};
import rocket_project.file_b::{velocity as velocity_b};
```

モジュール全体にエイリアスを付けることもできます。

```graphcal
import very.long.package.path as p;
node y: Length = @p.helper(...)::result;
```

## インポートされるもの { #what-gets-imported }

`import` は**コンパイル時**の名前だけを持ち込みます。次元、単位、型、インデックスを選択的にインポートするには、明示的な `dim`、`unit`、`type`、`index` マーカーを使います。マーカーのない裸の項目は、定数、DAG、アサーション、コンストラクターなどの項を選択します。別のファイルの実行時の値 (`param` や `const` でない `node` など) を使うには、値を import する代わりに、それを生成する DAG を *include* してください ([複数ファイルプロジェクト](../language/multi-file.md#the-include-form)を参照)。

| 宣言の種類 | インポート方法 | 参照方法 |
|------------------|-------------------------------------|------------------|
| `const node`     | `import package.file::{name}`                | `@name`          |
| `dim`            | `import package.file::{dim DimName}`         | `DimName`        |
| `unit`           | `import package.file::{unit unit_name}`      | `unit_name`      |
| `type`           | `import package.file::{type TypeName}`       | `TypeName`       |
| `index`          | `import package.file::{index IndexName}`     | `IndexName`      |
| `dag`            | `import package.file::{dag_name}`            | `include` する、または `@dag_name(...)::out` として呼び出す |
| `assert`         | `import package.file::{assert_name}`         | `#[assumes(assert_name)]` |

## 単一ファイルで十分な場合 { #when-a-single-file-suffices }

計算全体が 1 つのファイルに収まるなら、マニフェストはまったく必要ありません。スタンドアロンの `rocket.gcl` スクリプトは仮想パッケージのように振る舞い、外部からアドレス可能な名前はそのファイル自身のステム (拡張子を除いたファイル名) だけです。インラインの DAG からトップレベル宣言への参照には、その自己参照パスを使います。

```graphcal
// rocket.gcl  (standalone script, no graphcal.toml)
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    import rocket::{type OrbitType};   // file's own name
    param o: OrbitType;
    // ...
}
```

2 つ目のファイルに分割した時点で、プロジェクトルートに `graphcal.toml` を追加し、上に示したように `<source_dir>/<pkg>/` の下にファイルを配置してください。兄弟ファイルの `import` は、パッケージの名前空間の中にないファイルからは明確なエラーで拒否されます。これには、`graphcal.toml` の隣にあってもその `<source_dir>/<pkg>/` ディレクトリの外にあるファイルも含まれます。

## 循環インポートの検出 { #circular-import-detection }

Graphcal はコンパイル時に循環インポートを検出します。2 つのモジュール `<pkg>.a` と `<pkg>.b` を持つ本物のパッケージでは、次のようになります。

```graphcal
// src/<pkg>/a.gcl
import <pkg>.b::{x};

// src/<pkg>/b.gcl
import <pkg>.a::{y};   // ERROR: circular import
```

## アサーションは明示的なインスタンスに属する { #assertions-belong-to-explicit-instances }

`import` はモジュールを評価せずにコンパイル時の名前を読み込むため、アサーションを実行しません。明示的な `include` インスタンスはそれぞれ、そのインスタンスの束縛で自身のアサーションを実行します。`#[assumes(...)]` でアサーションの名前が必要な場合は、インクルードの波括弧の中でそのアサーションを選択してください。詳細は[アサーション](../language/assertions.md#assertions-in-multi-file-projects)を参照してください。

## 学んだこと { #what-you-learned }

- すべての `.gcl` ファイルは**パッケージ**であり、仮想 (単一ファイルのスタンドアロンスクリプト) または本物 (マニフェストに裏付けられた複数ファイルプロジェクト) のいずれかです。
- 仮想パッケージはちょうど 1 つのファイルを持ちます。複数ファイルプロジェクトは常に `graphcal.toml` を持ちます。
- 3 つの `import` 形式 (裸、エイリアス付き、波括弧リスト) は、書いたとおりの名前だけをスコープに持ち込みます。
- `import` はコンパイル時の名前のためのものであり、実行時の値は `include` を通じてファイルの境界を越えます。
- 循環インポートは自動的に検出され、アサーションは明示的なインクルードインスタンスに対して実行されます。

## 次のステップ { #next-step }

[ステップ 6](step6-indexed-values.md) では、複数要素の計算のためにインデックス付きコレクションを扱います。

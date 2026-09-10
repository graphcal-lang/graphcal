---
icon: material/transit-connection-variant
---

# Tenax 連携 { #tenax-integration }

Graphcal は、検査済みの工学計算を永続的な
[Tenax](https://github.com/shunichironomura/tenax) モデルとして提供できます。プロセスは
一度だけコンパイルし、stdin で Arrow レコードバッチを受け取り、`graphcal eval` で使用されるものと
同じ型付き DAG を通じて各行を評価し、stdout に Arrow の結果を書き出します。

初期のインターフェースは意図的に厳格です: Tenax stdio プロトコルバージョン 1
と Arrow スキーマバージョン 2 です。サポートされていない Graphcal の値は、明示的な型なしに
平坦化、リネーム、変換されるのではなく、起動時に拒否されます。

## 1. モデルを定義する { #1-define-a-model }

すべての数値入力に有限のサンプリング領域を宣言し、各カテゴリカル入力には具体的な
名前付きインデックスを使用し、選択した Boolean 出力を公開 (public) にします:

```graphcal title="reliability.gcl"
pub index Mode = { Nominal, Degraded };

param load: Force(min: 0.0 N, max: 10_000.0 N);
param cycles: Int(min: 0, max: 100_000);
param mode: Key<Mode>;

pub node failure: Bool =
    @load > 8_000.0 N
    && @cycles > 50_000
    && @mode == Mode#Degraded;
```

上記のインターフェースは次のように発見されます:

- `load`: 連続値 `Float64`、範囲 `0..10000`、単位 `N`
- `cycles`: 整数 `Int64`、範囲 `0..100000`
- `mode`: `Dictionary<Int32, Utf8>`、カテゴリは辞書順で `Degraded`、`Nominal`
- `failure`: Boolean 出力

辞書コードは意味的な同一性ではありません。Graphcal は常にカテゴリカルな要求を
辞書の**値**によって解決するため、送信側は有効な任意の順序で
コードを割り当てることができます。

## 2. 永続サーバーを起動する { #2-start-the-persistent-server }

```bash
graphcal model serve reliability.gcl --output failure
```

複数の公開 Boolean ノードを選択するには `--output` を繰り返します。発見は、
フラグが別の順序で指定された場合でも、ソースの宣言順を保持します。

このコマンドを対話的なテキストプログラムとして実行しないでください。stdout は 2 つの
連結された Arrow IPC ストリームで構成され、Tenax または他の
プロトコル互換クライアントに接続されている必要があります。ログとエラーは stderr を使用します。

## 3. Tenax から起動する { #3-spawn-it-from-tenax }

Rust の Tenax クライアントは、コマンドを起動し、発見結果を調べ、複数の要求に対して
同じ子プロセスを再利用できます:

```rust
use std::process::Command;
use tenax::{
    EvalRequest, EvaluationId, Evaluator, Feature, InputChunk, StdioEvaluator,
};

let mut command = Command::new("graphcal");
command.args([
    "model", "serve", "reliability.gcl",
    "--output", "failure",
]);
let evaluator = StdioEvaluator::spawn(command)?;
let schema = evaluator.schema();

let inputs = InputChunk::new(
    schema,
    vec![
        Feature::continuous("load", vec![2_000.0, 9_000.0])?,
        Feature::integer("cycles", vec![10_000, 80_000])?,
        Feature::categorical(
            "mode",
            vec!["Nominal".to_owned(), "Degraded".to_owned()],
        )?,
    ],
)?;
let request = EvalRequest::new(EvaluationId::new(1), 42, inputs);
let result = evaluator.evaluate(vec![request]).next().unwrap()?;

// More evaluate(...) calls reuse the compiled Graphcal process.
let status = evaluator.shutdown()?;
assert!(status.success());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Graphcal モデルは決定論的であるため、要求のシードはプロトコルの整合性のために
検証されますが、それ以外では無視されます。

## 4. サンプリングして PRIM を実行する { #4-sample-and-run-prim }

Tenax のサンプリングと解析は、発見された領域を直接使用します:

```rust
use tenax::{
    Evaluator, Objective, Prim, PrimConfig, evaluation_to_dataset,
    sample_latin_hypercube,
};

let request = sample_latin_hypercube(evaluator.schema(), 5_000, 0x5eed, 1)?;
let retained = request.clone();
let result = evaluator.evaluate(vec![request]).next().unwrap()?;
let failure = evaluator.schema().output_position("failure")?;
let dataset = evaluation_to_dataset(
    evaluator.schema(), retained, result, failure,
)?;
let config = PrimConfig::new(0.05, 0.05, 0.05, Objective::Lenient1)?;
let discovered = Prim::new(&dataset, config).find_box();
# Ok::<(), Box<dyn std::error::Error>>(())
```

終了時には必ず `shutdown()` を呼び出してください。不完全な結果イテレーターをドロップすると
ストリームの再利用が曖昧になるため、Tenax はその子プロセスを終了させます。

## サポートされるインターフェース { #supported-interface }

### 入力 { #inputs }

| Graphcal の型 | 要件 | Arrow の型 |
|---------------|-------------|------------|
| 量 / `Dimensionless` | 有限の閉じた `min` と `max`。有限の幅 | `Float64` |
| `Int` | 閉じた `i64` の `min` と `max` | `Int64` |
| `Key<I>` | 具体的で空でない名前付きインデックス | `Dictionary<Int32, Utf8>` |

次元を持つ量は、正規のスケール 1 の SI 単位式で公開されます。
範囲と要求の値はその同じ単位を使用します。`Dimensionless` は単位
メタデータを省略します。すべての Tenax v2 要求は発見されたすべての入力を含むため、
デフォルト値を持つパラメーターも入力のままです。

### 出力 { #outputs }

選択された各値は、直接宣言され、明示的に公開された、スカラーの
`Bool` ノードでなければなりません。Graphcal はソースの順序を保持し、入力/出力インターフェース全体にわたって
重複する名前を拒否します。

Graphcal の準備済み評価器自体は、Boolean、日時、複素数、
代数的、キー、再帰的にインデックスされた束縛と出力もサポートしています。直接的および
相互に再帰的な代数的型は、有限の型付き定義グラフとして保持されるため、
通常の評価と有限のパラメーター値は Arrow スキーマに
依存しません。これらのより豊かな型のファミリーは、スキーマ v2 を通じて黙って強制されることはありません:
再帰的な入力は Tenax モデル投影の際に型付きのモデル定義エラーを受け取り、
すでに準備された Graphcal プロジェクトは有効なままです。それらをエンコードするには、
将来の再帰的 Arrow プロトコルが必要です。

## 失敗時の動作 { #failure-behavior }

1 つの有効な要求バッチは、同じ行数、ID、行順序を持つ正確に 1 つの
結果バッチを生成します。

- 成功: ステータス `0`、すべての出力が非 null、失敗メッセージなし。
- 通常の Graphcal のランタイム/ドメイン/アサーションの失敗: ステータス `1`、すべての出力が
  null、診断あり。
- 共有プロトコル検査を超える防御的な束縛の拒否: ステータス `4`。

不正な形式の IPC、互換性のない要求スキーマ、null の入力/コンテキスト値、
非有限または領域外の数値データ、未知のカテゴリカル値、無効な
辞書キー、整合しない ID/シード、重複する ID、および評価器内部の
不変条件はプロセスエラーです。これらは合成された失敗行にはなりません。

## プロトコルのライフサイクル { #protocol-lifecycle }

パイプのデッドロックを避けるため、起動順序は固定されています:

1. Graphcal がプロジェクトを読み込み、検査し、準備します。
2. stdout がスキーマのみの発見ストリームとその EOS マーカーを受け取ります。
3. stdout が結果ストリームのスキーマを受け取ります。
4. Graphcal が stdin の要求ストリームを開いて読み取ります。
5. 空でない各要求バッチが順次評価されます。
6. 要求の EOS が結果ストリームを終了させ、正常に終了します。

最初の stdout ストリームの読み取り側は、2 つ目のストリームのためにそれらのバイトを返せる場合を除き、
EOS を超えてバイトをバッファリングしてはなりません。Arrow のバッファリングされない
ストリームリーダーが適しています。

## 起動時のトラブルシューティング { #startup-troubleshooting }

`graphcal model serve` は、stdout に書き込む前に設定の失敗を報告します。
よくあるエラーには次のものがあります:

- 数値パラメーターに `min` がない、`max` がない、または片側のみ/非有限の範囲である。
- 入力が `Bool`、日時、複素数、代数的、インデックス付き、座標キー、
  または有限キーであり、いずれもスキーマ v2 では表現できない (再帰的な代数的
  入力は、再帰スキーマのケイパビリティ不一致として明示的に報告されます)。
- 選択された出力が未知、非公開、重複、またはスカラーの `Bool` でない。
- 入力と出力がフィールド名を共有している。
- プロジェクトの依存関係や WASM プラグインが通常の Graphcal の読み込み検査に失敗する。

まず `graphcal check reliability.gcl` でソースの診断を行い、次に
`graphcal model serve ...` の stderr でインターフェースのケイパビリティエラーを確認してください。stdout を
テキストとしてデコードしてはなりません。

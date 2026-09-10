---
icon: material/home
---

# Graphcal へようこそ { #welcome-to-graphcal }

Graphcal は、工学計算のための**型安全で、単位を認識し、Git と親和性の高いリアクティブプログラミング言語**です。スプレッドシートやその場しのぎのスクリプトを、型付けされ、バージョン管理された単一の計算グラフに置き換えます。

ドキュメントは英語と日本語で提供しています。ヘッダーの言語選択で各言語版のホームに移動でき、ページタイトルの上のリンクで同じページの別言語版を開けます。コード例とプレイグラウンドは共通で、プレイグラウンドの UI は英語のままです。

![ロケット方程式の計算について、計算値をインラインで表示している Helix 上の Graphcal](/docs/assets/rocket-screenshot.png)

*Graphcal の言語サーバーは計算されたノードの値をインラインで表示するため、プレーンテキストの計算ファイルがライブな工学ワークシートのように感じられます。*

## クイック例 { #quick-example }

Graphcal によるツィオルコフスキーのロケット方程式:

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

実行します:

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

## なぜ Graphcal なのか { #why-graphcal }

- **型安全性** -- 次元の不一致はコンパイル時に検出されます。
- **単位の認識** -- 物理的な次元を定義し、単位を付与し、単位間で変換できます。コンパイラーが次元の整合性を強制します。[Mars Climate Orbiter](https://en.wikipedia.org/wiki/Mars_Climate_Orbiter) のような失敗はもう起こりません。
- **リアクティブ計算** -- パラメーターとノードからなる DAG を定義します。入力を変更すると、依存するすべての値が自動的に更新されます。
- **Git との親和性** -- 差分の取得やマージがきれいに行えるプレーンテキストの `.gcl` ファイルです。バイナリのスプレッドシートは不要です。
- **ライブなエディター体験** -- LSP サーバーが計算値をインラインで表示するインレイヒントを提供し、エディターをライブな計算シートに変えます。

## はじめに { #getting-started }

<div class="grid cards" markdown>

- :material-download:{ .lg .middle } **インストール**

    ---

    Cargo を使って crates.io から Graphcal をインストールします。

    [:octicons-arrow-right-24: Graphcal をインストールする](installation.md)

- :material-play:{ .lg .middle } **ブラウザープレイグラウンド (実験的)**

    ---

    インストール不要で、アルファ段階の Graphcal をすぐに試せます。

    [:octicons-arrow-right-24: プレイグラウンドを開く](https://graphcal.org/playground/)

- :material-school:{ .lg .middle } **インタラクティブチュートリアル**

    ---

    プレイグラウンドで開ける単一ファイルの例を使って、Graphcal を段階的に学びます。

    [:octicons-arrow-right-24: チュートリアルを始める](tutorial/index.md)

- :material-book-open-variant:{ .lg .middle } **言語リファレンス**

    ---

    すべての言語機能に関する正式なドキュメントです。

    [:octicons-arrow-right-24: 言語リファレンス](language/index.md)

- :material-console:{ .lg .middle } **CLI リファレンス**

    ---

    コマンドラインインターフェースの完全なドキュメントです。

    [:octicons-arrow-right-24: CLI コマンド](cli-reference.md)

- :material-transit-connection-variant:{ .lg .middle } **Tenax 連携**

    ---

    一度コンパイルし、型付けされた工学モデルを永続的な Arrow IPC 経由で提供します。

    [:octicons-arrow-right-24: Graphcal を Tenax に接続する](tenax-integration.md)

- :material-puzzle:{ .lg .middle } **エディターのセットアップ**

    ---

    VS Code 拡張機能をインストールするか、ライブなインレイヒント付きで Zed/Neovim をセットアップします。

    [:octicons-arrow-right-24: エディターのセットアップ](editor-setup.md)

</div>

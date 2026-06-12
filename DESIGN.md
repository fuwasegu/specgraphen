# specgraphen — 設計ドラフト

> Higher Graphen (HG) を基盤に、コードの「意味」を根拠付き・多重導出で構築し、
> **AI エージェントが高精度かつ低コストでコードを読み取れる substrate** を提供するツール。

---

## 0. 北極星（確定済みの前提）

1. **全自動** — 人間レビューを介さない。
2. **単独LLM超え** — 単独の生成AIがソースを直読みするより高精度に意味を抽出する。
3. **主たる消費者は AI** — 人間向け仕様書は副次的な projection にすぎない。第一目的は
   「AI がこのツールを使うとコードの意味を読み取りやすくなる」こと。

HG はそもそも *AI-operated software development substrate*（AIが直接操作する開発基盤）であり、
この3条件は HG の設計思想と完全に一致する。specgraphen は HG の **Level 2 ドメイン製品**として位置づく。

---

## 1. 二つの柱（＝Higher Graphen の活用方法）

### 柱A — 精度エンジン：単独LLMより「正確に作る」

中核は **多重導出コロボレーション (multi-derivation corroboration)**。
同一の意味事実を独立な複数経路で導出し、HG の evidence モデルで融合する。

- ①決定的解析（CFG/DFG・ルート表・型・スキーマ）
- ②観点別 LLM パスを複数（挙動 / 契約 / 不変条件 / エラー処理。各々 **witness 引用必須**）
- ③テスト・型・実行トレースを裏取り witness として採用

融合規則：**一致→高確信で採用 / 部分一致→低確信で採用（不確実と明示）/ 矛盾→採用せず obstruction**。
確信度は LLM の「主張」ではなく **計算結果**。これがアンサンブル的に単独LLMの精度を超える源泉。

| 単独LLMの失敗モード | specgraphen(HG) の自動対策 |
|---|---|
| コンテキスト窓に入らず横断関係を見落とす | 静的解析で全体を **Space/Complex** 化 → LLMは確定済みの全体構造の上で局所解釈するだけ |
| 根拠なく断定（幻覚） | 各 spec cell に **witness（コード位置）必須**。witness無しの主張は不採用 or obstruction |
| 自分の出力内の矛盾に気づかない | **invariant** が機械検査（無矛盾・到達可能性・被覆）。違反は **obstruction** として自動検出 |
| 自己修正ループを持たない | obstruction を入力に **再導出ループ**（証拠追加／主張分割／確信度降格）で収束 |
| docstring・命名を鵜呑み | 意図(doc)と実装(CFG/DFG)を別 derivation として突き合わせ、不一致を obstruction化（ドリフト検知） |
| 自信過剰で均一トーン | 複数 derivation の **一致度＝確信度** を計算。低一致は明示的に不確実 or obstruction |
| テスト・型を体系的に使わない | テスト・型・実行トレースを **witness** として採用し裏取り |

人間レビューの代わりに **invariant検査 ＋ コロボレーション閾値** が自動ゲートになる。
ループは無人で回るが、provenance/derivation の監査証跡は残り、後から検証可能。

### 柱B — AI配信層：AIが「読みやすく」受け取る

HG は projection を *human / agent / audit* など消費者別に出せる。specgraphen の第一 projection は **agent 向け**。

| HG 機能 | AIのコード理解をどう容易にするか |
|---|---|
| **Projection (agent audience)** | シンボル単位の意味を数百トークンの構造化レコードで返す。N個のファイル直読みが不要に |
| **Cell（意味単位）** | 行単位でなく entity/relation/constraint 単位で navigate。トークンあたりの信号量が高い |
| **Context** | 「prod スコープでの意味」「flag ON時の挙動」だけを要求でき、ノイズ無しで読める |
| **Morphism（code→intent）** | 「何を書いてあるか」でなく「何を意味するか」の抽象層を事前計算して提供 |
| **Witness / Provenance** | 必要な箇所だけソースへドリルダウン。全読みせず安価に裏取りできる |
| **Obstruction** | 意味が曖昧な箇所を明示 → AIは限られた推論を効く場所に集中できる |
| **Confidence** | 記述ごとに信頼度。AIが過信せず、確信度に応じて検証を増減できる |

---

## 2. コードベース → Higher Graphen マッピング

| HG プリミティブ | specgraphen での意味 |
|---|---|
| **Space** | コードベース / プロジェクト / 解析対象モジュール |
| **Cell(0)** | module, class, function, type, endpoint, config, DBテーブル, 機能 |
| **Cell(1)** | call, import, data-flow, read/write, route→handler, event pub/sub |
| **Cell(2+)** | 関係をまたぐ整合条件＝仕様ルール（例「全 endpoint に認可チェックがある」） |
| **Complex** | 呼び出しグラフ／モジュール階層／API面／状態機械／データモデル |
| **Context** | public/internal, prod/test, feature flag, version などの意味スコープ |
| **Morphism** | code→挙動仕様、code→アーキ、v1→v2（移行）、具体→意図の抽象化 |
| **Invariant** | 期待する仕様ルール（自動検証の基準） |
| **Obstruction** | 仕様化できない理由（動的ディスパッチ、ソース欠落、doc↔実装の矛盾） |
| **Completion candidate** | 導出された意味の候補（閾値で自動採否、監査証跡は保持） |
| **Projection** | agent向け意味ビュー（第一）／Markdown仕様・OpenAPI・トレーサビリティ（副次） |
| **Witness / Derivation** | 各記述の根拠：file/line/commit・抽出手法・確信度・推論連鎖 |

---

## 3. パイプライン（無人ループ）

```
Lift（事実スキャフォールド・高確信）
  → 多重導出（決定的 + 観点別LLM×N + テスト/型）
    → コロボレーション融合 ＋ 確信度算定
      → invariant 検査（自動の"レビュア"）
        → obstruction 駆動の再導出ループ（収束 or 正直なギャップ報告）
          → Project（agent / human / audit）
```

再 Lift（commit ごと）→ Space を差分 → **仕様ドリフト**（挙動変更・未文書の新endpoint・新規違反）も出せる。

---

## 4. AI から見たインターフェース（第一級の成果物）

MCP サーバ / Claude skill としてクエリツールを公開し、エージェントが Space を直接操作する。例：

- `explain(symbol)` → 署名 / 一行意図 / 契約(pre・post・invariant) / 副作用 / 入出力依存 /
  エラー挙動 / 関連context / **各項目の確信度** / **witness(file:line)** / **未解決のobstruction** を構造化で返す
- `callers(symbol)` / `callees(symbol)` — 検証済みグラフ上の呼び出し関係
- `dataflow(source → sink)` — 到達可能性・データフロー（単独LLMが苦手な横断クエリ）
- `impact(change)` — 「Xを変えると何に波及するか」を complex 走査で返す
- `enforces(invariant)` — 「この不変条件を担保しているコードはどこか」
- `contexts(scope)` — 指定スコープでの意味だけを抽出
- `unknowns(module)` — obstruction 列挙（＝AIが注意すべき曖昧点・Known Unknowns）
- 人間向け projection：Markdown仕様書 / OpenAPI / 要求↔コード↔テスト トレーサビリティ（副次）

ねらい：エージェントがタスク中に **数百トークンで根拠付きの「意味」を取得**でき、
ファイル全読み・構造の再導出を回避し、確信度と obstruction で検証コストを最適配分できる。

---

## 5. 機能一覧（MVP → 拡張）

**MVP**
- 1言語の Lift（エンティティ/関係抽出 → Space化、witness付与）
- 決定的 + 単一LLMパスの2系統コロボレーション、確信度算定
- 基本 invariant 数本（grounding必須・無矛盾・到達可能）と obstruction出力
- `explain` / `callers` / `callees` クエリ（MCP or CLI）

**拡張**
- 観点別 LLM パス複数化、obstruction 再導出ループ
- 多言語フロントエンド、context（prod/test/flag）対応
- `dataflow` / `impact` / `enforces` / `unknowns`
- 人間向け projection（Markdown/OpenAPI/トレーサビリティ）
- commit 差分による仕様ドリフト

---

## 6. アーキテクチャ（推奨と代替・未確定）

精度ループ（コロボレーション＋invariant＋自己修正）を HG の Rust 製 reasoning/evidence エンジンと
密に回す必要があるため、Rust 寄りを推奨。

- **推奨：Rust ネイティブ + tree-sitter** — HGクレート直結のLevel2製品。多言語を1プロセス解析、
  LLMパスはAPI経由。invariant/evidence/obstruction をフル活用でき精度ループが最短。
- 代替1：**ハイブリッド** — Rust HGコア + 各言語の本格解析器(LSP/コンパイラAPI)をアダプタ連携。抽出精度↑だが構成は重い。
- 代替2：**ポリグロット(CLI駆動)** — TS/Python等で抽出しCaseGraphen JSON生成、CLIで推論。実装は軽いが
  現状CLIの保守的mutation制約に縛られHGの価値を活かしきれない懸念。

> 注：core クレートの実体は `id/provenance/confidence/review/source/correspondence/typed_provenance` 等の
> **信頼性基盤**。Space/Cell/Complex は `-structure`、推論/検査は `-reasoning`/`-evidence`/`-projection`。
> 「core を利用」はこの Level 0 基盤群の活用と解釈。着手時にソースで境界を確認する。

---

## 7. 想定ユースケース

- エージェントが改修タスク中、対象関数を `explain` して契約・副作用・呼び出し元を即把握 → 全読み不要。
- 変更前に `impact` で波及範囲を確認 → 壊しうる不変条件を事前に知る。
- `unknowns` で曖昧箇所だけソースを精読 → 推論コストを集中投下。
- レビュー/監査時、人間向け projection（確信度・損失申告つき）と監査証跡を提示。

---

## 8. 確定事項 / 実装状況

- [x] アーキテクチャ確定：**Rust ネイティブ + tree-sitter**（HG v0.7.1 クレート直結）
- [x] 初期対象言語：**Java**（tree-sitter-java 0.23、Shift-JIS/EUC-JP 自動エンコーディング対応）
- [x] HG クレートの実 API 表面確認済み（Space/Cell/Incidence/Provenance/Confidence/Obstruction）
- [x] MVP スコープ確定・実装完了（9 クレート、44 ファイル、5,268 行）

### 実装済み機能

**Lift（コード → Space 変換）**
- tree-sitter-java による Java パース（Package/Class/Interface/Enum/Method/Constructor/Field 抽出）
- 関係抽出（ContainedIn/Extends/Implements/Calls/Constructs/Throws/Imports/AnnotatedWith）
- FQN ベースのシンボル同一性、Witness（file:line）付き全 Cell

**多重導出コロボレーション**
- 方式1：API 直接呼び出し（Claude API / OpenAI 互換、`--llm-provider` フラグ）
- 方式2：Claude Code 自身を LLM として利用（`enrich` → Claude 推論 → `annotate` → `save`）
- 確信度算定：tree-sitter 導出 + LLM 導出の一致度で計算

**Invariant 検査**
- Grounding：全 Cell に Witness 必須
- Consistency：矛盾する derivation の検出
- Reachability：Space root からの到達可能性

**AI 向けインターフェース（MCP サーバ、14 ツール）**

| ツール | DESIGN.md 対応 | 説明 |
|---|---|---|
| `overview` | 新規 | プロジェクト全体の構造（エンティティ数、パッケージ構成） |
| `search` | 新規 | 名前でエンティティ検索（部分一致、型フィルタ） |
| `feature` | 新規 | 機能単位の分析（関連クラス、エントリポイント、内部呼び出し、外部依存） |
| `package_dependencies` | 新規 | パッケージ間の依存グラフ |
| `class_dependencies` | 新規 | クラスの依存関係 |
| `explain` | §4 explain | シンボルの意味（signature, intent, behavior, contracts, witnesses） |
| `callers` | §4 callers | 呼び出し元一覧 |
| `callees` | §4 callees | 呼び出し先一覧 |
| `impact` | §4 impact | 変更影響範囲分析 |
| `unknowns` | §4 unknowns | 曖昧点・未解決参照一覧 |
| `enrich` | 新規 | シンボルのソースコード + 構造コンテキスト取得（Claude Code 連携用） |
| `enrich_batch` | 新規 | 未分析エンティティの一括取得 |
| `annotate` | 新規 | Claude Code が分析した意味注釈を Space に書き戻し |
| `save` | 新規 | 注釈をディスクに永続化 |

### 次アクション（拡張フェーズ）

- [ ] 観点別 LLM パス複数化（behavior / contract / invariant の独立パス）
- [ ] obstruction 再導出ループ（収束 or ギャップ報告）
- [ ] 多言語対応（TypeScript / Python の tree-sitter grammar 追加）
- [ ] context 対応（prod/test/feature flag によるスコープ分離）
- [ ] `dataflow` / `enforces` クエリ
- [ ] 人間向け projection（Markdown / OpenAPI / トレーサビリティ）
- [ ] commit 差分による仕様ドリフト検知

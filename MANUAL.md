# specgraphen マニュアル

## これは何？

specgraphen は **Java のソースコードを事前解析して、AI エージェントが高速・高精度にコードの意味を取得できるようにするツール**。

通常 AI がコードを調べるとき、grep でファイルを探し、Read で中身を読み、推論で意味を理解する。ファイルが 300 個あれば、何十回もツールを呼ぶ必要があり、トークンも時間も大量に消費する。

specgraphen は「事前に全ファイルを解析して構造化データを作っておく」ことで、AI が 1 回のツール呼び出しで必要な情報を取得できるようにする。

---

## 全体の流れ

```
① lift（初期解析）
   Java ソースコードを tree-sitter で全ファイルパース
   → クラス、メソッド、フィールド、呼び出し関係などを抽出
   → 構造化データ（Space）として JSON ファイルに保存

② serve（MCP サーバ起動）
   保存された Space を読み込み、MCP サーバとして待機
   → Claude Code から 15 種類のクエリを受け付ける

③ Claude Code が使う
   ユーザーが「注文機能の仕様を教えて」と聞く
   → Claude Code が specgraphen のツールを呼ぶ
   → 構造化された回答が返る（grep + Read の何十倍も速い）
```

---

## lift とは何か

**lift = ソースコードを構造化データに変換する処理。** 1回実行すればよく、コードが変わったら再実行する。

### やっていること（ステップバイステップ）

1. **ファイル収集**: 指定ディレクトリ内の `**/*.java` を全て見つける
2. **パース（Pass 1）**: 各ファイルを tree-sitter（構文解析器）でパースし、以下を抽出:
   - **エンティティ**: パッケージ、クラス、インターフェース、enum、メソッド、コンストラクタ、フィールド
   - 各エンティティに FQN（完全修飾名）を付与: `com.example.model.User.email`
   - 各エンティティにソースの位置（ファイル名 + 行番号）を記録
3. **関係抽出（Pass 2）**: 再度全ファイルをパースし、エンティティ間の関係を抽出:
   - `ContainedIn`: メソッドがクラスに属する、クラスがパッケージに属する
   - `Extends`: 継承関係
   - `Implements`: インターフェース実装
   - `Calls`: メソッド呼び出し
   - `Constructs`: `new` によるオブジェクト生成
   - `Throws`: 例外送出
   - `Imports`: import 文
   - `AnnotatedWith`: アノテーション
4. **ストア構築**: 全エンティティと関係を Higher Graphen の `InMemorySpaceStore` に格納（グラフインデックス）
5. **Invariant 検査**: 構造の整合性チェック
   - Grounding: 全エンティティにソース位置（witness）があるか
   - Reachability: 孤立したエンティティがないか（HG の BFS エンジン）
   - Acyclicity: 継承の循環がないか（HG の cycle detection エンジン）
   - Consistency: 矛盾する情報がないか
6. **保存**: 結果を `.specgraphen/spaces/<id>/space.json` に JSON 保存

### エンコーディング

Shift-JIS / EUC-JP のレガシー Java コードも自動判定して読める。

### LSP 統合（オプション）

`--lsp java` を付けると、jdtls（Java Language Server）を起動して**型レベルの解決**を行う。tree-sitter だけでは `obj.method()` の `obj` の型がわからないが、LSP があれば正確に解決できる。jdtls がなければ自動でヒューリスティック（名前ベース推定）にフォールバック。

---

## serve とは何か

**serve = lift で作った構造化データを MCP サーバとして公開する処理。**

Claude Code の設定（`.claude.json` または `mcp.json`）に登録すると、Claude Code 起動時に自動で specgraphen サーバが立ち上がる。以後、Claude Code は 15 種類のツールでコードの情報を取得できる。

---

## 15 のツール一覧

### 全体を見る

| ツール | 何がわかる | 使い方の例 |
|---|---|---|
| **overview** | プロジェクト全体の構造。エンティティ数、関係数、パッケージ構成 | 「プロジェクトの全体像を教えて」 |
| **search** | 名前でエンティティを検索。部分一致、型フィルタ対応 | 「認証に関連するクラスは？」 |
| **package_dependencies** | パッケージ間の依存グラフ | 「アーキテクチャを見せて」 |

### 機能・テーブルを調べる

| ツール | 何がわかる | 使い方の例 |
|---|---|---|
| **feature** | キーワードで機能を分析。関連クラス、エントリポイント、内部呼び出し、外部依存 | 「注文機能の仕様を教えて」 |
| **column_usage** | テーブルクラスの各カラムの論理名・データ型・読み書き箇所 | 「User テーブルの各カラムの用途は？」 |

### シンボルを調べる

| ツール | 何がわかる | 使い方の例 |
|---|---|---|
| **explain** | シンボルの意味。signature, intent, behavior, 契約, 副作用, witness, callers, callees | 「createUser メソッドを説明して」 |
| **callers** | あるメソッド/クラスを呼び出している箇所の一覧 | 「save の呼び出し元は？」 |
| **callees** | あるメソッドが呼び出している先の一覧 | 「deleteUser は何を呼んでる？」 |
| **class_dependencies** | 特定クラスが何に依存し、何が依存しているか | 「UserService の依存関係は？」 |

### 変更の影響を調べる

| ツール | 何がわかる | 使い方の例 |
|---|---|---|
| **impact** | シンボルを変更した場合の波及範囲（直接影響 + 推移影響 + 影響ファイル） | 「UserService を変えたら何に影響する？」 |
| **unknowns** | 解析で曖昧だった箇所・未解決参照の一覧 | 「不明点を教えて」 |

### AI がコードの意味を学習する（enrich フロー）

| ツール | 何をする | 流れ |
|---|---|---|
| **enrich** | シンボルのソースコード + 構造コンテキストを返す | Claude がソースを読む |
| **enrich_batch** | 未分析エンティティをまとめて返す | Claude が一括で分析 |
| **annotate** | Claude が分析した結果（intent, behavior 等）を Space に書き戻す | 学習結果を保存 |
| **save** | annotate した内容をディスクに永続化 | 次回以降も使える |

**enrich フローの特徴**: API キー不要。Claude Code 自身が LLM として分析を行う。

---

## セットアップ手順

### 1. ビルド

```sh
cd /path/to/specgraphen
cargo build --release -p specgraphen-cli
```

バイナリは `target/release/specgraphen` に生成される。

### 2. lift（初期解析）

```sh
specgraphen lift \
  --root /path/to/java-project \
  --space-id myproject \
  --store /path/to/.specgraphen
```

**オプション**:
- `--lsp java`: jdtls で型解決を強化（jdtls が必要）
- `--llm-provider claude --llm-api-key $KEY`: LLM で意味注釈を自動追加

### 3. MCP 登録

プロジェクトの `.claude.json`（`~/.claude.json` のプロジェクト設定内）に追加:

```json
{
  "mcpServers": {
    "specgraphen": {
      "type": "stdio",
      "command": "/path/to/specgraphen",
      "args": [
        "serve",
        "--space-id", "myproject",
        "--store", "/path/to/.specgraphen",
        "--source-root", "/path/to/java-project",
        "--transport", "stdio"
      ]
    }
  }
}
```

`--source-root` は enrich ツールでソースコードを読むために必要。

### 4. 使う

```sh
cd /path/to/project
claude
```

Claude Code 内で自然言語で聞けば、specgraphen のツールが自動で呼ばれる。

---

## アーキテクチャ

```
                    ┌─────────────┐
                    │  Claude Code │
                    └──────┬──────┘
                           │ MCP (stdio JSON-RPC)
                    ┌──────▼──────┐
                    │ specgraphen │
                    │  MCP サーバ  │
                    └──────┬──────┘
                           │
              ┌────────────▼────────────┐
              │     Query Engine        │
              │  15 ツール + Projection  │
              └────────────┬────────────┘
                           │
         ┌─────────────────▼─────────────────┐
         │       InMemorySpaceStore (HG)      │
         │  entities / relations  │
         └─────────────────┬─────────────────┘
                           │
    ┌──────────┬───────────▼───────────┬──────────┐
    │ Bayesian │  Reachability/Cycle   │ Corresp. │
    │Confidence│   (HG BFS engine)    │ + Gluing │
    │ (HG)     │                      │  (HG)    │
    └──────────┴───────────────────────┴──────────┘
```

### 10 クレート

| クレート | 役割 |
|---|---|
| `specgraphen-model` | ドメイン型。JavaEntityType, SemanticAnnotation, SpaceData |
| `specgraphen-lift` | tree-sitter で Java をパース → HG Space に変換 |
| `specgraphen-resolver` | TypeResolver trait。LSP (jdtls) / heuristic / chain fallback |
| `specgraphen-llm` | LLM 抽象化。Claude API / OpenAI 互換 |
| `specgraphen-corroboration` | 多重導出コロボレーション。HG Bayesian Confidence + Correspondence + Gluing |
| `specgraphen-invariant` | 構造検査。HG reachable() + find_simple_cycles() |
| `specgraphen-query` | 15 ツールのクエリエンジン |
| `specgraphen-store` | JSON ファイル永続化 |
| `specgraphen-mcp` | MCP サーバ（stdio JSON-RPC） |
| `specgraphen-cli` | CLI バイナリ（lift / query / serve） |

### Higher Graphen (HG) の活用

specgraphen は HG の**推論エンジン**に計算を委譲している:

| 計算 | HG エンジン |
|---|---|
| グラフ走査（到達可能性、影響分析） | `InMemorySpaceStore::reachable()` |
| 循環検出（継承の循環） | `InMemorySpaceStore::find_simple_cycles()` |
| 確信度計算 | `EvidenceLikelihood` + `update_confidence()` (ベイズ推論) |
| 導出の一致/矛盾検出 | `derive_correspondence_candidates()` |
| 導出のマージ可否判定 | `attempt_gluing()` |
| 出力の情報損失追跡 | Projection information loss |

---

## 用語集

| 用語 | 意味 |
|---|---|
| **lift** | ソースコードを構造化データ（Space）に変換する処理 |
| **Space** | HG の最上位コンテナ。1プロジェクト = 1 Space |
| **Cell** | エンティティ（クラス、メソッド、フィールド等）。dimension=0 |
| **Incidence** | Cell 間の関係（呼び出し、継承、包含等） |
| **FQN** | 完全修飾名。`com.example.model.User.email` |
| **Witness** | あるエンティティや関係の根拠。ソースファイルのパスと行番号 |
| **Provenance** | 情報の出所と確信度。witness + confidence + derivation source |
| **Obstruction** | 解析できなかった箇所。型解決失敗、矛盾する情報等 |
| **Derivation** | 情報がどの手段で導出されたか。TreeSitter / LSP / LLM |
| **Corroboration** | 複数の導出を突き合わせて確信度を計算すること |
| **Confidence** | 情報の信頼度。0.0〜1.0。HG のベイズ推論で計算 |
| **Annotation** | Claude が分析した意味注釈（intent, behavior, preconditions 等） |
| **Projection** | 特定の受け手（AI / 人間 / 監査）向けに情報を絞ったビュー |

---

## CLI リファレンス

### lift

```sh
specgraphen lift \
  --root <ソースコードのルートディレクトリ> \
  --space-id <プロジェクト識別子> \
  [--store <保存先ディレクトリ>]          # デフォルト: .specgraphen
  [--lsp java]                           # jdtls で型解決を強化
  [--llm-provider claude|openai]         # LLM で意味注釈を自動追加
  [--llm-api-key <API key>]              # または環境変数 ANTHROPIC_API_KEY
  [--llm-model <モデル名>]
```

### query

```sh
specgraphen query --space-id <id> --store <path> explain <symbol>
specgraphen query --space-id <id> --store <path> callers <symbol>
specgraphen query --space-id <id> --store <path> callees <symbol>
```

### serve

```sh
specgraphen serve \
  --space-id <id> \
  --store <path> \
  [--source-root <ソースコードのルート>]  # enrich ツール用
  [--transport stdio]
```

---

## FAQ

**Q: コードを変更したら specgraphen に反映される？**
A: 自動では反映されない。`lift` を再実行する。

**Q: lift にどのくらい時間がかかる？**
A: 数百ファイルの Java プロジェクトで約 1 秒。

**Q: LSP (jdtls) がないと使えない？**
A: 使える。LSP なしでも tree-sitter + ヒューリスティックで動く。LSP は精度を上げるオプション。

**Q: Java 以外は？**
A: 現在は Java のみ。TypeResolver trait で言語を抽象化しているので、将来 TypeScript / Python を追加可能。

**Q: grep と何が違う？**
A: grep は文字列検索。specgraphen は構文解析した構造化データに対するクエリ。「`Order` に関連するクラスを全部見せて」「`UserService` を変えたら何に影響する？」のような横断クエリは grep では実質不可能。

**Q: API キーは必要？**
A: MCP サーバとして使うだけなら不要。`--llm-provider` で LLM 自動注釈を使う場合のみ必要。enrich フロー（Claude Code 自身が分析）なら API キー不要。

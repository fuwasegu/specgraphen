# specgraphen

**AI-operated code meaning extraction substrate built on [Higher Graphen](https://github.com/CAPHTECH/higher-graphen).**

specgraphen pre-analyzes your codebase and exposes structured, evidence-backed code intelligence to AI agents via MCP (Model Context Protocol). Instead of grepping through hundreds of files, an AI agent queries specgraphen once and gets back type-resolved call graphs, feature scopes, column usage maps, and change impact analysis — with source witnesses (file:line) for every claim.

---

## Why

When an AI agent investigates code with `grep` + `Read`, it burns tokens and time on each file. Cross-cutting queries like "what calls this method?" or "what columns does this table have?" require dozens of tool calls and still miss things.

specgraphen solves this by:

1. **Pre-analyzing** the entire codebase (tree-sitter + optional LSP) into a structured graph
2. **Storing** entities, relations, and witnesses in a Higher Graphen Space
3. **Serving** 15 query tools via MCP — one call per question, sub-second response

| Without specgraphen | With specgraphen |
|---|---|
| grep → Read → grep → Read → ... (dozens of calls) | `feature("Order")` → structured result in 1 call |
| No type resolution, text matching only | LSP-backed type-strict symbol resolution |
| Can't do cross-file call graphs | Pre-computed call graph with witnesses |
| Tokens: 10,000-50,000 per question | Tokens: 300-2,000 per question |

## Quick Start

### Build

```sh
cargo build --release -p specgraphen-cli
```

### Analyze a Java project

```sh
# Basic (tree-sitter only)
specgraphen lift --root ./my-java-project --space-id myproject

# With LSP type resolution (requires jdtls)
specgraphen lift --root ./my-java-project --space-id myproject --lsp java
```

### Register as MCP server

Add to your Claude Code project config (`.claude.json` or `mcp.json`):

```json
{
  "mcpServers": {
    "specgraphen": {
      "type": "stdio",
      "command": "/path/to/specgraphen",
      "args": [
        "serve",
        "--space-id", "myproject",
        "--store", ".specgraphen",
        "--source-root", "./my-java-project",
        "--transport", "stdio"
      ]
    }
  }
}
```

### Use from Claude Code

Just ask naturally:

- "What's the overall structure of this project?"
- "Show me the Order feature — what classes are involved?"
- "What would break if I change UserService?"
- "What are the columns of the Customer table and where are they used?"
- "Explain the createUser method"

## 15 MCP Tools

### Project-wide

| Tool | What it returns |
|---|---|
| `overview` | Entity/relation counts, package structure |
| `search` | Find entities by name (partial match, type filter) |
| `package_dependencies` | Cross-package dependency graph |

### Feature & table analysis

| Tool | What it returns |
|---|---|
| `feature` | Classes, entry points, internal calls, external deps for a business feature |
| `column_usage` | Per-column logical name, data type, and all read/write sites |

### Symbol-level

| Tool | What it returns |
|---|---|
| `explain` | Signature, intent, behavior, contracts, witnesses, callers, callees |
| `callers` | All verified callers with confidence and witnesses |
| `callees` | All verified callees with confidence and witnesses |
| `class_dependencies` | What a class depends on and what depends on it |

### Change analysis

| Tool | What it returns |
|---|---|
| `impact` | Direct + transitive impacts of changing a symbol, affected files |
| `unknowns` | Ambiguous points, unresolved references (known unknowns) |

### Enrich flow (Claude Code as LLM)

| Tool | What it does |
|---|---|
| `enrich` | Returns source code + context for a symbol, ready for analysis |
| `enrich_batch` | Returns a batch of unannotated entities |
| `annotate` | Saves Claude's analysis (intent, behavior, etc.) back to the Space |
| `save` | Persists annotations to disk |

No API key needed — Claude Code itself is the LLM engine.

## LSP Integration

specgraphen can optionally use Language Server Protocol for type-strict symbol resolution:

```sh
specgraphen lift --root ./project --space-id myproject --lsp java
```

| Without LSP | With LSP (jdtls) |
|---|---|
| Name-based heuristic resolution | Type-strict definition lookup |
| Cross-file and overloaded calls often stay unresolved | Far fewer unresolved references, much denser call graph |

The `TypeResolver` trait supports multiple languages:
- **Java**: jdtls (Eclipse JDT Language Server)
- **TypeScript**: typescript-language-server (planned)
- **Python**: pyright (planned)

Falls back to tree-sitter heuristics when LSP is unavailable.

## Architecture

```
specgraphen-cli          CLI binary (lift / query / serve)
specgraphen-mcp          MCP server (stdio JSON-RPC, 15 tools)
specgraphen-query         Query engine + projection
specgraphen-lift          tree-sitter Java parser → HG Space
specgraphen-resolver      TypeResolver trait (LSP / heuristic / chain)
specgraphen-corroboration Multi-derivation fusion (HG Bayesian + Correspondence)
specgraphen-invariant     Structural checks (HG reachable + cycle detection)
specgraphen-model         Domain types (JavaEntityType, SpaceData)
specgraphen-store         JSON file persistence
specgraphen-llm           LLM abstraction (Claude / OpenAI)
```

Built on **Higher Graphen** reasoning engines:
- `InMemorySpaceStore` — indexed graph storage with traversal
- `EvidenceLikelihood` + `update_confidence()` — Bayesian confidence
- `find_simple_cycles()` — inheritance cycle detection
- `derive_correspondence_candidates()` + `attempt_gluing()` — multi-derivation fusion

## License

MIT

---

# specgraphen (日本語)

**[Higher Graphen](https://github.com/CAPHTECH/higher-graphen) を基盤とした、AI エージェント向けコード意味抽出基盤。**

specgraphen はコードベースを事前解析し、構造化された根拠付きのコード知識を MCP（Model Context Protocol）経由で AI エージェントに提供します。何百ものファイルを grep する代わりに、AI エージェントは specgraphen に1回問い合わせるだけで、型解決済みの呼び出しグラフ、機能スコープ、カラム使用マップ、変更影響分析を — すべてソース witness（ファイル:行番号）付きで — 取得できます。

## なぜ必要か

AI エージェントが `grep` + `Read` でコードを調査すると、ファイルごとにトークンと時間を消費します。「このメソッドを呼んでいるのは？」「このテーブルのカラムは何に使われている？」のような横断クエリは、何十回ものツール呼び出しが必要で、それでも見落としが発生します。

specgraphen は:

1. コードベース全体を事前解析（tree-sitter + オプションで LSP）して構造化グラフに変換
2. エンティティ・関係・根拠を Higher Graphen Space に格納
3. MCP 経由で 15 種類のクエリツールを提供 — 1回の問い合わせ、サブ秒で応答

## クイックスタート

### ビルド

```sh
cargo build --release -p specgraphen-cli
```

### Java プロジェクトを解析

```sh
# 基本（tree-sitter のみ）
specgraphen lift --root ./my-java-project --space-id myproject

# LSP で型解決を強化（jdtls が必要）
specgraphen lift --root ./my-java-project --space-id myproject --lsp java
```

### MCP サーバとして登録

Claude Code のプロジェクト設定（`.claude.json` または `mcp.json`）に追加:

```json
{
  "mcpServers": {
    "specgraphen": {
      "type": "stdio",
      "command": "/path/to/specgraphen",
      "args": [
        "serve",
        "--space-id", "myproject",
        "--store", ".specgraphen",
        "--source-root", "./my-java-project",
        "--transport", "stdio"
      ]
    }
  }
}
```

### 使う

Claude Code で自然に聞くだけ:

- 「このプロジェクトの全体像を教えて」
- 「注文機能の仕様は？関連するクラスは？」
- 「UserService を変えたら何に影響する？」
- 「User テーブルの各カラムはどこで使われてる？」
- 「createUser メソッドを説明して」

## LSP 統合

LSP を使うと型厳密なシンボル解決が可能に:

| LSP なし | LSP あり (jdtls) |
|---|---|
| 名前ベースのヒューリスティック解決 | 型厳密な定義ルックアップ |
| クロスファイル・オーバーロード呼び出しが未解決になりやすい | 未解決参照が大幅に減り、呼び出しグラフが密になる |

`TypeResolver` trait で言語を抽象化:
- **Java**: jdtls（実装済み）
- **TypeScript**: typescript-language-server（予定）
- **Python**: pyright（予定）

LSP が使えない環境では tree-sitter ヒューリスティックに自動フォールバック。

## 15 の MCP ツール

| カテゴリ | ツール | 説明 |
|---|---|---|
| 全体 | `overview` | プロジェクト全体の構造 |
| 全体 | `search` | 名前でエンティティ検索 |
| 全体 | `package_dependencies` | パッケージ間依存グラフ |
| 機能 | `feature` | 機能単位の分析（関連クラス、エントリポイント、依存） |
| 機能 | `column_usage` | テーブルカラムの論理名・型・読み書き箇所 |
| シンボル | `explain` | シンボルの意味（signature, witness, callers, callees） |
| シンボル | `callers` | 呼び出し元一覧 |
| シンボル | `callees` | 呼び出し先一覧 |
| シンボル | `class_dependencies` | クラスの依存関係 |
| 変更 | `impact` | 変更影響範囲（直接 + 推移 + 影響ファイル） |
| 変更 | `unknowns` | 曖昧点・未解決参照 |
| 学習 | `enrich` | ソースコード + コンテキストを返す |
| 学習 | `enrich_batch` | 未分析エンティティの一括取得 |
| 学習 | `annotate` | Claude の分析結果を Space に書き戻す |
| 学習 | `save` | 注釈をディスクに永続化 |

## ライセンス

MIT

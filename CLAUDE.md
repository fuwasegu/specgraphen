# specgraphen

AI-operated code meaning extraction substrate built on Higher Graphen (HG).

## Build

```sh
cargo build
cargo check
```

## Test

```sh
cargo test
```

## Lint

```sh
cargo clippy --all-targets
cargo fmt --check
```

## Run CLI

```sh
cargo run -p specgraphen-cli -- lift --root ./tests/fixtures/simple-project --space-id test
cargo run -p specgraphen-cli -- query explain com.example.UserService.createUser
cargo run -p specgraphen-cli -- serve --transport stdio
cargo run -p specgraphen-cli -- export --space-id test --out SPEC.md
```

## Architecture

Rust workspace with 10 crates:

- `specgraphen-model` — Domain types (JavaEntityType, JavaRelationType, SemanticAnnotation)
- `specgraphen-lift` — tree-sitter Java → HG Space/Cell/Incidence with witnesses
- `specgraphen-llm` — LLM abstraction trait + Claude/OpenAI implementations
- `specgraphen-corroboration` — Multi-derivation fusion, confidence calculation
- `specgraphen-invariant` — Grounding/consistency/reachability checks
- `specgraphen-query` — explain/callers/callees query engine
- `specgraphen-store` — JSON file store (SpaceStore trait)
- `specgraphen-mcp` — MCP server (stdio transport)
- `specgraphen-cli` — CLI binary
- `specgraphen-resolver` — TypeResolver trait (LSP / heuristic / chain)

All crates depend on Higher Graphen (HG) crates via git dependency, pinned to a specific rev in the workspace Cargo.toml.

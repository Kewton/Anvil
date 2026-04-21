# ANVIL.md

## Product Summary

Anvil は Ollama 前提の local-first coding agent。
v0.1.0 では旧 Anvil の汎用 phase machine を廃止し、以下のコアへ絞っている。

- Ollama direct chat
- tool-first agent loop
- Bash / Read / Write / Edit / Glob / Grep
- Plan / Act
- session persistence
- XML fallback
- git checkpoint / rollback

## Non-Goals For This Rewrite

- multi-provider abstraction
- 旧 `src/app/*` ベースの継続移植
- 複雑な termination / bootstrap / detector 群の維持
- MCP / skills / watcher / auto-test / heavy TUI の先行移植

## Public Compatibility Kept

- crate name: `anvil`
- binary name: `anvil`
- CI entrypoints: `fmt`, `clippy`, `test`, `build`
- release flow: tag push -> GitHub Release -> gzipped artifacts

## Source Layout

```text
src/
  agent/
  git/
  modes/
  ollama/
  safety/
  session/
  tools/
```

## Quality Bar

- local LLM が一本道で追えること
- 状態機械よりも失敗しにくい protocol を優先すること
- 「起動した」ではなく「依頼どおりに動く」へ寄せること

# ANVIL.md

## Product Summary

Anvil は Ollama 前提の local-first coding agent。
旧 Anvil の汎用 phase machine ではなく、ローカル LLM が追いやすい tool-first loop と構造化された recovery / verification に寄せている。

- Ollama direct chat
- tool-first agent loop
- Bash / Read / Write / Edit / Glob / Grep
- Plan / Act
- session persistence
- XML fallback
- git checkpoint / rollback
- evidence-based WorkMode policy
- AutoTest / Tester / temporary test workspace
- RepoGraph / Case Memory / AntiPattern
- structured eval log
- Photon サイドカー連携（context_pack / evaluate / shadow mode / canary）

## Non-Goals For This Rewrite

- multi-provider abstraction
- 旧 `src/app/*` ベースの継続移植
- 汎用クラウド agent 的な provider / subagent / MCP transport の拡張
- 重い full-screen TUI
- deterministic template を通常完了証明として扱うこと

## Public Compatibility Kept

- crate name: `anvil`
- binary name: `anvil`
- CI entrypoints: `fmt`, `clippy`, `test`, `build`
- release flow: tag push -> GitHub Release -> gzipped artifacts

## Source Layout

```text
src/
  agent/
    loop_run/
  git/
  modes/
  ollama/
  repo_graph/
  safety/
  session/
  tools/
```

## Quality Bar

- local LLM が一本道で追えること
- 状態機械よりも失敗しにくい protocol を優先すること
- 「起動した」ではなく「依頼どおりに動く」へ寄せること
- パターンマッチは security / syntax recovery では許容し、intent / quality / success / verifier では evidence として扱うこと

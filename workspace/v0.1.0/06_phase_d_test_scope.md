# v0.1.0 Phase D Test Scope

作成日: 2026-04-16
対象: `anvil` v0.1.0 local-first rebuild

## 目的

Phase C までで small loop に寄せたコア経路に合わせて、テスト対象を最小化する。
コア責務に含まれない機能の suite は、v0.1.0 の通常 test inventory から外す。

## 1. v0.1.0 コア test inventory

Phase D 完了時点で通常 test inventory に残すのは次だけ。

- `tests/config_tests.rs`
- `tests/ollama_client_tests.rs`
- `tests/xml_fallback_tests.rs`
- `tests/tool_registry_tests.rs`
- `tests/session_tests.rs`
- `tests/plan_act_tests.rs`
- `tests/recovery_tests.rs`
- `tests/e2e_local_llm.rs`

理由:

- いずれも `workspace/v0.1.0/05_phase_a_scope_freeze.md` のコア責務に対応している
- `CLI / config / Ollama / XML fallback / tools / session / Plan/Act / recovery / live E2E`
  を過不足なく見る

## 2. 通常 inventory から外す suite

Phase D では次を通常 test inventory から外す。

- `tests/git_checkpoint_tests.rs`
- `tests/watch_autotest_tests.rs`
- `tests/skills_mcp_parallel_tests.rs`

理由:

- `git checkpoint`
- `watch / autotest`
- `skills / mcp / parallel`

はいずれも v0.1.0 コア責務から外しているため。

これらは削除ではなく、post-core の後段復帰時に再設計する前提とする。

## 3. live E2E の成功条件

`tests/e2e_local_llm.rs` は、単なる scaffold / file existence ではなく、次を確認する。

- file write 系 test では、期待文字列が実ファイルへ書かれている
- session file が保存されている
- Next.js edit 系 test では、`page.tsx` が初期テンプレートではない
- Next.js edit 系 test では、要求した marker が入っている
- `3011` 起動が成功する

## 4. 今後の扱い

Phase E では、このコア test inventory を前提に live 1-run / 5-run を再計測する。
post-core 機能を戻す場合は、その時点で suite を別途復帰または再作成する。

# Orchestration Summary — 2026-04-29 / 2026-04-30 (Epic E 完了)

## 結論

`/orchestrate 468 469 470` を **Option A: 直列実行** で完遂。
**Epic E: Repo Graph & Domain Context** (#448) の構成 3 Issue を develop へ統合。

## 対象 Issue / PR / 統合 commit

| Phase | GitHub Issue | 論理# | PR | develop 統合 commit |
|---|---|---|---|---|
| E-1 | #468 RepoGraph v1 を実装する | #18 | #521 | `1421ea0` |
| E-2 | #469 graph-aware repo context ranking を導入する | #19 | #522 | `c035493` |
| E-3 | #470 ANVIL.md instructions を path-aware にする | #20 | #523 | `8176bdb` |

全 PR は `--squash --delete-branch` で develop に統合、対応 Issue は close 済。

## 統合検証 (develop @ `8176bdb`, anvil v0.4.0)

| チェック項目 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo clippy --all-targets -- -D warnings` | ✅ pass (0 warnings) |
| `cargo test --all` | ✅ **1289 passed / 0 failed / 8 ignored** |
| `cargo build --release` | ✅ 9.0 MiB (`target/release/anvil`) |

## Anvil への影響（Epic E による repo context 強化）

```
Agent::new
  ↓ blocking build (1 回)
RepoGraph (#468) — session-lifetime, regex-based
  - file / import / symbol / likely_covers 関係
  - Rust / Node / Python 対応
  - 永続化: state_root/repo_graph/<id>.json + LRU eviction
  - util::git_hardened SSoT 共有 (DR1-001)
  - std::sync::OnceLock<Regex> で zero-new-dependency (DR2-003)

prompting::repo_context_message
  ↓ #469 で graph-derived scoring 追加
RepoContextInputs<'a> bundled-input 型 (Default 派生)
  - score_lexical (pure) + score_graph (pure) 分離 (SRP)
  - graph_neighbor second pass via HashMap index (O(N+M))
  - 11 weights SSoT in prompting.rs (test_impl_pair=7 / suspected=10 / changed=4 / graph_neighbor=6 / lexical=7 等)
  - sanitize_import_target で path traversal/abs/null/control 拒否 (DR4-002)
  - RepoContextCache key 拡張: (repo_graph_present, last_feedback_kind, suspected_files_fingerprint, touched_files_fingerprint)

ANVIL.md parsing
  ↓ #470 で path-aware 拡張
[paths: <glob>] ブロックを認識
  - touched / suspected / target files に応じて path-scoped instruction を選択的注入
  - global instruction は従来通り常時注入
  - invalid syntax は warning（実行は継続）
```

## 学び・知見

### 直列実行の安定性

- Track 5 は明確な階層構造 (graph 基盤 → ranking 適用 → instruction scoping) で Option A が最適
- 各段で前段 API を順序通り活用、衝突ゼロ
- API Error は通常通り escalation で対応

### 想定外の挙動と対処

| 事象 | 対処 |
|---|---|
| 各 worker が Issue review 後 stuck | escalation + 'a' probe で commit まで完走 |
| #470 worker は max iterations 直前で完走 (約 557 probe) | 通常運用、commit 確認後 PR 作成 |
| CI failure なし（Epic E は 3 PR 全て初回 green） | — |

### Epic E 完了で利用可能になる Anvil 機能

1. **Repo 構造ベースの context 選択**: lexical のみだった repo context が import / test-impl / changed file / graph neighbor の関係性で精度向上
2. **Path-scoped instruction**: ANVIL.md に作業領域別の制約を書け、無関係な指示が prompt を圧迫しない
3. **将来の Track 6 (Observability) の素材**: graph data がそのまま evaluation log の repo context fingerprint として使える

## 累積（Epic A + B + C + D + E）

| Epic | Issue 数 | 実行戦略 | 主な追加 |
|---|---|---|---|
| Epic A | 7 (含 #443) | Option A 直列 | FeedbackFrame / Precaution / Reminder / Recovery |
| Epic B | 6 | Option B 段階並列 | AnvilScore / Tester / Tmp Workspace / Sandbox |
| Epic C | 3 | Option A 直列 | CaseRecord / Retrieval / AntiPattern |
| Epic D | 3 | Option A 直列 | AgentSkill trait / Verifier migration / Trust tier |
| Epic E | 3 | Option A 直列 | RepoGraph / graph-aware ranking / path-aware ANVIL.md |
| **計** | **22 Issue / 25 feature PR** | — | — |

## 成果物

- ソースコード:
  - `src/repo_graph/mod.rs` (#468 NEW, RepoGraph)
  - `src/util/git_hardened.rs` (#468 NEW, hardened git invocation SSoT)
  - `src/agent/prompting.rs` 拡張 (#469 graph-aware ranking + #470 path-aware ANVIL.md parsing)
  - `src/agent/loop_run/turn.rs` 統合 (#469 / #470)
- テスト: `tests/repo_graph_smoke.rs`, `tests/repo_context_ranking_smoke.rs`, `tests/anvil_md_path_aware_smoke.rs`
- 計画/報告: `workspace/orchestration/runs/2026-04-29-epic-e/{plan,summary}.md`

## 次の Track 候補

`workspace/v0.1.1/feature.md` Track 整理:

- **Track 6: Observability / Eval** (#471 #472 #473) — structured evaluation log、A/B harness、dataset export。Epic A〜E の成果を観測・評価する基盤。Epic E の RepoGraph data も評価メタデータとして活用可

Track 6 完了で Anvil v0.5.0 / v0.6.0 release 候補（main へ）。

main へのリリースは前回同様 develop → main PR で対応可能。

# Orchestration Summary — 2026-04-29 (Epic D 完了)

## 結論

`/orchestrate 465 466 467` を **Option A: 直列実行** で完遂。
**Epic D: Agentic Skills Layer** (#447) の構成 3 Issue を develop へ統合。

## 対象 Issue / PR / 統合 commit

| Phase | GitHub Issue | 論理# | PR | develop 統合 commit |
|---|---|---|---|---|
| D-1 | #465 AgentSkill trait + skill registry | #15 | #504 | `72ded6f` |
| D-2 | #466 PrecautionSkill / VerifierSkill registry 移植 | #16 | #505 | `71849d8` |
| D-3 | #467 SkillTrustTier permission enforcement | #17 | #506 | `521faf3` |

全 PR は `--squash --delete-branch` で develop に統合、対応 Issue は close 済。

## 統合検証 (develop @ `521faf3`)

| チェック項目 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo clippy --all-targets -- -D warnings` | ✅ pass (0 warnings) |
| `cargo test --all` | ✅ **1240 passed / 0 failed / 8 ignored** |
| `cargo build --release` | ✅ 8.9 MiB (`target/release/anvil`) |

## Anvil への影響（Epic D による skill architecture 確立）

```
turn.rs (10,952 LOC) からの直接 dispatch:
  - Reminder (#452)
  - Tester (#459)
  - CaseRecord (#462)
  - CaseRetrieval (#463)
  - AntiPattern (#464)

  ↓ Epic D で skill architecture 統合

src/agent/skills/mod.rs (#465)
  - AgentSkill trait (name / applicability / execute / termination + tier)
  - SkillRegistry (登録 / applicability 評価 / 実行ログ)
  - SkillTrigger (IterationInternal / MessageBuild / PostLoop)
  - SkillExecuteError (project-local、anyhow 非依存)

  ↓ #466 で Verifier 移植

VerifierSkill (post-loop hook で AnvilScore + AutoTest 統合 dispatch)
  ReminderSkill (基盤として #465 で薄い trait adapter)

  ↓ #467 で permission tier 追加

SkillTrustTier { BuiltInReadOnly, BuiltInCanWriteTemp, BuiltInCanRequestBash, BuiltInCanRequestWrite }
  - SkillRegistry::invoke の applicability 直後で tier check
  - read-only skill による誤 Write/Edit を runtime レベルで block
  - SkillOutput::PermissionDenied + FeedbackKind::SkillPermissionDenied + agent.skill.permission_denied jsonl event
```

## 学び・知見

### 直列実行の安定性

- Track 4 はちょうど 3 段の階層構造 (foundation → migration → policy enforcement) で、Option A が最適
- 各段で skill registry の API が拡張され、後続 issue が前段 API を素直に活用
- 衝突ゼロ、API Error は通常通り escalation で対応

### 想定外の挙動と対処

| 事象 | 対処 |
|---|---|
| #465 worker が Issue review 後 stuck | escalation + 'a' probe で commit まで完走 |
| #466 worker も同様 stuck | 同上 |
| #467 worker は probe 数回で完走 | 通常運用 |
| CI failure なし（Epic D は 3 PR 全て初回 green） | — |

### Epic D 完了で利用可能になる Anvil 機能

1. **Skill 統一 dispatch**: turn.rs の `working_memory_message` / 各 hook 経路から `SkillRegistry::invoke` 経由で Reminder/Verifier を呼べる
2. **Permission boundary 明示**: trust tier で skill が触れる範囲が runtime で enforced、誤 Write を防止
3. **将来拡張の足場**: Tester/CaseRecord/CaseRetrieval/AntiPattern も同じ trait に乗せれば turn.rs 直書き分岐を整理可能

## 累積（Epic A + B + C + D）

| Epic | Issue 数 | 実行戦略 | 主な追加 |
|---|---|---|---|
| Epic A | 7 (含 #443) | Option A 直列 | FeedbackFrame / Precaution / Reminder / Recovery |
| Epic B | 6 | Option B 段階並列 | AnvilScore / Tester / Tmp Workspace / Sandbox |
| Epic C | 3 | Option A 直列 | CaseRecord / Retrieval / AntiPattern |
| Epic D | 3 | Option A 直列 | AgentSkill trait / Verifier migration / Trust tier |
| **計** | **19 Issue / 22 feature PR** | — | — |

## 成果物

- ソースコード:
  - `src/agent/skills/mod.rs` (#465 NEW, AgentSkill trait + registry)
  - `src/agent/skills/reminder_skill.rs` (#465 NEW, ReminderSkill adapter)
  - `src/agent/loop_run/verifier_skill.rs` (#466 NEW, VerifierSkill)
  - `src/agent/skills/mod.rs` 拡張 (#467 SkillTrustTier)
- テスト: `tests/agent_skill_registry_smoke.rs`, `tests/skill_trust_tier_smoke.rs`
- 計画/報告: `workspace/orchestration/runs/2026-04-29-epic-d/{plan,summary}.md`

## 次の Track 候補

`workspace/v0.1.1/feature.md` Track 整理:

- **Track 5: Repo Context** (#468 #469 #470) — RepoGraph、graph-aware ranking、path-aware ANVIL.md。他 Track と独立で並列着手可
- **Track 6: Observability / Eval** (#471 #472 #473) — structured evaluation log、A/B harness、dataset export。Epic A〜D の成果を観測・評価する基盤
- 残り Tracks（5, 6）を完了すれば Anvil v0.4.0 として main に release 候補

main へのリリースは前回同様 develop → main PR で対応可能。

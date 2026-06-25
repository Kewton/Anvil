# Phase0〜Phase7 実施結果

作成日: 2026-06-25

対象計画:

- `workspace/mvp/eval/002/minimal_source_safeguard_gap_inventory.md`
- `workspace/mvp/eval/002/minimal_source_safeguard_phase_test_plan.md`
- `workspace/mvp/eval/002/minimal_source_safeguard_work_breakdown.md`

## 実装済み

| Phase | 実施内容 | 主な対応SG |
|---|---|---|
| Phase 0 | SG-01〜SG-38 の traceability test を追加 | 全SG |
| Phase 1 | post-tool early success、prompt内 `Required final artifacts` 抽出、eval expected_artifacts の prompt contract 注入 | SG-01, SG-02, SG-03, SG-06, SG-33 |
| Phase 2 | empty response / completion without write / missing tool call feedback、XML fallback prompt、tool-call preamble除去 | SG-04, SG-05, SG-10, SG-12, SG-27, SG-28, SG-29, SG-30 |
| Phase 3 | verify commandの危険構文拒否、Next.js dependency missing分類、invalid planner output corrective retry、plan/ultra lint強化 | SG-13, SG-17, SG-22, SG-37 |
| Phase 4 | `max_iterations` と missing tool call の eval failure classificationを追加 | SG-26 |
| Phase 5 | artifact path extraction の `../`, absolute, symlink escape, `.anvil`, `target`, `node_modules` 拒否を追加 | SG-03, SG-06, SG-33 |
| Phase 6 | ultra phase promptに original goal/profile/style/intent/required artifacts を継承し、non-final profile failureを即停止 | SG-23, SG-24 |
| Phase 7 | TUI slash の実コマンド形、Rust/Python/eval dry-run、provider shape既存回帰を確認 | SG-26, SG-29, SG-33 |

## 明示 defer

次のSGは traceability table に `defer:別計画` として残した。理由は、今回の直接原因である max_iterations / provider shape / eval分類とは別の tool-wide または profile-wide の移植面積を持つため。

- SG-07: frontend relative import scanner
- SG-08, SG-36: Edit fallback / already-applied / normalized anchor
- SG-11, SG-35: workspace policy と Read/Glob/Grep ignore/output上限
- SG-14, SG-15, SG-16: typed StepKind / semantic plan lint
- SG-18: per-step / repair iteration cap config
- SG-19, SG-20, SG-21: progress-aware bounded repair / exhausted report / verification aggregation
- SG-25: Next.js Tailwind/rootDir 等の profile contract拡張
- SG-31: compaction evidence protection
- SG-32, SG-38: Bash timeout/process group/output shaping
- SG-34: data profile raw-input protection

## 検証結果

- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings`
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`
- `python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes minimal-loop,step-plan --runs 1 --run-root /tmp/anvilminimal-eval-dry --dry-run`
- `rg "Required final artifacts" /tmp/anvilminimal-eval-dry/runs -n`

## 注意

- Live provider smoke は network/API key依存のため、この実施では ignored test のまま未実行。
- TUI PTY smoke は `ANVIL_PTY_TESTS=1` が必要な ignored test のため、この実施では通常 suite のみ確認。
- 既存 worktree には本作業前から `Cargo.toml`, `Cargo.lock`, `scripts/codex_orchestrate.py`, `tests/test_codex_orchestrate.py` 等の unrelated dirty がある。本実施では触っていない。

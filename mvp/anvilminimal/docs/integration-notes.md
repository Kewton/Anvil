# Integration Notes

## Repair Pressure Integration (2026-07-11)

The following pre-existing gaps were observed while consolidating repair
pressure. They are recorded here because this integration preserves behavior
and does not change thresholds, events, payloads, or terminal reasons.

### No-Progress Does Not Raise Write Pressure [CLOSED]

The integration originally preserved a gap where the no-progress streak drove
`no_progress_feedback` and `model_stagnation:no_progress_recorded` without
activating `write_required`. T26b2 closes it: the feedback limit now promotes
directly, while carried or short-budget read-only pressure combines with
no-progress turns to reach the same write-required threshold earlier. Compile
repair non-edit turns use the existing target selection, write enforcement,
carryover, and exhaustion paths. The gate #2 #5 shape is fixed in corpus and
loop-level regression tests.

### Dependency Setup Can Stall Network-Restricted Tests [OPEN (low priority)]

`plan_run_nextjs_game_setup_only_fails_inferred_obligation` can enter the real
Node dependency-setup path. Under a network-restricted test environment,
`npm install` can wait for the existing 600-second setup timeout. The test
passes when dependency setup can complete. This integration does not change
the setup decision or timeout.

## Frozen Exception: Template-Owned Implement Steps (2026-07-12) [CLOSED]

UAT `test0712_bs_001` run 3 produced an `ensure-port-scripts` step classified
as `implement`. Its package-script objective was already satisfied, but the
Task 24 preset conversion and short-circuit gates both required `setup`, so the
run exhausted through `model_stagnation:no_progress_recorded` four times.

This bounded exception replaces the kind/name gate only for steps that
explicitly reference template-owned artifacts through `expected_paths`, the
planner-authored instruction, the step id, or deterministic verify commands.
Package scripts/ports and known scaffold configuration files qualify;
ambiguous text such as "configure the project" does not. A route-bound game
implementation and `npm run build` alone remain outside the predicate. The
generic profile contract appended by the runner is also excluded so it cannot
reclassify ordinary implementation steps.

The exception permits kind-independent preset verification conversion,
profile-check prechecks, and the existing verification-bearing no-progress
feedback for this artifact class. It does not change the repair-pressure
transition table, escalation thresholds, event names or payloads, or terminal
reason strings. Converted verification paths are evidence dependencies rather
than mutating ownership claims, so they do not conflict with an implementation
step that owns the same path; duplicate ownership between mutating steps still
fails lint. A failing port check still enters the executor repair path.

## Declarative Knowledge Extraction (2026-07-12) [CLOSED]

The following values moved without byte changes to embedded TOML files:

- `src/planner/profiles/nextjs/knowledge.toml`: preset UltraPlan phase ids and
  prompts; deterministic scaffold, port, build, and implementation matcher
  tokens; setup and template-owned artifact tokens; state-binding and contract
  attribute guidance; canonical package scripts, required hooks, scaffold
  paths, and scaffold file bodies.
- `src/minimal_loop/evidence_knowledge.toml`: visible-surface, input-handler,
  state-update, adversary, motion, score, failure, restart, and persistence
  vocabulary; goal stopwords and Japanese-to-English translations; behavioral
  evidence keys and ordered Next.js repair target candidates.

Both files are embedded with `include_str!`, parsed once through `OnceLock`,
and have no runtime fallback. Migration golden tests fixed the old Rust values
before the final reference switch; retained tests cover the embedded body,
ordering, translation lookup, contract wording, and canonical JSON structure.
No functional issue was discovered during this extraction. The initial
sandboxed full test run could not bind local browser-probe ports; the same test
command passed when run with local-port permission, without a code change.

## Post-gate queue (2026-07-12)

### Panic Boundary [QUEUED]

Gate後タスクT26、最優先。`catch_unwind` でpanicを終端イベントとrecovery
noteへ変換する境界を追加し、fault-injectionテストを完了条件に含む。

### No-Progress to Write-Required Promotion [CLOSED]

T26b2で`repair_pressure.rs`の遷移表を変更し、no-progress上限または
持ち越し・短予算による前倒し圧力との合算で`write_required`へ昇圧するよう
閉鎖した。gate #2の#5（コンパイル修復のno_progress枯渇）をcorpusと
loop-level回帰テストに固定した。バッチ検証では同クラスの件数を継続計測する。

### Undiagnosed package.json Observation [CLOSED]

`test0712_bs_001` #6の観測。ゲート2回＋直近4計測セットで再発なし。
n=1・未再現につきwatch解除。再発時は当該runの`repair.md`で裁定する。

T27 status: REOPENED -> CLOSED (root-caused, fixed by T27)。
`test0712_gab_001` #10でinteraction失敗単独時に診断済みのroute-bound sourceが
修復ターゲットへ接続されず、`required_path`の先頭`package.json`へ落ちる同属事象を
再確認した。確定`state_binding_diagnosis`のpathを最優先ターゲットへ接続し、
interaction failureのevidence mappingとcontract hook guidanceも接続して閉鎖した。

### Dependency Setup Network Stall [OPEN (low priority)]

依存セットアップのネットワーク停滞は低優先度のままOPENとする。既存の
600秒setup timeoutとネットワーク制限下の待機挙動はこの棚卸しでは変更しない。

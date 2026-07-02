# T2-4 Minimal Loss Cluster Triage

Date: 2026-06-12 JST

対象:

- Legacy root: `.anvil/benchmarks/20260612T024532-70925`
- Minimal root: `.anvil/benchmarks/20260612T145227-40162`
- Compare root: `.anvil/benchmarks/20260612T145227-combined-t2-4-fixed-rerun`

本調査は llm-io/session ログと成果物の比較のみ。minimal loop、parser、prompt、success_check は変更していない。

## Summary

対象シナリオ:

- `new-python-csv-small`: minimal 0/5 vs legacy 5/5
- `fix-js-date-helper`: minimal 0/5 vs legacy 5/5
- `fix-rust-parser-error`: minimal 1/5 vs legacy 5/5
- `fix-readme-command`: minimal 2/5 vs legacy 5/5
- `fix-python-slugify`: minimal 2/5 vs legacy 4/5

minimal の失敗 run 分類:

| class | count | scenarios |
|---|---:|---|
| `no_edit_loop` | 13 | `new-python-csv-small` 3、`fix-rust-parser-error` 4、`fix-readme-command` 3、`fix-python-slugify` 3 |
| `check_failed` | 6 | `new-python-csv-small` 2、`fix-js-date-helper` 4 |
| `max_iterations_with_success_artifact` | 1 | `fix-js-date-helper` run-5 |
| `anchor_mismatch` | 0 | none observed |

主因:

- 想定していた Edit anchor mismatch は、今回の loss cluster では主要因ではなかった。
- より強いパターンは「空 workdir を `Bash`/`Glob`/`Read` で確認したあと、作成意図を自然文で述べて終了し、`Write` が出ない」no-edit loop。
- 一方で `fix-js-date-helper` と `new-python-csv-small` の一部は、成果物はできているが `min_lines` の過剰条件で落ちている。

## Scenario Findings

### `new-python-csv-small`

Result:

- legacy: 5/5 success
- minimal: 0/5 success

Branch point:

- legacy は初回から `Write:tools/csv_stats.py` に到達し、全 run で対象ファイルを作成。
- minimal run-1/run-2/run-3 は `Bash` 等で空 workdir を見た後、作成意図を本文で返して終了。対象ファイルなし。
- minimal run-4/run-5 は `tools/csv_stats.py` を作成しているが24行で、`min_lines:25` に1行だけ届かず失敗。

Classification:

- `no_edit_loop`: 3
- `check_failed`: 2

判定:

- 設計の限界というより、最初の `Write` に入れない no-edit loop と check 過剰条件の混在。
- Phase 3 admission 候補。対象は `new-python-csv-small` の no-edit 3 run。

候補機構:

- 空 workdir かつ明示的な作成タスクで、assistant が「作成します」と述べるだけで tool call を出さない場合の1回限り ephemeral feedback。
- full legacy recovery ではなく、最小の no-tool-after-create-intent feedback として測るべき。

### `fix-js-date-helper`

Result:

- legacy: 5/5 success
- minimal: 0/5 success

Branch point:

- legacy は全 run で `Write:src/dateRange.js` を実行。
- minimal run-1/run-4 は `src/dateRange.js` を作成しており、機能的には成立している可能性が高いが15-18行で `min_lines:25` に届かない。
- minimal run-5 は `minimal loop reached max_iterations (12)` で `rc=1`。ただし `meta.json.success_check_success=true` で、`summary.tsv` の `extras_json` は null 系になっている。

Classification:

- `check_failed`: 4
- `max_iterations_with_success_artifact`: 1

判定:

- これは minimal の機構負けとして扱う前に check と集計不一致を直すべき。
- Task13 の対象。Phase 3 admission にはまだ進めない。

### `fix-rust-parser-error`

Result:

- legacy: 5/5 success
- minimal: 1/5 success

Branch point:

- legacy は `Write:src/config_parser.rs` へ到達。
- minimal の成功 run は、探索後に `mkdir` と `Write:src/config_parser.rs` へ入っている。
- minimal の失敗4 run は `Glob`/`Read`/`Bash ls` で空 workdir を確認したあと、自然文で作業意図を述べて終了。対象ファイルがない。

Classification:

- `no_edit_loop`: 4

判定:

- anchor mismatch ではなく、Write 未発火。
- Phase 3 admission 候補。対象 scenario ID: `fix-rust-parser-error`。

候補機構:

- 「対象ファイルが存在しないが、prompt が create/fix を要求している」状態で no-tool response を返した場合の targeted feedback。

### `fix-readme-command`

Result:

- legacy: 5/5 success
- minimal: 2/5 success

Branch point:

- legacy は全 run で `README.md` を作成。
- minimal run-3/run-5 は `Write:README.md` で成功。
- minimal run-1/run-2/run-4 は `Bash ls` の後、README を作るという本文だけで終了し、`Write` が出ない。

Classification:

- `no_edit_loop`: 3

判定:

- no-edit loop。README という単純 artifact でも、最初に探索へ寄ると `Write` を落とす。
- Phase 3 admission 候補。対象 scenario ID: `fix-readme-command`。

### `fix-python-slugify`

Result:

- legacy: 4/5 success
- minimal: 2/5 success

Branch point:

- legacy はほぼ全 run で `Write:src/slugify.py` を実行。1 run は行数条件で失敗。
- minimal run-1/run-2 は `mkdir -p src` から `Write:src/slugify.py` へ到達し成功。
- minimal run-3/run-4/run-5 は探索後に自然文で終了し、対象ファイル未作成。

Classification:

- `no_edit_loop`: 3

判定:

- no-edit loop。directory creation が絡む場合でも、成功 run は素直に `mkdir` + `Write` できているため、能力限界というより状態遷移の不安定さ。
- Phase 3 admission 候補。対象 scenario ID: `fix-python-slugify`。

## Why Some fix-* Scenarios Win

比較対象:

- minimal wins: `fix-python-retry-policy`, `fix-shell-safe-clean`
- minimal losses: `fix-rust-parser-error`, `fix-readme-command`, `fix-python-slugify`

観測差:

| pattern | minimal wins | minimal losses |
|---|---|---|
| first useful action | early `Write` to target file | `Bash`/`Glob`/`Read` exploration first |
| workdir state | target path is acted on directly | empty workdir confirmation after which no `Write` follows |
| failure shape | mostly legacy line-count or blocked command issues | no artifact, or line-count false negative |
| anchor mismatch | not observed | not observed |

仮説:

- minimal は既存コード修正が一様に弱いわけではない。
- 負けは Edit anchor 精度より、「探索から作成へ戻るための feedback がない」ことに集中している。
- target path が明示され、初手か2手目で `Write` する run は強い。

## Legacy Mechanisms Observed

legacy 成功 run で効いている可能性がある機構:

- Objective Contract / required artifact tracking
- artifact-directed recovery
- verifier/repair loop
- tool policy that target artifact creationへ戻す

ただし、そのまま移植すべきとは限らない。`scaffold-rust-cli` では legacy recovery が `src/args.rs` に偏り、全体 scaffold を完成できていない。Phase 3 で admission するなら、full recovery ではなく、今回観測された failure mode に絞った小さい機構から測るべき。

## Admission Candidates

admission に進める候補:

| scenario | failure to target | candidate mechanism |
|---|---|---|
| `new-python-csv-small` | no edit after create intent in empty workdir | no-tool-after-create-intent feedback |
| `fix-rust-parser-error` | no artifact after exploration | target-path write nudge / no-tool feedback |
| `fix-readme-command` | no artifact after `ls` | no-tool feedback |
| `fix-python-slugify` | no artifact after exploration | no-tool feedback plus mkdir/write encouragement |

保留:

- `fix-js-date-helper`: check と Task13 の summary/meta 不一致を先に直す。
- `new-python-csv-small` run-4/run-5: check の `min_lines:25` false negative を別PRで検討する。

## Conclusion

minimal の大差負けは、parser 修正後は parser 由来ではない。主要な構造は以下。

1. check が過剰で、実装済み成果物を落としている false negative がある。
2. 空 workdir 探索後に `Write` へ戻れず、自然文終了する no-edit loop がある。
3. 今回の対象では anchor mismatch は主因ではない。

Phase 3 の最初の候補は「既存コード修正用の大きな legacy 機構」ではなく、no-tool-after-create-intent を1回だけ補正する小さい feedback 機構が妥当。

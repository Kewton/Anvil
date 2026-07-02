# Phase 0 Fixture Implementation Result

作成日: 2026-06-30

対象:

- `workspace/mvp/uat/001/README.md` Phase 0
- `workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md`

## 実施内容

`test0630_001` の UAT failure を repo 内の self-contained fixture として固定した。

追加ファイル:

- `mvp/anvilminimal/scripts/eval_lib/uat_regression.py`
- `mvp/anvilminimal/tests/eval/fixtures/uat_001/test0630_001_regression.json`
- `mvp/anvilminimal/tests/eval/test_uat_regression_fixtures.py`

## 固定した failure shape

fixture は以下を構造として検出する。

- phase 4 scaffold failure
- `verify command may not use shell control syntax`
- `phase_scaffold_error`
- `recovery_prompt_saved`
- recovery UltraPlan YAML missing
- completed phase の `npm run build` pass
- browser readiness HTTP 500
- `required_artifacts_satisfied_after_tool` による path-only early stop

Space Invaders 固有文字列ではなく、event / artifact / browser evidence の構造で判定する。

## 検証結果

```bash
pytest mvp/anvilminimal/tests/eval/test_uat_regression_fixtures.py
```

結果:

```text
5 passed
```

```bash
pytest mvp/anvilminimal/tests/eval
```

結果:

```text
187 passed, 1 skipped
```

## Phase 0 受入条件

- `phase_scaffold_error`、`recovery_prompt_saved`、recovery UltraPlan YAML missing を fixture で検出できる: 完了
- build pass / browser fail の組み合わせを fixture で表現できる: 完了
- pytest `mvp/anvilminimal/tests/eval` が通る: 完了


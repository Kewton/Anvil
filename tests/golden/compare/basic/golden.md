# compare report

- schema_version: 1
- model_slug: test-model
- baseline_dir: <REPO_ROOT>/tests/golden/compare/basic/baseline
- experiment_dir: <REPO_ROOT>/tests/golden/compare/basic/experiment
- generated_at: 2026-01-01T00:00:00Z
- threshold: 0.05

| metric | baseline (mean [CI]) | experiment (mean [CI]) | delta | delta_pct | verdict |
|---|---|---|---|---|---|
| elapsed_s | 125.000 [112.579, 137.421] (n=3) | 105.000 [92.579, 117.421] (n=3) | -20.000 | -16.00% | ✅ improved |
| error_500_count | n/a | n/a | null | null | ➖ unchanged |
| iter_count | 10.000 [7.516, 12.484] (n=3) | 8.000 [5.516, 10.484] (n=3) | -2.000 | -20.00% | ✅ improved |
| page_tsx_has_game_keywords | n/a | n/a | null | null | ➖ unchanged |
| rc | 0.667 [0.208, 0.939] (n=3) | 1.000 [0.438, 1.000] (n=3) | 0.333 | null | ✅ improved |
| we_total | 0.667 [-0.768, 2.101] (n=3) | 1.000 [1.000, 1.000] (n=3) | 0.333 | null | ℹ️ informational |

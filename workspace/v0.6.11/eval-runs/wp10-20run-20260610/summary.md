# WP10 Evaluation Summary


- run_id: `wp10-20run-20260610`
- total: 20
- pass: 14/20
- high_quality: 14/20
- verification_pass: 15/20
- false_done: 1
- false_missing: 2
- repair_exhausted: 2
- max_iterations: 0

## By Variant

| variant | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| no_pam | 14 | 14 | 20 |

## By Case

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| data_csv | 0 | 0 | 1 |
| docs_runbook | 2 | 2 | 2 |
| fastapi_notes | 0 | 0 | 2 |
| node_csv | 2 | 2 | 2 |
| node_json | 2 | 2 | 2 |
| ops_health | 0 | 0 | 1 |
| python_markdown | 1 | 1 | 1 |
| python_sales | 3 | 3 | 3 |
| research_cache | 0 | 0 | 1 |
| rust_ndjson | 0 | 0 | 1 |
| rust_word | 2 | 2 | 2 |
| toml_merge | 2 | 2 | 2 |

## Non High-Quality Rows

- 5 no_pam rust_ndjson: pass=false exit=verifier_failed note=cargo test failed: ::merge_ndjson_lines;
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^ no `merge_ndjson_lines` in the root

For more information about this error, try `rustc --explain E0432`.
error: could not compile `rust_ndjson` (test "lib") due to 1 previous error
warning: build failed, waiting for other jobs to finish...

- 8 no_pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: ssert 422 == 201
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:16: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_post_notes - assert 422 == 201
1 failed, 1 passed in 0.13s

- 10 no_pam data_csv: pass=false exit=done note=unexpected CSV: ['id,total,same', '1,10,true', '2,25,true']
- 11 no_pam research_cache: pass=false exit=missing_repo_edits note=research brief missing
- 12 no_pam ops_health: pass=false exit=missing_repo_edits note=health report missing
- 19 no_pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: sert 422 == 200
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:16: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_create_note - assert 422 == 200
1 failed, 1 passed in 0.12s


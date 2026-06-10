# WP11 Evaluation Summary


- run_id: `wp11-50run-20260610`
- total: 50
- pass: 38/50
- high_quality: 35/50
- verification_pass: 37/50
- false_done: 2
- false_missing: 5
- repair_exhausted: 8
- max_iterations: 0

## By Variant

| variant | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| no_pam | 19 | 18 | 25 |
| pam | 19 | 17 | 25 |

## By Case

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| data_csv | 0 | 0 | 2 |
| docs_runbook | 4 | 4 | 4 |
| fastapi_notes | 1 | 1 | 6 |
| node_csv | 4 | 4 | 4 |
| node_json | 6 | 6 | 6 |
| ops_health | 0 | 0 | 2 |
| python_markdown | 2 | 1 | 2 |
| python_sales | 8 | 8 | 8 |
| research_cache | 0 | 0 | 2 |
| rust_ndjson | 3 | 3 | 4 |
| rust_word | 4 | 4 | 4 |
| toml_merge | 6 | 4 | 6 |

## Non High-Quality Rows

- 8 no_pam toml_merge: pass=true exit=safe_stop_verifier_missing note=safe_stop_verifier_missing
- 13 no_pam rust_ndjson: pass=false exit=repair_exhausted note=cargo test failed: tered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.00s
     Running unittests src/lib.rs (target/debug/deps/rust_ndjson-1b32907985f896c5)
     Running tests/lib.rs (target/debug/deps/lib-92bb9d52a4db7d5a)
error: test failed, to rerun pass `--test lib`

- 19 no_pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: ssert 422 == 201
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:17: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_post_notes - assert 422 == 201
1 failed, 1 passed in 0.13s

- 20 no_pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: ssert 422 == 201
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:17: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_post_notes - assert 422 == 201
1 failed, 2 passed in 0.15s

- 23 no_pam data_csv: pass=false exit=done note=unexpected CSV: ['id,total,same', '1,10,10', '2,25,25']
- 24 no_pam research_cache: pass=false exit=missing_repo_edits note=research brief missing
- 25 no_pam ops_health: pass=false exit=missing_repo_edits note=health report missing
- 34 pam toml_merge: pass=true exit=repair_exhausted note=repair_exhausted
- 44 pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: ssert 422 == 201
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:16: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_post_notes - assert 422 == 201
1 failed, 1 passed in 0.13s

- 45 pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: sert 422 == 200
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:17: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_create_note - assert 422 == 200
1 failed, 1 passed in 0.13s

- 46 pam fastapi_notes: pass=false exit=repair_exhausted note=pytest failed: assert 422 == 200
E        +  where 422 = <Response [422 Unprocessable Entity]>.status_code

tests/test_app.py:16: AssertionError
=========================== short test summary info ============================
FAILED tests/test_app.py::test_post_note - assert 422 == 200
1 failed, 1 passed in 0.12s

- 47 pam python_markdown: pass=true exit=repair_exhausted note=repair_exhausted
- 48 pam data_csv: pass=false exit=done note=unexpected CSV: ['id,total,same', '1,10,10', '2,25,25']
- 49 pam research_cache: pass=false exit=missing_repo_edits note=research brief missing
- 50 pam ops_health: pass=false exit=missing_repo_edits note=health report missing

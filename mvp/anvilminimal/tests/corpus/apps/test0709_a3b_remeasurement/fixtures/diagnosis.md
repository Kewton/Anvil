# a3b Re-Measurement Diagnosis

- Stale path injection fixture: `test0709_bs_002/test0709_camp_003`, run `019f4641-6e55-7533-aedb-0c8c624b5f9e`.
- FIRST stale foreign occurrence: bs_002 camp_003 events.jsonl line 38, the Bash command argument `cat /Users/<user>/share/work/commandagent_mvp/01/test0709_bs_002/test0709_camp_003/package.json`.
- No earlier tool-output event introduced the foreign literal. The preceding observed events were successful relative `Read` calls for `package.json`; the foreign literal entered as a model-authored Bash command argument.
- Root-anchor/required-path fallback was not reached because this occurrence was a Bash command argument, not a guarded filesystem Write/Edit path rejection.
- Source fix: annotate outside-workspace absolute paths in Bash stdout/stderr with `[outside workspace root — do not reference]`; do not filter output and do not salvage across workspaces.

- Evidence repair fixture: `test0709_bs_001/test0709_camp_003`, run `019f4597-714d-77c0-bece-3a1807232a9f`.
- Evidence repair ladder fired with one changed path, then exhausted without an evidence regeneration decision in the harvested run.
- Implemented fix records a `repair_regeneration` decision for evidence-shaped final acceptance exhaustion when no earlier evidence regeneration decision exists.

- Missing-tool-call class fixture: `test0709_kv-_003`, run `019f44ef-2d86-7531-9735-fcba24a3dbd6`.
- Missing-tool-call class is represented by compact no-change repair followed by regeneration_turn_error:model_stagnation:no_progress_recorded; executor read-only stagnation emitted compact restatement first.
- Implemented fix routes evidence-shaped final acceptance `missing tool call for action prompt` through the evidence no-source-change ladder instead of a silent failure path.

- pre-run probe card: absent from harvested events; absolute_path_rate/corrupted_path_count did not predict setup no-progress or evidence follow-through. The stale absolute path class remains adjacent to absolute-path-rate risk, but the live first occurrence was not a tool-output injection in this fixture.

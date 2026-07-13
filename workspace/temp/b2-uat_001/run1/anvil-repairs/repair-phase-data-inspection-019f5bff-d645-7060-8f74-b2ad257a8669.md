Recover this failed run by producing and executing a focused ultra plan.

Original goal:
data/sales.csv を読み込み、月次×地域の売上集計と全体合計を計算し、無効な行は理由別に除外して件数を明記した上で、要約レポートを作成してください。

Profile: data

Failure scope:
- phase: data-inspection
- step: unknown
- kind: phase_execute_error

Failure evidence:
- step verify-inspection-artifact failed verification after bounded repair: command failed: grep -q 'columns' data/inspection_report.md outcome: CommandFailed status: exit status: 1 elapsed_ms: 25 summary: command did not succeed: grep -q 'columns' data/inspection_report.md stdout: stderr: ; failure_kind=verify_repair_progress_unchanged; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true Paths: - repair prompt saved: .anvil/repairs/repair-veri

Missing paths:
- pipeline/main.py
- output/results.json

Missing capabilities:
- none

Verification commands:
- none

Changed paths:
- none

Repair targets:
- implementation

Required recovery action:
- Inspect the current workspace state first.
- Preserve already useful artifacts.
- Create or repair the missing implementation artifacts.
- Use deterministic verification.
- Do not treat scaffold-only or build-only output as complete.

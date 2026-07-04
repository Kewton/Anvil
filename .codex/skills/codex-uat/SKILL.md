---
name: codex-uat
description: Generate and record UAT checks for Anvil, including CLI, TTY, release, GUI, or real-device steps when needed.
---

# Codex UAT

Use this skill after `develop` has received one or more orchestrated PRs.

## Required Output

Write `workspace/management/runs/<run_id>/uat-report.md` with:

- acceptance scenarios derived from the Issues
- automated checks and results
- manual CLI/TTY/GUI or real-device steps when relevant
- expected results
- evidence to collect on failure
- pass/fail status
- follow-up fix prompt when UAT fails

Treat unclear, unsafe, or non-reproducible UAT as an actionable stop with the
missing evidence called out.

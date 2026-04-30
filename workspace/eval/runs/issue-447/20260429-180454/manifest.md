# Issue 447 Evaluation Manifest

Run ID: `20260429-180454`
Issue: `447`
Epic: `Epic D - Agentic Skills Layer`
Branch: `develop`
Evaluation commit: `545245d88a79604dca513a641a3155af332a235d`
Epic D feature commits: `72ded6f`, `71849d8`, `521faf3`

## Models

| Role | Model |
| --- | --- |
| Main A | `qwen3.6:27b-coding-nvfp4` |
| Main B | `qwen3.5:122b` |
| Sidecar | `qwen3.5:9b` |

## Static Checks

| Command | Log |
| --- | --- |
| `cargo fmt --check` | `raw/cargo-fmt-check.log` |
| `cargo clippy --all-targets -- -D warnings` | `raw/cargo-clippy.log` |
| `cargo test` | `raw/cargo-test.log` |
| `cargo test --test skill_trust_tier_smoke` | `raw/cargo-test-skill-trust-tier-smoke.log` |
| `cargo test --test agent_skill_registry_smoke` | `raw/cargo-test-agent-skill-registry-smoke.log` |

## Practical E2E Commands

### P4-02 VerifierSkill / qwen3.6

Workdirs: `e2e/verifier-python-qwen36`, `e2e/verifier-python-qwen36-r2`, `e2e/verifier-python-qwen36-r3`

```bash
target/debug/anvil -m qwen3.6:27b-coding-nvfp4 \
  --sidecar-model qwen3.5:9b \
  --auto-plan --fresh-session --oneshot --no-footer -y \
  --state-dir e2e/verifier-python-qwen36/.anvil-state \
  -p 'app.py を読んで、greet(name) が正確に "Hello, <name>!" を返すように最小限だけ修正してください。余計なファイルは作らないでください。'
```

### P4-02 VerifierSkill / qwen3.5

Workdirs: `e2e/verifier-python-qwen35`, `e2e/verifier-python-qwen35-r2`, `e2e/verifier-python-qwen35-r3`

```bash
target/debug/anvil -m qwen3.5:122b \
  --sidecar-model qwen3.5:9b \
  --auto-plan --fresh-session --oneshot --no-footer -y \
  --chat-timeout-secs 180 \
  --state-dir e2e/verifier-python-qwen35/.anvil-state \
  -p 'app.py を読んで、greet(name) が正確に "Hello, <name>!" を返すように最小限だけ修正してください。余計なファイルは作らないでください。'
```

### P4-01 Reminder Registry Probe / qwen3.6

Workdirs: `e2e/reminder-registry-qwen36`, `e2e/reminder-registry-qwen36-r2`

```bash
target/debug/anvil -m qwen3.6:27b-coding-nvfp4 \
  --sidecar-model qwen3.5:9b \
  --auto-plan --fresh-session --oneshot --no-footer -y \
  --chat-timeout-secs 120 \
  --state-dir e2e/reminder-registry-qwen36/.anvil-state \
  -p '存在しないコマンド issue447_missing_command_probe を一度だけ実行してください。失敗したら失敗内容を短く報告し、ファイル編集はしないでください。'
```

## Notes

- Follow-up testing added 3 repetitions for the P4-02 VerifierSkill scenario on both main models.
- Full fixed matrix across P0-P4 was not run. This evaluation focused on Issue 447-specific P4 acceptance behavior plus static coverage.
- Ollama access required running outside the sandbox.
- Runtime `.anvil-state` files are intentionally preserved under the run directory for log inspection.

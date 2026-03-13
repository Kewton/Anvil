# Scorecard 2026-03-14

Environment:

- machine: current local development machine
- date: 2026-03-14
- anvil command: `cargo run --quiet -- --help`
- vibe-local command: `python3 /Users/maenokota/share/work/github_kewton/vibe-local/vibe-coder.py --help`

| Scenario | Axis | Anvil | vibe-local | Winner | Notes |
| --- | --- | ---: | ---: | --- | --- |
| Startup latency | FirstUseExperience | 217 ms avg | 190 ms avg | vibe-local | 3 runs each, `--help` path only |
| First-use flow clarity | FirstUseExperience | 4/5 | 5/5 | vibe-local | vibe-local ships install scripts and immediate `vibe-local` command |
| First prompt / follow-up latency readiness | IterationSpeed | 4/5 | 3/5 | Anvil | Anvil has provider streaming contract and lighter current surface; vibe-local has richer features but larger runtime path |
| Interrupt recovery score | StabilityAndRecovery | 4/5 | 5/5 | vibe-local | vibe-local has mature interrupt/session/rollback coverage; Anvil has clean state flow but less battle history |
| Long-session resume score | LongSessionUsability | 3/5 | 5/5 | vibe-local | vibe-local already has compaction/RAG/session tooling; Anvil currently has resume but not compaction |
| UX clarity score | UxClarity | 5/5 | 3/5 | Anvil | `[U]/[A]/[T]`, active step, prompt identity, and resume header are clearer in Anvil |

## Summary

- Anvil already wins on console clarity and architectural cleanliness.
- `vibe-local` still wins on first-use packaging and long-session maturity.
- Startup overhead is close; current `--help` measurements favor `vibe-local` slightly.

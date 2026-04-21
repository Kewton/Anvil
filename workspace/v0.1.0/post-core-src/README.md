Post-core source inventory for `v0.1.0`.

These modules are intentionally moved out of `src/` so the active runtime tree stays core-only while the code is preserved for later reintroduction.

Moved here:
- `agent/parallel.rs`
- `git/`
- `mcp/`
- `skills/`
- `testloop/`
- `tui/`
- `watch/`

They are not compiled or referenced by the current `v0.1.0` core build.

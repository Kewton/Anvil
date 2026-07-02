//! Issue #680 (parent): residual `turn.rs` scaffold.
//!
//! Historically this file housed the assistant-loop dispatcher and
//! its many supporting helpers (peak: ~39,489 LOC). The split has
//! migrated every responsibility to a sibling module under
//! `src/agent/loop_run/`. The file is intentionally left empty: the
//! `mod turn;` registration in `loop_run.rs` is kept so historical
//! line-anchored references in `dev-reports/` / commit messages
//! remain resolvable, but no production code lives here.
//!
//! See the per-module doc comments in `loop_run.rs` for the full
//! responsibility map.

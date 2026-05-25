//! Cross-layer utility helpers shared by `session/` and `agent/`.
//!
//! Modules here must depend only on `std` / very lightweight crates and
//! must not pull anything from `session/` or `agent/`. This keeps the
//! dependency direction one-way (util → session → agent).

pub mod file_classify;
pub mod git_hardened;
pub mod workspace_paths;

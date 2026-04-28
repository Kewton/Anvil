//! Anvil Tester Skill v1 fixture (Issue #459 / Phase 2 E2E).
//!
//! Intentionally tiny so the Tester smoke harness has a workspace package
//! to depend on without dragging in heavy dev-deps. No `#[cfg(test)]` block
//! here either — the Tester is supposed to be the FIRST source of test
//! coverage on this crate.

/// Returns the smallest positive integer. Useful as a stand-in target for the
/// Tester smoke test fixture.
pub fn answer() -> i32 {
    1
}

/// Sums two `i32`s without checked-arithmetic guards. Provides a second
/// surface for the LLM-written smoke test to assert against.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

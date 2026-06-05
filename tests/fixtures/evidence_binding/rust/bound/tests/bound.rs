// Issue #989 fixture: every binding resolves. The crate-under-test import
// matches `[package]`/default lib name, the bin handle matches `[[bin]] name`,
// and the dev-dependency import is excluded from self-import detection.
// Inert fixture data (never compiled).
use slugify::slugify;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn runs_cli() {
    let _ = TempDir::new();
    let bin = env!("CARGO_BIN_EXE_slugify");
    let _ = Command::new(bin).arg("Hello World");
    assert_eq!(slugify("Hello World"), "hello-world");
}

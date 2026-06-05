// Issue #989 fixture: references the binary as `word_counter`, but the package
// default bin is `wordcount`. Inert fixture data (never compiled).
use std::process::Command;

#[test]
fn counts_words() {
    let bin = env!("CARGO_BIN_EXE_word_counter");
    let output = Command::new(bin).arg("--help").output().unwrap();
    assert!(output.status.success());
}

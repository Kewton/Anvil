use anvil::tools::{
    edit_file, exec_in_dir, glob_paths, read_file, search_in_files, unified_diff, write_file,
};
use tempfile::tempdir;

#[test]
fn tool_read_write_edit_cycle_works() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("note.txt");

    write_file(&path, "hello world\n").unwrap();
    assert_eq!(read_file(&path).unwrap(), "hello world\n");

    edit_file(&path, "world", "anvil").unwrap();

    assert_eq!(read_file(&path).unwrap(), "hello anvil\n");
}

#[test]
fn tool_exec_runs_command_in_directory() {
    let dir = tempdir().unwrap();

    let output = exec_in_dir(
        dir.path(),
        &[
            "/bin/sh".to_string(),
            "-lc".to_string(),
            "printf 'ok'".to_string(),
        ],
    )
    .unwrap();

    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, "ok");
}

#[test]
fn tool_glob_search_and_diff_work() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_file(&src.join("a.rs"), "fn main() {}\n").unwrap();
    write_file(&src.join("b.rs"), "fn helper() { println!(\"hi\"); }\n").unwrap();

    let matches = glob_paths(dir.path(), "**/*.rs").unwrap();
    assert_eq!(matches.len(), 2);

    let search = search_in_files(dir.path(), "println!").unwrap();
    assert_eq!(search.len(), 1);
    assert!(search[0].path.ends_with("b.rs"));

    let diff = unified_diff("before\n", "after\n");
    assert!(diff.contains("-before"));
    assert!(diff.contains("+after"));
}

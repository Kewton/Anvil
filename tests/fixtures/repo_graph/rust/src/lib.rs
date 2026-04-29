use std::path::PathBuf;

pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

pub struct Greeter {
    pub root: PathBuf,
}

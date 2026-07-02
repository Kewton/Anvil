pub const COMMIT: &str = env!("ANVIL_BUILD_COMMIT");
pub const DIRTY_RAW: &str = env!("ANVIL_BUILD_DIRTY");
pub const TIMESTAMP: &str = env!("ANVIL_BUILD_TIMESTAMP");
pub const VERSION: &str = env!("ANVIL_VERSION");

pub fn dirty() -> bool {
    DIRTY_RAW == "true"
}

pub fn commit_with_dirty() -> String {
    if dirty() {
        format!("{COMMIT}+dirty")
    } else {
        COMMIT.to_string()
    }
}

pub fn summary_line() -> String {
    format!("Build: {} {}", commit_with_dirty(), TIMESTAMP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_string_contains_embedded_commit_or_unknown() {
        assert!(VERSION.contains(env!("CARGO_PKG_VERSION")), "{VERSION}");
        assert!(
            VERSION.contains(COMMIT) || VERSION.contains("unknown"),
            "{VERSION}"
        );
        assert!(VERSION.contains(TIMESTAMP), "{VERSION}");
    }
}

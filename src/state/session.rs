use std::path::PathBuf;

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub root: PathBuf,
}

impl Session {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            id: format!("sess_{}", Uuid::new_v4().simple()),
            root: root.into(),
        }
    }
}

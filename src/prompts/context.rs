use crate::runtime::trust::SourceType;

#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub source_type: SourceType,
    pub path: Option<String>,
    pub value: String,
}

impl ContextBlock {
    pub fn new(source_type: SourceType, value: impl Into<String>) -> Self {
        Self {
            source_type,
            path: None,
            value: value.into(),
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn render(&self) -> String {
        match &self.path {
            Some(path) => format!(
                "[source={} path={}]\n{}",
                self.source_type, path, self.value
            ),
            None => format!("[source={}]\n{}", self.source_type, self.value),
        }
    }
}

pub fn render_context_blocks(blocks: &[ContextBlock]) -> String {
    let mut sorted = blocks.to_vec();
    sorted.sort_by_key(|block| (block.source_type.precedence(), block.path.clone()));
    sorted
        .iter()
        .map(ContextBlock::render)
        .collect::<Vec<_>>()
        .join("\n\n")
}

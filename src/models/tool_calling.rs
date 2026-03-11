use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolUseOptions {
    pub temperature: f32,
    pub max_context_tokens: usize,
    pub keep_alive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeModelResponse {
    Message(String),
    ToolCalls(Vec<NativeToolCall>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolCall {
    pub id: Option<String>,
    pub name: String,
    pub arguments: Value,
}

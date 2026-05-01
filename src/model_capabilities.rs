#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelCapabilities {
    pub(crate) family: ModelFamily,
    pub(crate) native_tool_calls: bool,
    pub(crate) streaming_tool_calls: bool,
    pub(crate) non_streaming_hard_timeout_secs: Option<u64>,
    pub(crate) read_after_small_edit_protocol: bool,
    pub(crate) finish_after_edit_format_error: bool,
    pub(crate) deterministic_edit_after_format_error: bool,
    pub(crate) focused_edit: Option<FocusedEditCapabilities>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FocusedEditCapabilities {
    pub(crate) pre_read_timeout_secs: u64,
    pub(crate) pre_read_max_predict: usize,
    pub(crate) post_read_timeout_secs: u64,
    pub(crate) post_read_max_predict: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFamily {
    Qwen35,
    Qwen36,
    Generic,
}

const QWEN35_NON_STREAMING_HARD_TIMEOUT_SECS: u64 = 90;
const FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS: u64 = 30;
const FOCUSED_EDIT_PRE_READ_MAX_PREDICT: usize = 320;
const FOCUSED_EDIT_POST_READ_TIMEOUT_SECS: u64 = 45;
const FOCUSED_EDIT_POST_READ_MAX_PREDICT: usize = 320;

const LOCAL_QWEN_FOCUSED_EDIT: FocusedEditCapabilities = FocusedEditCapabilities {
    pre_read_timeout_secs: FOCUSED_EDIT_PRE_READ_TIMEOUT_SECS,
    pre_read_max_predict: FOCUSED_EDIT_PRE_READ_MAX_PREDICT,
    post_read_timeout_secs: FOCUSED_EDIT_POST_READ_TIMEOUT_SECS,
    post_read_max_predict: FOCUSED_EDIT_POST_READ_MAX_PREDICT,
};

impl ModelCapabilities {
    const fn generic() -> Self {
        Self {
            family: ModelFamily::Generic,
            native_tool_calls: false,
            streaming_tool_calls: true,
            non_streaming_hard_timeout_secs: None,
            read_after_small_edit_protocol: false,
            finish_after_edit_format_error: false,
            deterministic_edit_after_format_error: false,
            focused_edit: None,
        }
    }
}

pub(crate) fn model_capabilities(model: &str) -> ModelCapabilities {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.starts_with("qwen3.5:") {
        return ModelCapabilities {
            family: ModelFamily::Qwen35,
            native_tool_calls: false,
            streaming_tool_calls: false,
            non_streaming_hard_timeout_secs: Some(QWEN35_NON_STREAMING_HARD_TIMEOUT_SECS),
            read_after_small_edit_protocol: true,
            finish_after_edit_format_error: true,
            deterministic_edit_after_format_error: true,
            focused_edit: Some(LOCAL_QWEN_FOCUSED_EDIT),
        };
    }
    if normalized == "qwen3.6:27b-coding-nvfp4" {
        return ModelCapabilities {
            family: ModelFamily::Qwen36,
            native_tool_calls: true,
            streaming_tool_calls: true,
            non_streaming_hard_timeout_secs: None,
            read_after_small_edit_protocol: true,
            finish_after_edit_format_error: false,
            deterministic_edit_after_format_error: false,
            focused_edit: Some(LOCAL_QWEN_FOCUSED_EDIT),
        };
    }
    if normalized.starts_with("qwen3.6:") {
        return ModelCapabilities {
            family: ModelFamily::Qwen36,
            native_tool_calls: false,
            streaming_tool_calls: true,
            non_streaming_hard_timeout_secs: None,
            read_after_small_edit_protocol: true,
            finish_after_edit_format_error: false,
            deterministic_edit_after_format_error: false,
            focused_edit: Some(LOCAL_QWEN_FOCUSED_EDIT),
        };
    }
    ModelCapabilities::generic()
}

#[cfg(test)]
mod tests {
    use super::{ModelFamily, model_capabilities};

    #[test]
    fn qwen35_capabilities_preserve_local_recovery_behavior() {
        let capabilities = model_capabilities("qwen3.5:122b");
        assert_eq!(capabilities.family, ModelFamily::Qwen35);
        assert!(!capabilities.native_tool_calls);
        assert!(!capabilities.streaming_tool_calls);
        assert_eq!(capabilities.non_streaming_hard_timeout_secs, Some(90));
        assert!(capabilities.read_after_small_edit_protocol);
        assert!(capabilities.finish_after_edit_format_error);
        assert!(capabilities.deterministic_edit_after_format_error);
        let focused = capabilities.focused_edit.unwrap();
        assert_eq!(focused.pre_read_timeout_secs, 30);
        assert_eq!(focused.pre_read_max_predict, 320);
        assert_eq!(focused.post_read_timeout_secs, 45);
        assert_eq!(focused.post_read_max_predict, 320);
    }

    #[test]
    fn qwen36_coding_model_uses_native_tools_and_focused_edit_protocol() {
        let capabilities = model_capabilities("qwen3.6:27b-coding-nvfp4");
        assert_eq!(capabilities.family, ModelFamily::Qwen36);
        assert!(capabilities.native_tool_calls);
        assert!(capabilities.streaming_tool_calls);
        assert!(capabilities.read_after_small_edit_protocol);
        assert!(capabilities.focused_edit.is_some());
        assert!(!capabilities.finish_after_edit_format_error);
    }

    #[test]
    fn qwen36_unknown_size_keeps_family_protocol_without_native_tools() {
        let capabilities = model_capabilities("qwen3.6:7b");
        assert_eq!(capabilities.family, ModelFamily::Qwen36);
        assert!(!capabilities.native_tool_calls);
        assert!(capabilities.streaming_tool_calls);
        assert!(capabilities.read_after_small_edit_protocol);
    }

    #[test]
    fn unknown_models_are_generic() {
        let capabilities = model_capabilities("llama3.1:8b");
        assert_eq!(capabilities.family, ModelFamily::Generic);
        assert!(!capabilities.native_tool_calls);
        assert!(capabilities.streaming_tool_calls);
        assert!(!capabilities.read_after_small_edit_protocol);
        assert!(capabilities.focused_edit.is_none());
    }
}

use crate::agent::AgentEvent;
use crate::config::{EffectiveConfig, LlmTranscriptMode};
use crate::provider::{
    AssistantToolCallRecord, ProviderMessage, ProviderMessageRole, ProviderTurnError,
    ProviderTurnRequest,
};
use crate::session::SessionRecord;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
struct TranscriptSessionInfo {
    session_id: String,
    session_name: String,
    cwd: String,
    provider: String,
    model: String,
}

#[derive(Serialize)]
struct TranscriptMessageView {
    role: &'static str,
    content: String,
    image_count: usize,
    tool_call_id: Option<String>,
    assistant_tool_calls: Option<Vec<AssistantToolCallRecord>>,
}

#[derive(Serialize)]
struct TranscriptRequestView {
    model: String,
    stream: bool,
    max_output_tokens: Option<u32>,
    temperature: Option<f64>,
    context_window: Option<u32>,
    messages: Vec<TranscriptMessageView>,
    native_tool_names: Vec<String>,
}

#[derive(Serialize)]
struct TranscriptResponseView {
    text: String,
    agent_events: Vec<AgentEvent>,
    error: Option<String>,
}

#[derive(Serialize)]
struct TranscriptRecord {
    timestamp_ms: u128,
    exchange_kind: &'static str,
    transcript_mode: LlmTranscriptMode,
    outcome: &'static str,
    session: TranscriptSessionInfo,
    request: Option<TranscriptRequestView>,
    response: Option<TranscriptResponseView>,
}

pub(crate) fn append_provider_exchange(
    config: &EffectiveConfig,
    session: &SessionRecord,
    session_name: &str,
    exchange_kind: &'static str,
    request: &ProviderTurnRequest,
    output_text: &str,
    agent_events: &[AgentEvent],
    result: &Result<(), ProviderTurnError>,
) -> io::Result<()> {
    let mode = config.mode.llm_transcript;
    if mode == LlmTranscriptMode::Off {
        return Ok(());
    }

    std::fs::create_dir_all(&config.paths.logs_dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(transcript_path(config, session_name))?;

    let (outcome, error) = match result {
        Ok(()) => ("ok", None),
        Err(ProviderTurnError::Cancelled) => ("cancelled", Some("cancelled".to_string())),
        Err(err) => ("error", Some(err.to_string())),
    };

    let record = TranscriptRecord {
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        exchange_kind,
        transcript_mode: mode,
        outcome,
        session: TranscriptSessionInfo {
            session_id: session.metadata.session_id.clone(),
            session_name: session_name.to_string(),
            cwd: session.metadata.cwd.display().to_string(),
            provider: config.runtime.provider.clone(),
            model: request.model.clone(),
        },
        request: mode.includes_prompt().then(|| TranscriptRequestView {
            model: request.model.clone(),
            stream: request.stream,
            max_output_tokens: request.max_output_tokens,
            temperature: request.temperature,
            context_window: request.context_window,
            messages: request.messages.iter().map(message_view).collect(),
            native_tool_names: request
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
                .unwrap_or_default(),
        }),
        response: mode.includes_response().then(|| TranscriptResponseView {
            text: output_text.to_string(),
            agent_events: agent_events.to_vec(),
            error,
        }),
    };

    serde_json::to_writer(&mut file, &record).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn transcript_path(config: &EffectiveConfig, session_name: &str) -> PathBuf {
    config
        .paths
        .logs_dir
        .join(format!("llm-transcript-{session_name}.jsonl"))
}

fn message_view(message: &ProviderMessage) -> TranscriptMessageView {
    TranscriptMessageView {
        role: match message.role {
            ProviderMessageRole::System => "system",
            ProviderMessageRole::User => "user",
            ProviderMessageRole::Assistant => "assistant",
            ProviderMessageRole::Tool => "tool",
        },
        content: message.content.clone(),
        image_count: message
            .images
            .as_ref()
            .map(|images| images.len())
            .unwrap_or(0),
        tool_call_id: message.tool_call_id.clone(),
        assistant_tool_calls: message.assistant_tool_calls.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EffectiveConfig;
    use crate::provider::{ProviderMessage, ProviderMessageRole, ProviderTurnRequest};
    use tempfile::tempdir;

    #[test]
    fn prompt_only_transcript_omits_response_payload() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut config = EffectiveConfig::default_for_test().unwrap();
        config.paths.logs_dir = tmp.path().join("logs");
        config.paths.cwd = cwd.clone();
        config.paths.session_file = config.paths.session_dir.join("transcript.json");
        config.mode.llm_transcript = LlmTranscriptMode::Prompt;

        let session = SessionRecord::new_named("transcript", cwd).unwrap();
        let request = ProviderTurnRequest::new(
            "qwen".to_string(),
            vec![
                ProviderMessage::new(ProviderMessageRole::System, "sys"),
                ProviderMessage::new(ProviderMessageRole::User, "user"),
            ],
            true,
        );

        append_provider_exchange(
            &config,
            &session,
            "transcript",
            "top_level",
            &request,
            "hello",
            &[],
            &Ok(()),
        )
        .unwrap();

        let path = transcript_path(&config, "transcript");
        let line = std::fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert!(value.get("request").is_some());
        assert!(value.get("response").unwrap().is_null());
    }

    #[test]
    fn full_transcript_includes_response_payload() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut config = EffectiveConfig::default_for_test().unwrap();
        config.paths.logs_dir = tmp.path().join("logs");
        config.paths.cwd = cwd.clone();
        config.paths.session_file = config.paths.session_dir.join("transcript.json");
        config.mode.llm_transcript = LlmTranscriptMode::Full;

        let session = SessionRecord::new_named("transcript", cwd).unwrap();
        let request = ProviderTurnRequest::new(
            "qwen".to_string(),
            vec![ProviderMessage::new(ProviderMessageRole::System, "sys")],
            true,
        );

        append_provider_exchange(
            &config,
            &session,
            "transcript",
            "guarded_retry",
            &request,
            "assistant output",
            &[AgentEvent::Interrupted {
                status: "Interrupted".to_string(),
                interrupted_what: "provider turn".to_string(),
                saved_status: "session preserved".to_string(),
                next_actions: vec!["retry".to_string()],
                elapsed_ms: 1,
            }],
            &Err(ProviderTurnError::Cancelled),
        )
        .unwrap();

        let path = transcript_path(&config, "transcript");
        let line = std::fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["outcome"], "cancelled");
        assert_eq!(value["response"]["text"], "assistant output");
        assert_eq!(value["response"]["error"], "cancelled");
    }
}

//! Working-memory + repo-context prompt messages extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the read-only Agent state projections that build per-prompt
//! conversational messages:
//!
//! - `refresh_working_memory` — re-extract plan constraints from the
//!   active plan file and replace the session's working-memory
//!   constraint list.
//! - `working_memory_message` — render the working-memory snippet
//!   (with per-prompt precaution filtering) as a `ConversationMessage`
//!   when mode policy permits.
//! - `answer_only_fallback_response` — `AnswerOnly` mode's canned read-
//!   only response, selected from the current request text.
//! - `repo_context_message` — produce the cached repo-context system
//!   message (RepoGraph + suspected files + change history), bypassing
//!   when the cache key matches.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::answer_only_mode::answer_only_script_execution_fallback_response;
use super::lifecycle;
use super::photon_feedback_derive::request_explicitly_requests_script_execution;
use super::precaution_relevance::select_precautions_for_prompt;
use super::small_helpers::latest_tool_result_since_last_user;
use crate::agent::prompting;
use crate::session::store::ConversationMessage;

pub(super) fn refresh_working_memory(agent: &mut Agent) {
    let constraints = agent
        .current_plan_contents()
        .ok()
        .flatten()
        .map(|contents| lifecycle::extract_plan_constraints(&contents))
        .unwrap_or_default();
    agent
        .session
        .working_memory
        .replace_constraints(constraints);
}

pub(super) fn working_memory_message(agent: &mut Agent) -> Option<ConversationMessage> {
    if !agent.session.mode_state.policy().include_working_memory {
        return None;
    }
    refresh_working_memory(agent);

    // Issue #453: build the per-prompt precaution slice from
    // (active_precautions × mode × touched_files × last_feedback.suspected_files)
    // before handing it to the renderer. The Reminder Sidecar path
    // (handle_user_message → format_for_prompt() wrapper) keeps using the
    // full active-only list (design judgment #5).
    let suspected_owned: Option<Vec<PathBuf>> = agent
        .session
        .last_feedback
        .as_ref()
        .map(|f| f.suspected_files.clone());
    let precautions_for_prompt = select_precautions_for_prompt(
        &agent.session.working_memory.active_precautions,
        agent.session.mode_state.mode,
        &agent.session.working_memory.touched_files,
        suspected_owned.as_deref(),
    );
    let mut prompt_memory = agent.session.working_memory.clone();
    if let Some(task) = prompt_memory.active_task.as_deref() {
        let visible_task = super::task_contract::model_visible_request_text(task);
        prompt_memory.active_task = (!visible_task.is_empty()).then_some(visible_task);
    }
    prompt_memory
        .format_for_prompt_with_precautions(&precautions_for_prompt)
        .map(ConversationMessage::system)
}

pub(super) fn answer_only_fallback_response(agent: &Agent) -> String {
    let request = super::workspace_access::active_request_text(agent).unwrap_or_default();
    let lower = request.to_ascii_lowercase();
    if request_explicitly_requests_script_execution(&request)
        && let Some(output) = latest_tool_result_since_last_user(&agent.session.messages, "Bash")
    {
        return answer_only_script_execution_fallback_response(output);
    }
    if lower.contains("modepolicy") || lower.contains("構造化状態") {
        return "ファイルは変更せず、読み取り専用で整理します。\n\n利点:\n- モード判断を会話履歴から分離できるため、古い発話や回復プロンプトに引きずられにくい。\n- `repo_edit_required` や fallback 許可などを明示的な実行ポリシーとして扱えるため、ツール制御と品質ゲートを安定させやすい。\n- セッション保存や compaction 後も、必要な状態だけを小さく復元できる。\n\nリスク:\n- 状態更新の境界が曖昧だと、ユーザーの最新意図と ModePolicy がずれる。\n- ポリシーが強すぎると、読み取り専用のスクリプト実行など正当な作業まで止める。\n- LLM の自然言語判断と構造化状態の差分を観測できないと、誤分類の原因調査が難しい。\n\n方向性としては、ModePolicy は構造化状態で保持し、最新ユーザー要求から毎ターン再評価できるようにするのが妥当です。会話履歴へ埋め込むのは補助説明に留め、実際のツール許可と品質条件は構造化フィールドを正とするのが安定します。".to_string();
    }
    if lower.contains("rust") && lower.contains("cli") {
        return "ファイルは変更せず、Rust CLI 化の構成案だけを整理します。\n\n- `Cargo.toml`: crate 名、依存、bin 設定を管理する。\n- `src/main.rs`: 引数解析と終了コード制御だけを置く。\n- `src/cli.rs`: CLI オプション、help、入力検証をまとめる。\n- `src/lib.rs`: 実処理をライブラリ化し、CLI 以外からもテスト可能にする。\n- `tests/cli.rs`: 代表コマンド、異常入力、終了コードを E2E 寄りに検証する。\n- `README.md`: インストール、実行例、検証コマンド、制約を記載する。\n\n方針としては、CLI 表層とドメイン処理を分離し、`cargo test` でロジック、必要なら `assert_cmd` 系でコマンド挙動を確認するのが扱いやすいです。".to_string();
    }
    if lower.contains("readme")
        && (request.contains("要約") || lower.contains("summarize"))
        && let Ok(readme) = std::fs::read_to_string(agent.work_root.join("README.md"))
    {
        let summary = readme
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        return format!(
            "README の要約: {summary}\n\n設計上の課題: README から確認できる情報は概要レベルに限られており、内部構成、実行手順、検証方法、制約、fallback や session 管理の責務分担が文書化されていません。そのため、初見の開発者が変更範囲や品質確認方法を判断しにくい状態です。ファイルは変更していません。"
        );
    }
    "ファイルは変更せず、読み取り専用の回答として整理します。目的、前提、推奨構成、検証方法、残リスクを分け、実装や編集が必要な場合だけ次のターンで明示的に依頼してください。".to_string()
}

pub(super) fn repo_context_message(agent: &mut Agent) -> Option<ConversationMessage> {
    if !agent.session.mode_state.policy().allow_repo_context {
        return None;
    }
    refresh_working_memory(agent);
    let task = agent.session.working_memory.active_task.clone()?;
    let prompt_task = super::task_contract::model_visible_request_text(&task);
    let prompt_task = (!prompt_task.is_empty()).then_some(prompt_task);

    // Issue #469: cache key is widened to include graph ranking inputs.
    let suspected_files: Vec<PathBuf> = agent
        .session
        .last_feedback
        .as_ref()
        .map(|f| f.suspected_files.clone())
        .unwrap_or_default();
    let suspected_strings: Vec<String> = suspected_files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let changed_files: Vec<String> = agent.session.touched_files_at_turn_start.clone();
    let last_feedback_kind: Option<String> = agent
        .session
        .last_feedback
        .as_ref()
        .map(|f| format!("{:?}", f.kind));
    let repo_graph_present = agent.repo_graph.is_some();
    let suspected_fp = super::fingerprint_paths(&suspected_strings);
    let touched_fp = super::fingerprint_paths(&changed_files);

    if let Some(cache) = &agent.repo_context_cache
        && cache.task == task
        && cache.work_root == agent.work_root
        && cache.repo_graph_present == repo_graph_present
        && cache.last_feedback_kind == last_feedback_kind
        && cache.suspected_files_fingerprint == suspected_fp
        && cache.touched_files_fingerprint == touched_fp
    {
        return cache.message.clone();
    }

    let session_id = agent.session_store.session_id().to_string();
    let model = agent.models.main.clone();
    let inputs = prompting::RepoContextInputs {
        repo_graph: agent.repo_graph.as_deref(),
        suspected_files: &suspected_files,
        changed_files: &changed_files,
        session_id: &session_id,
        model: Some(model.as_str()),
    };
    let message =
        prompting::repo_context_message(&agent.work_root, prompt_task.as_deref(), &inputs);
    agent.repo_context_cache = Some(super::RepoContextCache {
        task,
        work_root: agent.work_root.clone(),
        repo_graph_present,
        last_feedback_kind,
        suspected_files_fingerprint: suspected_fp,
        touched_files_fingerprint: touched_fp,
        message: message.clone(),
    });
    message
}

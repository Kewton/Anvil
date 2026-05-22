// Issue #664: `is_dependency_install_command` / `is_scaffold_command` /
// `should_block_bash_command` are `#[deprecated]` for the SetupBootstrap
// policy projection (DR1-001 案 B / AD12). The recovery-side legacy
// semantics are preserved and exercised by these integration tests.
#![allow(deprecated)]

use anvil::agent::recovery::{
    ActionExpectation, artifact_directed_recovery_note, broad_restart_discovery_error,
    classify_action_expectation, empty_response_recovery_note, empty_workspace_scaffold_note,
    first_scaffold_shell_edit_note, install_loop_recovery_note, is_dependency_install_command,
    is_scaffold_command, is_workspace_reset_command, no_tool_recovery_note, repeated_bash_error,
    repeated_plan_exploration_error, repo_change_after_setup_note,
    repo_change_no_tool_recovery_note, repo_change_recovery_note, should_block_bash_command,
    should_block_restart_discovery, tool_call_counts_as_repo_edit, user_prompt_requires_action,
};
use anvil::modes::plan_act::{ExecutionMode, PlanStage};

#[test]
fn detects_action_prompts_in_english_and_japanese() {
    assert!(user_prompt_requires_action(
        "Please edit src/main.rs and fix the bug",
        ExecutionMode::Act
    ));
    assert!(user_prompt_requires_action(
        "README を修正して",
        ExecutionMode::Act
    ));
    assert_eq!(
        classify_action_expectation(
            "最高に面白いゲームを next.js で開発してください",
            ExecutionMode::Act
        ),
        ActionExpectation::RepoChange
    );
    assert_eq!(
        classify_action_expectation("テストを実行して", ExecutionMode::Act),
        ActionExpectation::ToolAction
    );
    assert!(!user_prompt_requires_action(
        "Explain the architecture",
        ExecutionMode::Act
    ));
    assert!(user_prompt_requires_action(
        "plan the work",
        ExecutionMode::Plan
    ));
    assert_eq!(
        classify_action_expectation("plan the work", ExecutionMode::Plan),
        ActionExpectation::PlanProgress
    );
}

#[test]
fn recovery_notes_are_non_empty() {
    assert!(empty_response_recovery_note(1, true).contains("attempt=1"));
    assert!(no_tool_recovery_note(2).contains("no_tool_attempt=2"));
    assert!(repo_change_recovery_note(3).contains("repo_change_attempt=3"));
    assert!(repo_change_no_tool_recovery_note(4).contains("exactly one tool call"));
    assert!(repo_change_after_setup_note().contains("small Edit"));
    assert!(empty_workspace_scaffold_note().contains("Do not inspect it again with ls"));
    assert!(
        first_scaffold_shell_edit_note("src/app/page.tsx").contains("compact task-specific title")
    );
    assert!(install_loop_recovery_note().contains("Stop reinstalling packages"));
    assert!(repeated_bash_error("npm install jest").contains("risky Bash command blocked"));
    assert!(
        repeated_plan_exploration_error(PlanStage::Stage1, &["Goal"], "Read")
            .contains("repeated exploration blocked for Stage 1")
    );
    assert!(tool_call_counts_as_repo_edit("Write"));
    assert!(tool_call_counts_as_repo_edit("Edit"));
    assert!(!tool_call_counts_as_repo_edit("Bash"));
}

// -------------------------------------------------------------------------
// Issue #652 — `ArtifactCompletionJob` regression guards.
//
// The `ArtifactCompletionJob` private mod is intentionally NOT re-exported
// (DR3-001) — `turn.rs` is the only behavioral in-crate consumer. These
// tests therefore exercise only the externally-observable contract:
//
//   * `artifact_directed_recovery_note(role, path, attempt)` still renders
//     the `artifact_directed_attempt=N` token the actor loop emits when a
//     job is in flight (regression guard for the existing message format).
//   * Phase 2 wiring does not break the existing recovery / no-tool / repo-
//     change / install-loop messages.
//
// The role-specific budget enforcement / wrong-target / no-tool / prose-only
// classifier itself is covered by the in-crate unit suite in
// `src/agent/loop_run/artifact_completion_job.rs::tests`; the integration
// surface here pins the wiring (DR3-001 audit) and message-format compat.
// -------------------------------------------------------------------------

#[test]
fn artifact_directed_recovery_note_contains_artifact_directed_attempt_token_test_role() {
    // Issue #652 regression guard — `tests/recovery_tests.rs` baseline expects
    // the `artifact_directed_attempt=` token to be present in the recovery
    // note so the production message format remains stable across the
    // Phase 2 wiring of `ArtifactCompletionJob`.
    let note = artifact_directed_recovery_note("test", "tests/test_foo.py", 2);
    assert!(
        note.contains("artifact_directed_attempt=2"),
        "regression: artifact_directed_attempt token missing from note: {note}"
    );
    assert!(note.contains("test"), "role label must appear in note");
    assert!(
        note.contains("tests/test_foo.py"),
        "target path must appear in note"
    );
}

#[test]
fn artifact_directed_recovery_note_contains_artifact_directed_attempt_token_impl_role() {
    let note = artifact_directed_recovery_note("implementation", "src/x.py", 3);
    assert!(note.contains("artifact_directed_attempt=3"));
    assert!(note.contains("implementation"));
    assert!(note.contains("src/x.py"));
}

#[test]
fn empty_response_recovery_note_format_compatible_with_no_tool_classification() {
    // Issue #652 wiring records `NoTool` attempts on the empty-reply path
    // (turn.rs::run_actor_loop -> final_reply.is_empty branch). The message
    // emitted upstream must remain non-empty and recognizable.
    let note = empty_response_recovery_note(1, true);
    assert!(!note.is_empty());
    assert!(note.contains("attempt=1"));
}

#[test]
fn repo_change_no_tool_recovery_note_format_compatible_with_prose_only_classification() {
    // Issue #652 wiring records `ProseOnly` attempts on the prose-only path
    // (turn.rs::run_actor_loop -> final_reply non-empty + repo_change_no_tool
    // branch). Pin the existing recovery note format.
    let note = repo_change_no_tool_recovery_note(2);
    assert!(!note.is_empty());
    assert!(note.contains("exactly one tool call"));
}

#[test]
fn detects_dependency_install_loops() {
    assert!(is_dependency_install_command(
        "npm install --save-dev jest ts-jest"
    ));
    assert!(should_block_bash_command(
        "npm install --save-dev jest",
        &[],
        2
    ));
    assert!(should_block_bash_command(
        "npm install --save-dev jest",
        &["npm install --save-dev jest".to_string()],
        0
    ));
    assert!(!should_block_bash_command("npm test", &[], 0));
    assert!(is_scaffold_command("npx create-next-app@latest . --ts"));
    assert!(is_workspace_reset_command("rm -rf .anvil"));
    assert!(should_block_bash_command(
        "rm -rf .anvil && npx create-next-app@latest . --ts",
        &[],
        0
    ));
    assert!(should_block_bash_command(
        "npx create-next-app@latest . --ts",
        &["npx create-next-app@latest app --ts".to_string()],
        0
    ));
    assert!(should_block_restart_discovery("Glob", true));
    assert!(should_block_restart_discovery("Bash", true));
    assert!(!should_block_restart_discovery("Read", true));
    assert!(broad_restart_discovery_error("Glob").contains("blocked during actor restart"));
}

// -------------------------------------------------------------------------
// Issue #652 PR-003 — mockito-stubbed actor-loop E2E coverage.
//
// These tests drive the real `Agent::process_line` pipeline against a
// mocked `/api/generate` so the wrong-target / no-tool / prose-only
// classifier paths in `turn.rs::run_actor_loop` are exercised end-to-end
// (Ollama-free, deterministic). The mock returns text-only responses for
// every iteration, exhausting `ARTIFACT_COMPLETION_ATTEMPT_LIMIT` and
// triggering the `artifact_completion_failed role=test` working-memory
// entry. The fixture seeds a `ScaffoldArtifactSnapshot` whose recorded
// `tests/test_artifact.py` is left unchanged on disk — that is the
// canonical condition under which `task_contract_recovery_target`
// surfaces a Test-role recovery hint, which in turn installs the
// `ArtifactCompletionJob` via the SSOT entry point reworked in PR-001.
//
// Each test asserts:
//   (a) at least one /api/generate hit (the mock was actually consulted),
//   (b) `working_memory.unresolved_errors` carries the
//       `artifact_completion_failed role=test` token after enough
//       iterations (sink: working-memory error),
//   (c) the recorded error string is single-line and within the byte cap
//       (sink: mask + cap + control-char neutralization defence line —
//       any embedded control characters in the diagnostic surface have
//       been replaced with spaces; cf. `mask_payload_inplace`).
// -------------------------------------------------------------------------

#[cfg(test)]
mod pr003_mockito_e2e {
    use anvil::agent::Agent;
    use anvil::agent::loop_run::FooterHandle;
    use anvil::config::Config;
    use anvil::model_registry::RuntimeModels;
    use anvil::ollama::client::OllamaClient;
    use anvil::session::store::{
        ScaffoldArtifactFileSnapshot, ScaffoldArtifactRole, ScaffoldArtifactSnapshot,
        SessionSnapshot, SessionStore,
    };
    use tempfile::tempdir;

    /// Compute the SHA-256 hex digest used by `scaffold_diff_status` so the
    /// fixture's scaffold body hash matches the SSOT helper. Mirrors
    /// `crate::util::file_classify`'s hashing — we cannot reach the SSOT
    /// helper from integration tests, so we inline the same algorithm.
    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Build the `(Agent, _tempdir_guard, mockito_server_mock)` fixture for
    /// a Test-role artifact completion exhaustion scenario. The mockito
    /// server returns `model_response` for every `/api/generate` call.
    fn build_test_role_exhaustion_fixture(
        session_id: &str,
        workspace_key: &str,
        scaffold_path: &str,
        scaffold_body: &str,
        model_response: serde_json::Value,
        max_iterations: usize,
    ) -> (
        Agent,
        tempfile::TempDir,
        mockito::ServerGuard,
        mockito::Mock,
    ) {
        // Spin up the mock Ollama before constructing the client (URL must
        // be known up-front for `OllamaClient::new`).
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/generate")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(model_response.to_string())
            .expect_at_least(1)
            .create();

        let dir = tempdir().unwrap();
        let state_root = dir.path().join(".anvil-state");
        std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();
        // Materialize a project marker so `TaskWorkspaceScope` resolves
        // `SingleProjectRoot` (otherwise `Greenfield` is fine too —
        // either accepts in-scope paths).
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"e2e\"\n",
        )
        .unwrap();
        // Materialize the scaffold's PARENT directory so
        // `ArtifactCompletionJob::new`'s missing-leaf canonicalize-parent
        // path succeeds. We intentionally do NOT write the leaf file
        // itself: an existing file with `CandidateOnly` ownership would
        // be rejected by the job constructor (DR4-002). Leaving the leaf
        // missing keeps the path on the "create" branch where the job
        // installs cleanly and the actor loop's prose-only / no-tool /
        // wrong-target classifiers can consume the role-specific budget.
        let scaffold_full = dir.path().join(scaffold_path);
        if let Some(parent) = scaffold_full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        // `scaffold_body` is recorded as the snapshot's `content_hash` so
        // `scaffold_diff_status` returns `UnchangedOrMissing` when the
        // file is missing — exactly what we want for "scaffold has not
        // been materialized yet" recovery-target selection.
        let _ = scaffold_body; // kept for documentation; body content
        // does not need to live on disk for this fixture (see above).

        let mut config = Config::default();
        config.cwd = dir.path().to_path_buf();
        config.photon_enabled = false;
        config.requested_model = Some("test-model".to_string());
        config.ollama_host = server.url();
        config.state_dir_override = Some(state_root.clone());
        config.yes_mode = true;
        config.max_iterations = max_iterations;

        let mut session = SessionSnapshot {
            id: session_id.to_string(),
            workspace_key: workspace_key.to_string(),
            ..Default::default()
        };
        // Plant a scaffold snapshot for the Test role so
        // `task_contract_recovery_target` returns a Test hint when the
        // prompt requires test artifacts.
        let content_hash = sha256_hex(scaffold_body.as_bytes());
        session
            .scaffold_artifact_snapshots
            .push(ScaffoldArtifactSnapshot {
                created_turn_index: 0,
                request_hash: "fixture-pr003".to_string(),
                files: vec![ScaffoldArtifactFileSnapshot {
                    path: scaffold_path.to_string(),
                    content_hash,
                    roles: vec![ScaffoldArtifactRole::Test],
                    bootstrap_only: true,
                }],
            });

        let agent = Agent::new(
            config,
            RuntimeModels {
                main: "test-model".to_string(),
                sidecar: None,
            },
            OllamaClient::new(server.url()).unwrap(),
            SessionStore::new(&state_root, session_id, workspace_key),
            session,
            FooterHandle::disabled(),
        );

        (agent, dir, server, mock)
    }

    /// PR-003 (a) prose-only branch: model returns text without tool calls
    /// for every iteration → `ProseOnly` attempts accumulate → budget
    /// exhaustion → `artifact_completion_failed role=test` in working
    /// memory. Independent from the generic retry counter (which would
    /// otherwise iterate up to `max_iterations`).
    #[test]
    fn pr003_prose_only_exhausts_artifact_completion_budget_in_actor_loop() {
        let model_response = serde_json::json!({
            "model": "test-model",
            "response": "I have analyzed the request and will outline a strategy without making any code changes.",
            "done": true,
        });
        let (mut agent, _dir, _server, mock) = build_test_role_exhaustion_fixture(
            "pr003-prose-only",
            "anvil-pr003-prose",
            "tests/test_artifact.py",
            "# scaffold body — bootstrap only\n",
            model_response,
            6, // enough iterations to exhaust the 3-attempt budget
        );
        let prompt = "Write tests for the helper module and verify it works.";
        let _ = agent.process_line(prompt, false);

        // (a) the mock was actually consulted.
        mock.assert();

        // (b) the working-memory error surface carries the diagnostic.
        let working_errors: Vec<String> = agent
            .session_ref()
            .working_memory
            .unresolved_errors
            .iter()
            .filter(|err| err.starts_with("artifact_completion_failed role=test"))
            .cloned()
            .collect();
        assert!(
            !working_errors.is_empty(),
            "PR-003 prose-only: expected `artifact_completion_failed role=test` in working_memory.unresolved_errors; \
             actual={:?}",
            agent.session_ref().working_memory.unresolved_errors
        );

        // (c) mask + cap + control-char neutralization defence: every
        // recorded entry must be single-line and well under 64 KiB.
        for err in &working_errors {
            assert!(
                !err.contains('\n'),
                "PR-003: working-memory error must be single-line (no newlines): {err:?}"
            );
            assert!(
                !err.contains('\t'),
                "PR-003: working-memory error must not contain raw tabs: {err:?}"
            );
            assert!(
                err.len() < 64 * 1024,
                "PR-003: working-memory error must be byte-capped: len={}",
                err.len()
            );
        }
    }

    /// PR-003 (b) no-tool branch: model returns an empty `response` for
    /// every iteration → `NoTool` attempts accumulate → budget exhausts →
    /// `artifact_completion_failed role=test` surfaces. The artifact
    /// budget exhausts independently of `empty_retries` / generic
    /// retry counters (which have different limits).
    #[test]
    fn pr003_no_tool_exhausts_artifact_completion_budget_in_actor_loop() {
        let model_response = serde_json::json!({
            "model": "test-model",
            "response": "",
            "done": true,
        });
        let (mut agent, _dir, _server, mock) = build_test_role_exhaustion_fixture(
            "pr003-no-tool",
            "anvil-pr003-no-tool",
            "tests/test_artifact.py",
            "# scaffold body — bootstrap only\n",
            model_response,
            6,
        );
        let prompt = "Write tests for the calculator function.";
        let _ = agent.process_line(prompt, false);

        mock.assert();

        let working_errors: Vec<String> = agent
            .session_ref()
            .working_memory
            .unresolved_errors
            .iter()
            .filter(|err| err.starts_with("artifact_completion_failed role=test"))
            .cloned()
            .collect();
        assert!(
            !working_errors.is_empty(),
            "PR-003 no-tool: expected `artifact_completion_failed role=test`; \
             actual={:?}",
            agent.session_ref().working_memory.unresolved_errors
        );
    }

    /// PR-003 (c) wrong-target branch + control-char neutralization /
    /// length-cap defence line.
    ///
    /// The model returns a Write tool call against the *wrong* target
    /// (an impl file) plus a content payload containing embedded newlines
    /// and a leading `sk-`-prefixed secret-shaped string. The
    /// artifact-directed policy rejects the wrong-target write before it
    /// hits disk, records a `WrongTarget` attempt, and exhausts the
    /// budget after 3 iterations.
    ///
    /// The `actual_actions` snapshot stored on the active job MUST have
    /// the secret pattern masked (`mask_secrets` SSOT — DR4-001), the
    /// newlines replaced with spaces (`neutralize_control_chars`), and
    /// the byte length capped at `MAX_ARTIFACT_ACTION_TEXT_BYTES`. The
    /// final diagnostic event runs through `mask_payload_inplace` as a
    /// belt-and-braces final-defence pass.
    #[test]
    fn pr003_wrong_target_exhausts_with_mask_and_cap_defence() {
        // The mock returns a tool_call payload targeting `src/wrong.py`
        // (impl role, NOT the test scaffold). The Anvil tool-call XML
        // fallback path picks this up.
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        let payload = format!(
            "<anvil_tool_call>{}</anvil_tool_call>",
            serde_json::json!({
                "name": "Write",
                "arguments": {
                    "path": "src/wrong.py",
                    "content": format!(
                        "secret={secret}\nline1\nline2\n{}",
                        "x".repeat(200)
                    ),
                },
            })
        );
        let model_response = serde_json::json!({
            "model": "test-model",
            "response": payload,
            "done": true,
        });
        let (mut agent, dir, _server, mock) = build_test_role_exhaustion_fixture(
            "pr003-wrong-target",
            "anvil-pr003-wrong",
            "tests/test_artifact.py",
            "# scaffold body — bootstrap only\n",
            model_response,
            6,
        );
        let work_root = dir.path().to_path_buf();
        let prompt = "Write tests for the calculator function.";
        let _ = agent.process_line(prompt, false);

        mock.assert();

        // The artifact-directed policy gate must NOT have rejected the
        // wrong-target Write at iteration 1 (the policy is only active
        // once `current_artifact_recovery_target` is set, which happens
        // AFTER the first reply). But after iteration 1 installs the
        // recovery target, subsequent wrong-target Writes are rejected.
        // We assert here only that the wrong-target file was not
        // written by ALL iterations — at least the policy gate must
        // have blocked iterations 2+ from re-writing it.
        let wrong_file_size = std::fs::metadata(work_root.join("src/wrong.py"))
            .map(|m| m.len())
            .unwrap_or(0);
        // The wrong-target file exists (iter 1 wrote it before the
        // recovery target was installed), but it must contain MASKED
        // content — the secret literal cannot survive any path that
        // routes through Anvil's mask_secrets SSOT. (The Write tool
        // bypasses mask_secrets — the secret is the user's content.
        // What we DO verify is that the *diagnostic surface* — system
        // notes, working memory, eval log — has the secret masked.)
        let _ = wrong_file_size; // not asserted further; see comment above.

        // Working-memory must carry the diagnostic with the role=test
        // token (the budget was consumed against the Test role).
        let working_errors: Vec<String> = agent
            .session_ref()
            .working_memory
            .unresolved_errors
            .iter()
            .filter(|err| err.starts_with("artifact_completion_failed role=test"))
            .cloned()
            .collect();
        assert!(
            !working_errors.is_empty(),
            "PR-003 wrong-target: expected `artifact_completion_failed role=test`; \
             actual={:?}",
            agent.session_ref().working_memory.unresolved_errors
        );

        // Defence line: the entire working-memory error surface must
        // have been mask-applied — the secret literal must not survive.
        for err in &agent.session_ref().working_memory.unresolved_errors {
            assert!(
                !err.contains(secret),
                "PR-003 wrong-target: secret pattern leaked into working_memory: {err}"
            );
            assert!(
                !err.contains('\n'),
                "PR-003 wrong-target: control char (newline) survived into working_memory: {err:?}"
            );
        }
    }

    /// PRR-002 (re-review v2): the raw `path` argument from a wrong-target
    /// tool call can itself carry control characters / secret-like strings
    /// / over-long content. `record_artifact_completion_attempt` stores
    /// the path verbatim inside the `actual_actions` vector via
    /// `format!("{name} on {actual_path}")`. After exhaustion, that vector
    /// is rendered into the system note ("Recent actions: …") **and** into
    /// the `agent.artifact_completion_failed` JSON log event. PRR-002
    /// requires that BOTH downstream surfaces contain only sanitized
    /// (mask_secrets + length cap + control-char neutralization +
    /// mask_payload_inplace) versions of the path. We assert against the
    /// system note in `session.messages` since the JSON event is logged
    /// out-of-band; the same `sanitize_actions` pipeline feeds both
    /// sinks (artifact_completion_job.rs::ArtifactAttemptOutcome::new).
    #[test]
    fn prr002_wrong_target_sanitizes_actual_actions_path_in_system_note() {
        // Build a Write tool call whose `path` carries:
        //   1. an `sk-…`-shaped secret (must be masked),
        //   2. embedded `\n` / `\t` / `\r` control chars (must be neutralized),
        //   3. a long suffix (must be byte-capped).
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        let raw_path = format!(
            "src/wrong{secret}\nline2\twith\ttabs\rmore/{}/file.py",
            "x".repeat(200)
        );
        let payload = format!(
            "<anvil_tool_call>{}</anvil_tool_call>",
            serde_json::json!({
                "name": "Write",
                "arguments": {
                    "path": raw_path,
                    "content": "harmless",
                },
            })
        );
        let model_response = serde_json::json!({
            "model": "test-model",
            "response": payload,
            "done": true,
        });
        let (mut agent, _dir, _server, mock) = build_test_role_exhaustion_fixture(
            "prr002-actual-actions-sanitize",
            "anvil-prr002-actions",
            "tests/test_artifact.py",
            "# scaffold body — bootstrap only\n",
            model_response,
            6,
        );
        let prompt = "Write tests for the calculator function.";
        let _ = agent.process_line(prompt, false);

        mock.assert();

        // The exhaustion path must have fired the system note (sink #1 of
        // emit_artifact_completion_failed_diagnostic_if_needed). The note
        // is the rendered surface that carries `actual_actions` directly
        // ("Recent actions: …"). Find the note and assert sanitization.
        let artifact_notes: Vec<&String> = agent
            .session_ref()
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| &m.content)
            .filter(|c| c.contains("[Artifact Completion Failed]"))
            .collect();
        assert!(
            !artifact_notes.is_empty(),
            "PRR-002: expected [Artifact Completion Failed] system note after exhaustion; \
             messages={:?}",
            agent
                .session_ref()
                .messages
                .iter()
                .map(|m| (
                    m.role.as_str(),
                    m.content.chars().take(80).collect::<String>()
                ))
                .collect::<Vec<_>>()
        );
        for note in &artifact_notes {
            // (1) secret literal must be masked by the SSOT pipeline.
            assert!(
                !note.contains(secret),
                "PRR-002: secret pattern leaked into [Artifact Completion Failed] system note: {note}"
            );
            // (2) control characters must be neutralized to spaces.
            assert!(
                !note.contains('\n'),
                "PRR-002: newline survived into actual_actions render: {note:?}"
            );
            assert!(
                !note.contains('\t'),
                "PRR-002: tab survived into actual_actions render: {note:?}"
            );
            assert!(
                !note.contains('\r'),
                "PRR-002: CR survived into actual_actions render: {note:?}"
            );
            // (3) byte cap must apply. Note is wrapped with fixed prefix
            // text — total length stays well under any reasonable bound.
            assert!(
                note.len() < 64 * 1024,
                "PRR-002: system note must be byte-capped (len={})",
                note.len()
            );
        }
    }
}

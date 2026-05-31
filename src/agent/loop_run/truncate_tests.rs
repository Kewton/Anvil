//! turn.rs `mod truncate_tests` extracted to a sibling file (parent #680).
//!
//! Hosts the `#[cfg(test)] mod truncate_tests` originally embedded near
//! the bottom of `turn.rs` (~620 LOC). Same pattern as
//! `turn_tests` / `progress_tests`: mirrored use prelude + rewritten
//! inner-mod use block + sibling-module paths preserved.
//!
//! #[cfg(test)] only; production binary excludes this file. No facade
//! re-export (DR3-001).

// Additional explicit imports needed by test bodies via qualified `super::X`.
use super::verifier_orchestration::{
    synthesized_missing_implementation_target_path_for_request,
    synthesized_missing_test_target_path_for_request, task_contract_no_verifier_note,
    test_target_path_compatible_with_request,
};
// --- Mirrored use prelude from turn.rs ---------------------------------------

// --- Extracted mod truncate_tests body --------------------------------------
#[cfg(test)]
mod inner {
    use super::super::actor_loop_flow::{
        reply_looks_like_future_work, should_apply_repo_change_partial_progress_recovery,
        task_contract_continue_requires_tool_recovery,
    };
    use super::super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
    use super::super::progress_text::truncate;
    use super::super::scaffold_pipeline::task_requires_nextjs_scaffold;
    use super::super::scaffold_pipeline::{
        ScaffoldFramework, deterministic_nextjs_scaffold_reply, requested_scaffold_framework,
        scaffold_candidate_for_missing_role_from_snapshots, scaffold_command_matches_framework,
        scaffold_file_snapshot, task_or_plan_requires_nextjs_scaffold,
    };
    use super::super::task_contract::{
        ArtifactRecoveryAction, ArtifactRole, TaskContract,
        repo_edit_satisfies_artifact_recovery_target,
    };
    use super::super::tool_policy::focused_edit_tool_policy_error;
    use super::super::turn_helpers::extract_filename_with_suffix;
    use super::super::verifier_orchestration::{
        task_contract_needs_verification, task_contract_verifier_repair_note,
    };
    use super::super::verifier_repair_targeting::changed_files_for_verifier;
    use crate::agent::orchestration::RepoVerification;
    use crate::agent::recovery::ActionExpectation;
    use crate::model_capabilities::model_capabilities;
    use crate::modes::plan_act::ExecutionMode;
    use crate::session::store::ScaffoldArtifactSnapshot;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn preserves_short_strings_verbatim() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncates_long_strings_with_ellipsis() {
        assert_eq!(truncate("abcdefgh", 3), "abc...");
    }

    #[test]
    fn never_splits_multibyte_code_points() {
        // Each Japanese char is 3 bytes in UTF-8; taking 2 must not slice mid-char.
        assert_eq!(truncate("あいうえお", 2), "あい...");
    }

    #[test]
    fn detects_future_work_prose_after_partial_edit() {
        assert!(reply_looks_like_future_work(
            "Now I'll create the full interactive app as a client component."
        ));
        assert!(reply_looks_like_future_work("次にゲーム本体を実装します。"));
        assert!(reply_looks_like_future_work(
            "READMEの全文を確認しました。さらに詳細な設計ファイルがないか探してみます。"
        ));
        assert!(!reply_looks_like_future_work(
            "Implemented the first playable shell in app/page.tsx."
        ));
    }

    #[test]
    fn task_contract_verify_preempts_future_work_repo_recovery() {
        let contract = TaskContract::from_request(
            "APIを実装してください。使用方法をREADME.mdに記述してください。テストコードも実装してください。",
        );
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Impl,
            count: 1,
        });
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Test,
            count: 1,
        });
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Docs,
            count: 1,
        });

        let verify_pending =
            task_contract_needs_verification(ExecutionMode::Act, Some(&contract), &evidence);
        assert!(verify_pending);
        let action = ArtifactRecoveryAction::RunVerifier;
        assert!(reply_looks_like_future_work(
            "次にテストを実行して確認します。"
        ));
        assert!(!should_apply_repo_change_partial_progress_recovery(
            ActionExpectation::RepoChange,
            3,
            "次にテストを実行して確認します。",
            Some(&action),
        ));
    }

    #[test]
    fn task_contract_continue_preempts_future_work_repo_recovery() {
        let action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::UsageDocs],
            target_hint: None,
        };

        assert!(reply_looks_like_future_work("次にREADMEを更新します。"));
        assert!(!should_apply_repo_change_partial_progress_recovery(
            ActionExpectation::RepoChange,
            2,
            "次にREADMEを更新します。",
            Some(&action),
        ));
    }

    #[test]
    fn task_contract_continue_no_tool_requires_targeted_recovery() {
        let action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::UsageDocs],
            target_hint: None,
        };

        assert!(task_contract_continue_requires_tool_recovery(
            Some(&action),
            0
        ));
        assert!(!task_contract_continue_requires_tool_recovery(
            Some(&action),
            1
        ));
        assert!(!task_contract_continue_requires_tool_recovery(
            Some(&ArtifactRecoveryAction::Done),
            0
        ));
    }

    #[test]
    fn generic_partial_progress_recovery_runs_when_contract_is_done() {
        let action = ArtifactRecoveryAction::Done;

        assert!(should_apply_repo_change_partial_progress_recovery(
            ActionExpectation::RepoChange,
            1,
            "Next, I will update the remaining file.",
            Some(&action),
        ));
    }

    #[test]
    fn verifier_repair_note_carries_diagnostic_without_completing() {
        let note = task_contract_verifier_repair_note(
            "python3 -m pytest",
            "FAILED tests/test_main.py::test_create\nsecret=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            1,
            3,
            None,
        );

        assert!(
            note.contains("command_json=\"python3 -m pytest\""),
            "got: {note}"
        );
        assert!(!note.contains("output_excerpt_json="), "got: {note}");
        assert!(
            note.contains("controller-owned diagnostic data"),
            "got: {note}"
        );
        assert!(note.contains("repair the implementation"), "got: {note}");
        assert!(note.contains("Do not finish with prose"), "got: {note}");
        assert!(!note.contains("ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    }

    #[test]
    fn verifier_changed_files_include_accumulated_and_current_sets() {
        let accumulated = vec![RepoVerification {
            changed_files: vec!["src/lib.rs".to_string()],
            all_changed_files: vec!["src/lib.rs".to_string(), "README.md".to_string()],
            implementation_files_changed: 1,
            test_files_changed: 0,
            setup_files_changed: 0,
            other_files_changed: 1,
            deleted_files_changed: 0,
        }];
        let current = RepoVerification {
            changed_files: vec!["tests/lib_test.rs".to_string()],
            all_changed_files: vec!["tests/lib_test.rs".to_string(), "README.md".to_string()],
            implementation_files_changed: 0,
            test_files_changed: 1,
            setup_files_changed: 0,
            other_files_changed: 1,
            deleted_files_changed: 0,
        };

        assert_eq!(
            changed_files_for_verifier(&accumulated, &current),
            vec![
                "README.md".to_string(),
                "src/lib.rs".to_string(),
                "tests/lib_test.rs".to_string(),
            ]
        );
    }

    #[test]
    fn verifier_changed_files_exclude_controller_owned_state() {
        let accumulated = vec![RepoVerification {
            changed_files: vec![".anvil-state/verifier-python/site/_pytest/__init__.py".into()],
            all_changed_files: vec![
                ".anvil-state/verifier-python/site/_pytest/__init__.py".into(),
                "app/main.py".into(),
            ],
            implementation_files_changed: 1,
            test_files_changed: 0,
            setup_files_changed: 0,
            other_files_changed: 1,
            deleted_files_changed: 0,
        }];
        let current = RepoVerification {
            changed_files: vec!["README.md".into()],
            all_changed_files: vec![
                ".anvil-state/verifier-python/site/_pytest/cache.py (deleted)".into(),
                "README.md".into(),
            ],
            implementation_files_changed: 0,
            test_files_changed: 0,
            setup_files_changed: 0,
            other_files_changed: 1,
            deleted_files_changed: 1,
        };

        assert_eq!(
            changed_files_for_verifier(&accumulated, &current),
            vec!["README.md".to_string(), "app/main.py".to_string()]
        );
    }

    #[test]
    fn artifact_recovery_target_filters_unrelated_repo_edits() {
        let target = super::super::task_contract::RecoveryTarget {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/main.py".to_string(),
            reason: "test target".to_string(),
            attempt: 1,
        };

        assert!(repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Impl,
            "app/main.py",
            Some(&target),
        ));
        assert!(!repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Impl,
            "app/__init__.py",
            Some(&target),
        ));
        assert!(!repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Docs,
            "README.md",
            Some(&target),
        ));
        assert!(repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Docs,
            "README.md",
            None,
        ));
    }

    #[test]
    fn artifact_recovery_target_accepts_same_family_test_artifacts() {
        let target = super::super::task_contract::RecoveryTarget {
            role: super::super::task_contract::ArtifactRole::Test,
            path: "tests/main.rs".to_string(),
            reason: "synthesized test artifact aligned with requested rust project family"
                .to_string(),
            attempt: 1,
        };

        assert!(repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Test,
            "tests/lib.rs",
            Some(&target),
        ));
        assert!(!repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Impl,
            "src/lib.rs",
            Some(&target),
        ));
        assert!(!repo_edit_satisfies_artifact_recovery_target(
            RepoEditCategory::Test,
            "tests/test_main.py",
            Some(&target),
        ));
    }

    #[test]
    fn synthesized_missing_impl_target_for_fastapi_uses_root_writable_path() {
        assert_eq!(
            super::synthesized_missing_implementation_target_path_for_request(
                super::super::task_contract::ArtifactRole::Implementation,
                "FastAPIでcrudのAPIを開発してください。"
            )
            .as_deref(),
            Some("main.py")
        );
    }

    #[test]
    fn synthesized_missing_impl_target_for_rust_library_uses_src_lib() {
        assert_eq!(
            super::synthesized_missing_implementation_target_path_for_request(
                super::super::task_contract::ArtifactRole::Implementation,
                "文字列スラッグ生成用のRustライブラリを開発してください。"
            )
            .as_deref(),
            Some("src/lib.rs")
        );
    }

    #[test]
    fn synthesized_missing_impl_target_does_not_apply_to_non_impl_roles() {
        assert!(
            super::synthesized_missing_implementation_target_path_for_request(
                super::super::task_contract::ArtifactRole::Setup,
                "FastAPIでcrudのAPIを開発してください。"
            )
            .is_none()
        );
    }

    #[test]
    fn synthesized_missing_test_target_for_rust_uses_rust_test_path() {
        assert_eq!(
            super::synthesized_missing_test_target_path_for_request(
                "Rustライブラリを作成し、cargo testで動くテストコードも実装してください。"
            ),
            Some(("tests/main.rs", "rust"))
        );
    }

    #[test]
    fn synthesized_missing_test_target_for_typescript_uses_ts_test_path() {
        assert_eq!(
            super::synthesized_missing_test_target_path_for_request(
                "TypeScript で実装し、.ts のテストも追加してください。"
            ),
            Some(("tests/main.test.ts", "typescript"))
        );
    }

    #[test]
    fn synthesized_missing_test_target_for_python_uses_pytest_path() {
        assert_eq!(
            super::synthesized_missing_test_target_path_for_request(
                "FastAPIでAPIを作成し、pytestで確認してください。"
            ),
            Some(("tests/test_main.py", "python"))
        );
    }

    #[test]
    fn case_record_success_accepts_clean_auto_test_turn() {
        let score = crate::session::anvil_score::AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(true),
            unsafe_actions_blocked: 0,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
            ..Default::default()
        };

        assert!(
            super::super::case_record_extract::case_record_extraction_succeeded(&score, false, 1)
        );
    }

    #[test]
    fn case_record_success_rejects_blocked_auto_test_turn() {
        let score = crate::session::anvil_score::AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(true),
            unsafe_actions_blocked: 1,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
            ..Default::default()
        };

        assert!(
            !super::super::case_record_extract::case_record_extraction_succeeded(&score, true, 0)
        );
    }

    #[test]
    fn case_record_success_requires_clean_repo_edit_without_auto_test() {
        let score = crate::session::anvil_score::AnvilScore {
            consecutive_no_progress_turns: 0,
            ..Default::default()
        };

        assert!(
            super::super::case_record_extract::case_record_extraction_succeeded(&score, true, 0)
        );
        assert!(
            !super::super::case_record_extract::case_record_extraction_succeeded(&score, false, 0)
        );
        assert!(
            !super::super::case_record_extract::case_record_extraction_succeeded(&score, true, 1)
        );
    }

    #[test]
    fn python_test_target_is_not_compatible_with_rust_request() {
        assert!(!super::test_target_path_compatible_with_request(
            "tests/test_main.py",
            "Rustでライブラリを実装し、cargo testで確認してください"
        ));
        assert!(super::test_target_path_compatible_with_request(
            "tests/main.rs",
            "Rustでライブラリを実装し、cargo testで確認してください"
        ));
    }

    #[test]
    fn missing_verifier_setup_hint_points_rust_requests_to_cargo_manifest() {
        assert_eq!(
            super::super::verifier_orchestration::missing_verifier_setup_hint_for_request(
                "Rustライブラリを作成し、cargo testで動くテストも実装してください。"
            ),
            Some("For Rust/Cargo workspaces, create or update Cargo.toml.")
        );
    }

    #[test]
    fn missing_verifier_note_for_rust_requires_single_cargo_metadata_edit() {
        let note = super::task_contract_no_verifier_note(
            1,
            3,
            "Rustライブラリを作成し、cargo testで動くテストも実装してください。",
        );
        assert!(note.contains("Emit exactly one Write or Edit now"));
        assert!(note.contains("create or update Cargo.toml"));
        assert!(note.contains("Do not call Bash"));
    }

    #[test]
    fn missing_verifier_setup_hint_points_python_requests_to_python_metadata() {
        assert_eq!(
            super::super::verifier_orchestration::missing_verifier_setup_hint_for_request(
                "FastAPIでCRUD APIを作り、pytestでテストしてください。"
            ),
            Some(
                "For Python/pytest workspaces, create or update pyproject.toml, requirements.txt, or pytest configuration."
            )
        );
    }

    #[test]
    fn missing_verifier_setup_hint_points_node_requests_to_package_json() {
        assert_eq!(
            super::super::verifier_orchestration::missing_verifier_setup_hint_for_request(
                "TypeScript CLI を作成し、npm test で確認してください。"
            ),
            Some("For Node/npm workspaces, create or update package.json with a test script.")
        );
    }

    #[test]
    fn focused_edit_policy_allows_write_for_missing_recovery_target_only() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("README.md");

        assert!(
            focused_edit_tool_policy_error(
                "Write",
                &json!({"path": "README.md", "content": "# Usage"}),
                &target,
                dir.path(),
                false,
            )
            .is_none()
        );

        let err = focused_edit_tool_policy_error(
            "Read",
            &json!({"path": "README.md"}),
            &target,
            dir.path(),
            false,
        )
        .expect("Read should be blocked for missing target");
        assert!(err.contains("only allows Write"), "got: {err}");
    }

    #[test]
    fn scaffold_candidate_prefers_substantive_impl_over_empty_support_file() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::write(dir.path().join("app/__init__.py"), "").unwrap();
        let main_content = b"from fastapi import FastAPI\napp = FastAPI()\n";
        std::fs::write(dir.path().join("app/main.py"), main_content).unwrap();
        let snapshot = ScaffoldArtifactSnapshot {
            created_turn_index: 1,
            request_hash: "test".to_string(),
            files: vec![
                scaffold_file_snapshot("app/__init__.py", b""),
                scaffold_file_snapshot("app/main.py", main_content),
            ],
        };

        assert_eq!(
            scaffold_candidate_for_missing_role_from_snapshots(
                &[snapshot],
                dir.path(),
                ArtifactRole::Implementation,
            ),
            Some("app/main.py".to_string())
        );
    }

    #[test]
    fn extracts_safe_project_instruction_filenames() {
        assert_eq!(
            extract_filename_with_suffix("main script `project_csv_tool.py`", ".py"),
            Some("project_csv_tool.py".to_string())
        );
        assert_eq!(
            extract_filename_with_suffix(
                "メインスクリプト名は user_requested_name.py にして下さい",
                ".py"
            ),
            Some("user_requested_name.py".to_string())
        );
        assert_eq!(
            extract_filename_with_suffix("use ../unsafe.py", ".py"),
            None
        );
    }

    #[test]
    fn detects_nextjs_framework_tasks() {
        assert!(task_requires_nextjs_scaffold(
            "3011ポートで起動可能なnext.jsアプリとして開発してください"
        ));
        assert!(task_requires_nextjs_scaffold("Build this as a NextJS app"));
        assert!(!task_requires_nextjs_scaffold("Build a Rust CLI tool"));
    }

    #[test]
    fn detects_explicit_scaffold_frameworks() {
        assert_eq!(
            requested_scaffold_framework("React.jsアプリとして開発してください"),
            Some(ScaffoldFramework::React)
        );
        assert_eq!(
            requested_scaffold_framework("Nuxt.jsアプリとして開発してください"),
            Some(ScaffoldFramework::Nuxt)
        );
        assert_eq!(
            requested_scaffold_framework("Next.jsアプリとして開発してください"),
            Some(ScaffoldFramework::Next)
        );
        assert_eq!(
            requested_scaffold_framework(
                "Node.jsでToDo管理CLIを開発してください。README.mdとテストコードも作成してください。"
            ),
            None
        );
        assert_eq!(requested_scaffold_framework("Rust CLIを作って"), None);
    }

    #[test]
    fn local_qwen_models_use_read_after_small_edit_protocol() {
        assert!(model_capabilities("qwen3.5:122b").read_after_small_edit_protocol);
        assert!(model_capabilities("qwen3.6:27b-coding-nvfp4").read_after_small_edit_protocol);
        assert!(!model_capabilities("llama3.1:8b").read_after_small_edit_protocol);
    }

    #[test]
    fn scaffold_commands_must_match_requested_framework() {
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npm create vite@latest . -- --template react-ts"
        ));
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::Nuxt,
            "npx nuxi@latest init . --packageManager npm"
        ));
        assert!(scaffold_command_matches_framework(
            ScaffoldFramework::Next,
            "npx create-next-app@latest . --typescript --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npx create-next-app@latest . --typescript --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::React,
            "npx create-react-app . --template cra-template --yes"
        ));
        assert!(!scaffold_command_matches_framework(
            ScaffoldFramework::Nuxt,
            "npm create vite@latest . -- --template react-ts"
        ));
    }

    #[test]
    fn detects_nextjs_request_from_active_task_or_plan_only() {
        assert!(task_or_plan_requires_nextjs_scaffold(
            Some("3011ポートで起動可能なnext.jsアプリとして開発してください"),
            None,
        ));
        assert!(task_or_plan_requires_nextjs_scaffold(
            Some("yes"),
            Some("Build the accepted plan as a Next.js app."),
        ));
        assert!(!task_or_plan_requires_nextjs_scaffold(
            Some("yes"),
            Some("Build a local Rust CLI."),
        ));
        assert!(!task_or_plan_requires_nextjs_scaffold(
            Some(
                "Node.jsでToDo管理CLIを開発してください。README.mdとテストコードも作成してください。"
            ),
            None,
        ));
    }

    #[test]
    fn deterministic_nextjs_scaffold_uses_pinned_noninteractive_command() {
        let reply = deterministic_nextjs_scaffold_reply();
        let command = reply.tool_calls[0]
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .expect("command");
        assert!(command.contains("npx --yes create-next-app@16.2.4"));
        assert!(command.contains(" --yes"));
        assert!(!command.contains("@latest"));
    }
}

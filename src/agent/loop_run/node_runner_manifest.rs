//! Issue #977 (parent #974, Issue C): deterministic Node test-runner
//! manifest completion operator.
//!
//! Pure operator for the MissingDeliverable / MissingEvidence recovery
//! lifecycle. When a Node coding task already produced test artifacts but
//! the workspace cannot bind a test runner — `package.json` is missing, or
//! present without a usable `scripts.test` — this operator deterministically
//! completes the manifest so the existing `auto_test::detect_node_scripts`
//! evidence binding picks up `npm test` on the next EvidenceRunner pass.
//!
//! This is a deterministic operator, not LLM free regeneration. It mirrors
//! the replay-fixture shapes named in the issue:
//! - `043`: `package.json` exists but `scripts.test` is missing.
//! - `048` / `058`: tests exist but `package.json` is missing.
//!
//! `pub(super)` limited / no facade re-export (DR3-001). The pure entry
//! point takes the raw manifest text (or its absence) and returns the
//! completion content; all filesystem / `Agent` access lives in the
//! `node_request_helpers` + `scaffold_pipeline` callers.

use serde_json::{Map, Value};

/// The deterministic test command bound into a completed manifest. Matches
/// the empty-workspace Node scaffold (`scaffold_pipeline::node_skeleton_files`)
/// so the two deterministic paths agree.
pub(super) const NODE_TEST_SCRIPT: &str = "node --test";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NodeManifestAction {
    /// `package.json` was absent — a minimal manifest is created.
    CreateManifest,
    /// `package.json` existed but lacked a usable `scripts.test` — the
    /// script is inserted, preserving the remaining declared fields.
    AddTestScript,
}

impl NodeManifestAction {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            NodeManifestAction::CreateManifest => "create_manifest",
            NodeManifestAction::AddTestScript => "add_test_script",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NodeManifestCompletion {
    pub(super) action: NodeManifestAction,
    pub(super) contents: String,
}

/// Minimal `package.json` written when none exists. Mirrors the
/// empty-workspace Node skeleton manifest (library shape — no `bin`), which
/// is enough for `detect_node_scripts` to bind `npm test` -> `node --test`.
fn created_manifest_contents() -> String {
    format!(
        r#"{{
  "name": "app",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "test": "{NODE_TEST_SCRIPT}"
  }}
}}
"#
    )
}

/// Deterministically complete a Node test-runner manifest.
///
/// - `existing == None` (no `package.json`) -> [`NodeManifestAction::CreateManifest`].
/// - `existing == Some(raw)` with no usable `scripts.test` ->
///   [`NodeManifestAction::AddTestScript`], preserving the other declared
///   fields.
/// - Already bindable (`scripts.test` present and non-empty) -> `None`.
/// - Malformed (unparseable JSON, non-object root, or a non-object
///   `scripts` field) -> `None`. Clobbering a malformed user file is never
///   deterministically safe; the LLM repair path owns that case.
pub(super) fn complete_node_test_runner_manifest(
    existing: Option<&str>,
) -> Option<NodeManifestCompletion> {
    let Some(raw) = existing else {
        return Some(NodeManifestCompletion {
            action: NodeManifestAction::CreateManifest,
            contents: created_manifest_contents(),
        });
    };
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let Value::Object(mut object) = value else {
        return None;
    };
    if manifest_has_usable_test_script(&object) {
        return None;
    }
    let scripts = object
        .entry("scripts".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(scripts) = scripts else {
        // A non-object `scripts` field is malformed; do not overwrite it.
        return None;
    };
    scripts.insert(
        "test".to_string(),
        Value::String(NODE_TEST_SCRIPT.to_string()),
    );
    let mut contents = serde_json::to_string_pretty(&Value::Object(object)).ok()?;
    contents.push('\n');
    Some(NodeManifestCompletion {
        action: NodeManifestAction::AddTestScript,
        contents,
    })
}

/// True when the manifest already declares a non-empty `test` script. The
/// key match is case-insensitive so an existing `"Test"` is not duplicated;
/// `detect_node_scripts` normalizes script names the same way.
fn manifest_has_usable_test_script(object: &Map<String, Value>) -> bool {
    let Some(scripts) = object.get("scripts").and_then(Value::as_object) else {
        return false;
    };
    scripts.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("test")
            && value
                .as_str()
                .is_some_and(|script| !script.trim().is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_script(contents: &str) -> Option<String> {
        let json = serde_json::from_str::<Value>(contents).expect("completion is valid json");
        json.get("scripts")?
            .get("test")?
            .as_str()
            .map(str::to_string)
    }

    #[test]
    fn missing_manifest_is_created_with_test_script() {
        let completion = complete_node_test_runner_manifest(None).expect("creates a manifest");
        assert_eq!(completion.action, NodeManifestAction::CreateManifest);
        assert_eq!(
            test_script(&completion.contents).as_deref(),
            Some("node --test")
        );
        // The created manifest must be re-completable into a no-op (it is
        // already bindable).
        assert!(complete_node_test_runner_manifest(Some(&completion.contents)).is_none());
    }

    #[test]
    fn manifest_without_scripts_gets_a_scripts_test() {
        let existing = r#"{"name":"app","version":"1.2.3","type":"module"}"#;
        let completion =
            complete_node_test_runner_manifest(Some(existing)).expect("adds a test script");
        assert_eq!(completion.action, NodeManifestAction::AddTestScript);
        assert_eq!(
            test_script(&completion.contents).as_deref(),
            Some("node --test")
        );
        // Preserves the previously declared fields.
        let json = serde_json::from_str::<Value>(&completion.contents).unwrap();
        assert_eq!(json.get("name").and_then(Value::as_str), Some("app"));
        assert_eq!(json.get("version").and_then(Value::as_str), Some("1.2.3"));
        assert_eq!(json.get("type").and_then(Value::as_str), Some("module"));
    }

    #[test]
    fn manifest_with_other_scripts_keeps_them_and_adds_test() {
        let existing = r#"{"scripts":{"build":"node build.js"}}"#;
        let completion =
            complete_node_test_runner_manifest(Some(existing)).expect("adds a test script");
        assert_eq!(completion.action, NodeManifestAction::AddTestScript);
        let json = serde_json::from_str::<Value>(&completion.contents).unwrap();
        let scripts = json.get("scripts").and_then(Value::as_object).unwrap();
        assert_eq!(
            scripts.get("build").and_then(Value::as_str),
            Some("node build.js")
        );
        assert_eq!(
            scripts.get("test").and_then(Value::as_str),
            Some("node --test")
        );
    }

    #[test]
    fn manifest_with_existing_test_script_is_left_alone() {
        let existing = r#"{"scripts":{"test":"vitest run"}}"#;
        assert!(complete_node_test_runner_manifest(Some(existing)).is_none());
    }

    #[test]
    fn manifest_with_case_insensitive_test_script_is_left_alone() {
        let existing = r#"{"scripts":{"Test":"vitest run"}}"#;
        assert!(complete_node_test_runner_manifest(Some(existing)).is_none());
    }

    #[test]
    fn manifest_with_empty_test_script_is_completed() {
        let existing = r#"{"scripts":{"test":"   "}}"#;
        let completion =
            complete_node_test_runner_manifest(Some(existing)).expect("blank script is unusable");
        assert_eq!(completion.action, NodeManifestAction::AddTestScript);
        assert_eq!(
            test_script(&completion.contents).as_deref(),
            Some("node --test")
        );
    }

    #[test]
    fn malformed_manifest_is_not_clobbered() {
        assert!(complete_node_test_runner_manifest(Some("{not json")).is_none());
    }

    #[test]
    fn non_object_root_manifest_is_not_clobbered() {
        assert!(complete_node_test_runner_manifest(Some("[1, 2, 3]")).is_none());
    }

    #[test]
    fn non_object_scripts_field_is_not_clobbered() {
        let existing = r#"{"scripts":"oops"}"#;
        assert!(complete_node_test_runner_manifest(Some(existing)).is_none());
    }
}

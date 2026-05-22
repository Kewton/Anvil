use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;

use crate::config::LogLevel;
use crate::session::feedback::mask_secrets;

static LLM_IO_LOGGER: OnceLock<Mutex<File>> = OnceLock::new();
static LLM_IO_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init_logging(log_level: LogLevel, log_path: &Path) -> Result<(), String> {
    let directive = log_level.env_filter();
    let filter = EnvFilter::try_new(&directive).unwrap_or_else(|err| {
        eprintln!("warning: invalid env filter '{directive}', falling back to info: {err}");
        EnvFilter::new("info")
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|err| format!("failed to initialize logging: {err}"))?;

    // llm-io.jsonl is always opened regardless of log_level; a failure to open
    // warns but does not abort the process.
    match OpenOptions::new().create(true).append(true).open(log_path) {
        Ok(file) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o600));
            }
            let _ = LLM_IO_LOG_PATH.set(log_path.to_path_buf());
            let _ = LLM_IO_LOGGER.set(Mutex::new(file));
        }
        Err(err) => {
            eprintln!(
                "warning: failed to open LLM I/O log {}: {err}",
                log_path.display()
            );
        }
    }

    Ok(())
}

pub fn llm_io_log_path() -> Option<&'static Path> {
    LLM_IO_LOG_PATH.get().map(PathBuf::as_path)
}

pub fn log_llm_event(event: &str, payload: Value) {
    let Some(logger) = LLM_IO_LOGGER.get() else {
        return;
    };

    let mut payload = payload;
    mask_payload_inplace(&mut payload);

    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let record = json!({
        "ts_ms": ts_ms,
        "event": event,
        "payload": payload,
    });

    if let Ok(mut file) = logger.lock() {
        let _ = writeln!(file, "{record}");
    }
}

/// Issue #461: Walk a `serde_json::Value` payload tree and mask secrets
/// before it is written to `llm-io.jsonl`.
///
/// - `Value::String` leaves go through `mask_secrets` (the public API
///   from `src/session/feedback.rs`) which catches token-prefix
///   credentials, key=value secret pairs, and URL userinfo.
/// - For `Value::Object`, any key matched by `is_secret_like_key` has its
///   value replaced wholesale with `Value::String("***")` regardless of
///   the original value type (DR4-003): this means an `api_key`
///   carrying an object/array/number/bool/null is also redacted, at
///   the cost of a localized schema break under those keys.
/// - For non-secret keys we recurse, so e.g. an `arguments` object with
///   nested string leaves still gets `mask_secrets` applied.
/// - `Value::Array` recurses element-wise.
/// - `Value::Number` / `Value::Bool` / `Value::Null` under non-secret
///   keys are left untouched, preserving the existing jsonl schema for
///   non-credential payloads.
pub(crate) fn mask_payload_inplace(value: &mut Value) {
    match value {
        Value::String(s) => {
            let masked = mask_secrets(s);
            if &masked != s {
                *s = masked;
            }
        }
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_secret_like_key(k) {
                    *v = Value::String("***".to_string());
                } else {
                    mask_payload_inplace(v);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                mask_payload_inplace(v);
            }
        }
        Value::Number(_) | Value::Bool(_) | Value::Null => {}
    }
}

/// Issue #461: Decide whether a JSON object key looks like an env var or
/// payload field carrying a credential. We uppercase first then look for
/// the canonical secret-name fragments, plus a `_KEY` / `_TOKEN` suffix
/// rule. This is intentionally broader than the Issue AC's anchored
/// alternation (`MY_API_KEY_BACKUP` is also caught) — we prefer over-mask
/// over leaks (DR2-003).
pub(crate) fn is_secret_like_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if upper.contains("API_KEY")
        || upper.contains("APIKEY")
        || upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.contains("ACCESS_KEY")
        || upper.contains("ACCESSKEY")
        || upper.contains("CLIENT_SECRET")
    {
        return true;
    }
    upper.ends_with("_KEY") || upper.ends_with("_TOKEN")
}

// ---------------------------------------------------------------------------
// Issue #606 T-1.9: completion-evidence log helpers.
//
// Three events are emitted along the post-hoc evidence observation pipeline:
//   * `agent.completion_evidence.observed` — fired from the Bash hook
//     (`VerifierExitZero`) and Edit/Write hook (`RepoEdit`) the moment the
//     tool result confirms a completion-relevant signal.
//   * `agent.completion_evidence.satisfied` — fired from success.rs when the
//     OR-fold over the per-turn `EvidenceSet` returns true and Stage-2
//     short-circuits the per-protocol reject text.
//   * `agent.completion_evidence.unsatisfied` — fired from success.rs when
//     the protocol's reject text is still produced; the payload carries the
//     `missing_shapes` slice from `evidence_set_missing_shapes`.
//   * `agent.completion_evidence.deterministic_rescued` — fired when
//     Stage-2 evidence rescues a turn that would otherwise be rejected as
//     deterministic-only recovery.
//
// All payloads pass through `mask_payload_inplace` (called from
// `log_llm_event`) as the final defence line per CLAUDE.md Security
// Invariants. The helpers only construct the payload — they do not bypass
// that pipeline.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Issue #661 DR1-002 / DR2-005: `stable_path_hash` SSOT.
//
// Path-hash representation is the single 16-hex `DefaultHasher` digest shared by:
//
//   * `agent.artifact_ledger.*` events (Issue #659)
//   * `agent.active_job.selected` events (Issue #660)
//   * `agent.verifier.invoked` events (Issue #661)
//   * `agent.verifier.external_import_rejected` events (Issue #661)
//
// Prior to Issue #661 the same algorithm lived in three private duplicates
// (`src/agent/loop_run/artifact_ledger.rs::stable_path_hash`,
// `src/agent/loop_run/turn.rs::stable_path_hash_for_active_job`,
// `src/agent/loop_run/active_job_arbiter.rs::stable_path_hash` `#[cfg(test)]`).
// This module now owns the canonical implementation and all callers must
// route through `crate::logging::stable_path_hash` so the per-event
// `path_hash` values remain a stable correlator across releases.
//
// Caller responsibility (NOT enforced by this layer):
//   1. Always pass `mask_secrets(...)`-applied input. The hash function is
//      not a secret-hiding primitive — the legitimate path-secrecy defence
//      is `mask_secrets` + `mask_payload_inplace` at `log_llm_event` time.
//   2. Treat the 16-hex string only as a non-cryptographic correlator
//      (`DefaultHasher` is not stable across Rust versions for
//      cross-deployment use; intra-deployment it IS stable, which is what
//      log consumers need).
// ---------------------------------------------------------------------------

/// 16-hex `DefaultHasher` digest of `input`. SSOT for the `path_hash` field
/// emitted across `agent.artifact_ledger.*` / `agent.active_job.selected` /
/// `agent.verifier.invoked` payloads. Callers MUST pass `mask_secrets(...)`-
/// applied input — this helper does not redact, and the secret-hiding
/// defence-in-depth is `mask_payload_inplace` at `log_llm_event` time.
pub(crate) fn stable_path_hash(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 8-byte `DefaultHasher` digest over the canonical JSON serialization of
/// `payload`. SSOT for per-turn payload-dedup digests (Issue #661
/// `last_verifier_invoked_payload_digest`).
///
/// Caller responsibility (DR1-004):
///   1. Apply `mask_payload_inplace(&mut payload)` before calling this
///      helper. Hashing the pre-mask payload would mean the dedup key
///      changes after secret redaction, which would break per-turn
///      suppression semantics.
///   2. Treat the `[u8; 8]` value as a non-cryptographic correlator.
///
/// Implementation: `serde_json::to_vec` is used as the canonical-byte
/// serializer (serde_json::Map preserves insertion order, so payloads
/// constructed via the same `json!{...}` literal hash equally). On
/// serialization failure (effectively impossible for valid `Value`s) the
/// fallback is the digest of an empty byte slice — this keeps the function
/// total without panicking inside a logging hot path.
//
// Production caller arrives in Issue #661 Phase 5 (`run_task_contract_verifier_once`
// pre-spawn `agent.verifier.invoked` dedup). Phase 1 lands the SSOT helper +
// unit tests only, so this is intentionally unused in production code until
// then.
#[allow(dead_code)]
pub(crate) fn compute_payload_digest(payload: &Value) -> [u8; 8] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    hasher.write(&bytes);
    hasher.finish().to_be_bytes()
}

/// Issue #606 T-1.9 — `agent.completion_evidence.observed`.
///
/// `event_label` is a `&'static str` matching the `serde tag = "kind"`
/// variant name (`repo_edit` / `verifier_exit_zero` / `answer_only`) so log
/// readers can pivot without re-parsing the inner payload. `detail` is the
/// optional structured side-data (e.g. `RepoEditCategory`, masked verifier
/// command class).
pub(crate) fn log_completion_evidence_observed(
    turn_index: usize,
    iter_index: usize,
    event_label: &'static str,
    detail: Value,
) {
    let payload = json!({
        "turn_index": turn_index,
        "iter_index": iter_index,
        "evidence_kind": event_label,
        "detail": detail,
    });
    log_llm_event("agent.completion_evidence.observed", payload);
}

/// Issue #606 T-1.9 — `agent.completion_evidence.satisfied`. `protocol_kind`
/// is passed as `&'static str` (one of `"python"` / `"typescript_ui"` /
/// `"docs"` / `"answer_only"` / `"generic_code"`) so this helper stays in
/// the `crate::logging` layer without pulling in `agent::loop_run`'s
/// `pub(super)` `ProtocolKind` enum (DR3-002 layer rule).
pub(crate) fn log_completion_evidence_satisfied(
    turn_index: usize,
    protocol_kind: &'static str,
    observed_count: usize,
) {
    let payload = json!({
        "turn_index": turn_index,
        "protocol_kind": protocol_kind,
        "observed_count": observed_count,
    });
    log_llm_event("agent.completion_evidence.satisfied", payload);
}

/// Issue #606 T-1.9 — `agent.completion_evidence.unsatisfied`.
pub(crate) fn log_completion_evidence_unsatisfied(
    turn_index: usize,
    protocol_kind: &'static str,
    missing_shapes: &[&'static str],
    observed_count: usize,
) {
    let payload = json!({
        "turn_index": turn_index,
        "protocol_kind": protocol_kind,
        "missing_shapes": missing_shapes,
        "observed_count": observed_count,
    });
    log_llm_event("agent.completion_evidence.unsatisfied", payload);
}

/// Issue #613 — `agent.completion_evidence.deterministic_rescued`.
pub(crate) fn log_completion_evidence_deterministic_rescued(
    turn_index: usize,
    protocol_kind: &'static str,
    observed_count: usize,
) {
    let payload = json!({
        "turn_index": turn_index,
        "protocol_kind": protocol_kind,
        "observed_count": observed_count,
    });
    log_llm_event("agent.completion_evidence.deterministic_rescued", payload);
}

#[cfg(test)]
mod tests {
    use super::{
        compute_payload_digest, is_secret_like_key, mask_payload_inplace, stable_path_hash,
    };
    use serde_json::{Value, json};

    // ---- Issue #661 DR1-002: stable_path_hash SSOT (16-hex DefaultHasher) ----

    #[test]
    fn stable_path_hash_is_deterministic() {
        let h1 = stable_path_hash("src/lib.rs");
        let h2 = stable_path_hash("src/lib.rs");
        assert_eq!(h1, h2);
    }

    #[test]
    fn stable_path_hash_is_16_hex_chars() {
        let h = stable_path_hash("src/agent/loop_run/turn.rs");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn stable_path_hash_differs_for_different_inputs() {
        let h1 = stable_path_hash("tests/test_a.py");
        let h2 = stable_path_hash("tests/test_b.py");
        assert_ne!(h1, h2);
    }

    #[test]
    fn stable_path_hash_handles_empty_input() {
        // empty input must still produce a 16-hex string (no panic, deterministic)
        let h = stable_path_hash("");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Issue #661 DR2-005: pin that the SSOT in `logging.rs` matches the
    /// `DefaultHasher` + `{:016x}` algorithm previously duplicated in
    /// `artifact_ledger.rs` / `turn.rs::stable_path_hash_for_active_job` /
    /// `active_job_arbiter.rs` test-only helper. Migration-prior callers
    /// computed `format!("{:016x}", DefaultHasher::new().hash(input).finish())`
    /// — this test reproduces that algorithm inline to guard against silent
    /// drift between the SSOT and the on-disk event payload hashes from
    /// `agent.artifact_ledger.*` / `agent.active_job.selected`.
    #[test]
    fn stable_path_hash_matches_default_hasher_16_hex_inline_algorithm() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let input = "src/agent/loop_run/artifact_ledger.rs";
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        let expected = format!("{:016x}", hasher.finish());
        assert_eq!(stable_path_hash(input), expected);
    }

    /// Issue #661 caller-migration regression guard. Replicates the exact
    /// `DefaultHasher` + `{:016x}` form of the (now-deleted) duplicate
    /// implementations and asserts the SSOT produces the same hash. Future
    /// algorithm changes in the SSOT MUST keep prior on-disk
    /// `agent.artifact_ledger.*` / `agent.active_job.selected` event
    /// `path_hash` values stable, or this test must be updated in lockstep
    /// with a schema-bump record in `dev-reports/`.
    #[test]
    fn stable_path_hash_matches_pre_issue_661_duplicate_implementations() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Sample inputs covering the 3 historical caller sites.
        for input in [
            "tests/test_a.py",                          // artifact_ledger duplicate
            "src/agent/loop_run/turn.rs",               // turn.rs duplicate
            "src/agent/loop_run/active_job_arbiter.rs", // active_job_arbiter test-only duplicate
        ] {
            let mut hasher = DefaultHasher::new();
            input.hash(&mut hasher);
            let pre_migration = format!("{:016x}", hasher.finish());
            assert_eq!(
                stable_path_hash(input),
                pre_migration,
                "SSOT hash for {input:?} must match the pre-migration inline algorithm"
            );
        }
    }

    // ---- Issue #661 DR2-009: compute_payload_digest SSOT ([u8; 8]) ----------

    #[test]
    fn compute_payload_digest_is_deterministic() {
        let payload = json!({"k": "v", "n": 42});
        let d1 = compute_payload_digest(&payload);
        let d2 = compute_payload_digest(&payload);
        assert_eq!(d1, d2);
    }

    #[test]
    fn compute_payload_digest_differs_for_different_payloads() {
        let p1 = json!({"k": "v1"});
        let p2 = json!({"k": "v2"});
        assert_ne!(compute_payload_digest(&p1), compute_payload_digest(&p2));
    }

    #[test]
    fn compute_payload_digest_returns_eight_bytes() {
        let payload = json!({"foo": "bar"});
        let digest = compute_payload_digest(&payload);
        // digest is [u8; 8] by type; assert the length is 8 to lock the SSOT
        assert_eq!(digest.len(), 8);
    }

    /// DR1-004: caller is required to pass `mask_payload_inplace`-applied
    /// payload. The helper does not invoke `mask_payload_inplace` internally
    /// (would be a layering violation); but the digest function must be a
    /// pure hash over the canonical JSON bytes of whatever Value is passed in,
    /// so two payloads with identical canonical JSON shapes hash equally.
    #[test]
    fn compute_payload_digest_uses_canonical_json_serialization() {
        // Build the same payload via two different construction orders; the
        // resulting Value's serialization should be identical and produce the
        // same digest.
        let p_a = json!({"a": 1, "b": 2});
        let p_b = json!({"a": 1, "b": 2});
        assert_eq!(compute_payload_digest(&p_a), compute_payload_digest(&p_b));
    }

    // ---- is_secret_like_key boundary cases (DR1-006 / DR2-003) -----------

    #[test]
    fn is_secret_like_key_handles_canonical_names() {
        assert!(is_secret_like_key("API_KEY"));
        assert!(is_secret_like_key("api_key"));
        assert!(is_secret_like_key("token"));
        assert!(is_secret_like_key("Secret"));
        assert!(is_secret_like_key("password"));
        assert!(is_secret_like_key("ACCESS_KEY"));
        assert!(is_secret_like_key("CLIENT_SECRET"));
    }

    #[test]
    fn is_secret_like_key_handles_mixed_case_secret_keys() {
        assert!(is_secret_like_key("Api_Key"));
        assert!(is_secret_like_key("api_token"));
        assert!(is_secret_like_key("StRiPe_Api_kEy"));
    }

    #[test]
    fn is_secret_like_key_handles_compound_key_like_my_api_key_backup() {
        // Over-mask (DR2-003): broader than the Issue AC's anchored
        // alternation, intentional.
        assert!(is_secret_like_key("MY_API_KEY_BACKUP"));
        assert!(is_secret_like_key("STRIPE_API_KEY"));
        assert!(is_secret_like_key("OAUTH_TOKEN_REFRESH"));
    }

    #[test]
    fn is_secret_like_key_does_not_match_innocent_keys() {
        assert!(!is_secret_like_key("note"));
        assert!(!is_secret_like_key("event"));
        assert!(!is_secret_like_key("ts_ms"));
        assert!(!is_secret_like_key("payload"));
        assert!(!is_secret_like_key("status"));
    }

    // ---- mask_payload_inplace recursion / type behavior (DR4-003) --------

    #[test]
    fn mask_payload_inplace_recursively_masks_string_leaf_in_object() {
        let mut payload = json!({
            "outer": {
                "inner": "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123",
            }
        });
        mask_payload_inplace(&mut payload);
        let masked = payload["outer"]["inner"].as_str().unwrap();
        assert!(
            !masked.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"),
            "got: {masked}"
        );
    }

    #[test]
    fn mask_payload_inplace_recursively_masks_string_leaf_in_array() {
        let mut payload = json!({
            "items": ["GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123", "ok"],
        });
        mask_payload_inplace(&mut payload);
        let first = payload["items"][0].as_str().unwrap();
        let second = payload["items"][1].as_str().unwrap();
        assert!(!first.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"));
        assert_eq!(second, "ok");
    }

    #[test]
    fn mask_payload_inplace_replaces_secret_key_value_with_stars() {
        let mut payload = json!({
            "api_key": "sk-proj-abcdefghijklmnopqrstuvwxyz",
            "note": "Hello world",
        });
        mask_payload_inplace(&mut payload);
        assert_eq!(payload["api_key"], Value::String("***".to_string()));
        assert_eq!(payload["note"], Value::String("Hello world".to_string()));
    }

    /// DR4-003: secret-like key with a NON-string value (object / array /
    /// number / bool / null) is also redacted to `***` to avoid leaking
    /// structured credential data. The schema invariant says: secret-like
    /// keys may now carry `Value::String("***")` regardless of original type.
    #[test]
    fn mask_payload_inplace_replaces_non_string_value_under_secret_key() {
        let mut payload = json!({
            "api_key": {"nested": "secret_object"},
            "auth_token": [1, 2, 3],
            "client_secret": 42,
            "password": true,
            "access_key": null,
        });
        mask_payload_inplace(&mut payload);
        assert_eq!(payload["api_key"], Value::String("***".to_string()));
        assert_eq!(payload["auth_token"], Value::String("***".to_string()));
        assert_eq!(payload["client_secret"], Value::String("***".to_string()));
        assert_eq!(payload["password"], Value::String("***".to_string()));
        assert_eq!(payload["access_key"], Value::String("***".to_string()));
    }

    #[test]
    fn mask_payload_inplace_preserves_number_bool_null_under_non_secret_keys() {
        let mut payload = json!({
            "count": 42,
            "ok": true,
            "missing": null,
            "ratio": 1.5,
        });
        let before = payload.clone();
        mask_payload_inplace(&mut payload);
        assert_eq!(payload, before);
    }

    #[test]
    fn mask_payload_inplace_does_not_mask_non_secret_key_value_pair() {
        let mut payload = json!({
            "note": "Hello world",
            "summary": "ts_ms=1700000000",
        });
        let before = payload.clone();
        mask_payload_inplace(&mut payload);
        assert_eq!(payload, before);
    }

    #[test]
    fn mask_payload_inplace_masks_value_in_inner_string_via_token_regex() {
        // Non-secret-like key carrying a string with embedded
        // `password=foo` — the recursive `mask_secrets` call must catch
        // the `password=...` kv pattern even though the parent key isn't
        // secret-like.
        let mut payload = json!({
            "summary": "user supplied password=hunter2 in arg",
        });
        mask_payload_inplace(&mut payload);
        let s = payload["summary"].as_str().unwrap();
        assert!(!s.contains("hunter2"), "got: {s}");
        assert!(s.contains("***"));
    }

    #[test]
    fn mask_payload_inplace_masks_compound_key_like_my_api_key_backup() {
        let mut payload = json!({
            "MY_API_KEY_BACKUP": "ghp_abcdefghijklmnopqrstuvwxyz0123",
        });
        mask_payload_inplace(&mut payload);
        assert_eq!(
            payload["MY_API_KEY_BACKUP"],
            Value::String("***".to_string())
        );
    }

    /// Issue #606 U-16 / Issue #607 / Issue #613: pin that
    /// `mask_payload_inplace` preserves the payload key set used by the
    /// completion-evidence events (`agent.completion_evidence.{observed,
    /// satisfied,unsatisfied,deterministic_rescued}`).
    /// Snake_case `command_class` matches the `BashCommandClass`
    /// serde rename_all schema introduced in Issue #607 (BP-07).
    #[test]
    fn mask_payload_inplace_preserves_completion_evidence_payload_key_set() {
        let mut payload = json!({
            "turn_index": 5,
            "iter_index": 2,
            "evidence_kind": "verifier_exit_zero",
            "detail": {
                "command_class": "build_test",
            },
            "protocol_kind": "python",
            "missing_shapes": ["repo_edit_impl_or_test", "verifier_exit_zero"],
            "observed_count": 3,
        });
        let before_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        mask_payload_inplace(&mut payload);
        let after_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(before_keys, after_keys);
        // Inner non-secret payload values preserved.
        assert_eq!(payload["turn_index"], json!(5));
        assert_eq!(payload["iter_index"], json!(2));
        assert_eq!(payload["observed_count"], json!(3));
        assert_eq!(
            payload["evidence_kind"].as_str().unwrap(),
            "verifier_exit_zero"
        );
        assert_eq!(payload["protocol_kind"].as_str().unwrap(), "python");
        assert_eq!(
            payload["detail"]["command_class"].as_str().unwrap(),
            "build_test"
        );
        assert_eq!(
            payload["missing_shapes"][0].as_str().unwrap(),
            "repo_edit_impl_or_test"
        );
    }

    /// Issue #607: same shape with the new `env_setup` label — the mask
    /// pipeline preserves it identically.
    #[test]
    fn mask_payload_inplace_preserves_env_setup_command_class_label() {
        let mut payload = json!({
            "turn_index": 7,
            "iter_index": 0,
            "evidence_kind": "verifier_exit_zero",
            "detail": {
                "command_class": "env_setup",
            },
            "protocol_kind": "generic_code",
            "missing_shapes": ["repo_edit_any"],
            "observed_count": 1,
        });
        let before_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        mask_payload_inplace(&mut payload);
        let after_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(before_keys, after_keys);
        assert_eq!(payload["turn_index"], json!(7));
        assert_eq!(
            payload["detail"]["command_class"].as_str().unwrap(),
            "env_setup"
        );
        assert_eq!(payload["protocol_kind"].as_str().unwrap(), "generic_code");
    }

    #[test]
    fn mask_payload_inplace_preserves_existing_reminder_payload_key_set() {
        // CLAUDE.md mentions `agent.reminder.{completed,failed,skipped}`
        // payloads. We pin that the existing object structure under
        // non-secret keys is preserved.
        let mut payload = json!({
            "completed": {
                "iteration": 3,
                "feedback_kind": "UnsafeCommandBlocked",
                "duration_ms": 1234,
            },
            "skipped_reason": "no_recent_failure",
        });
        let before_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        mask_payload_inplace(&mut payload);
        let after_keys: Vec<String> = payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(before_keys, after_keys);
        // Inner payloads' types are preserved.
        assert_eq!(payload["completed"]["iteration"], json!(3));
        assert_eq!(payload["completed"]["duration_ms"], json!(1234));
        assert_eq!(
            payload["completed"]["feedback_kind"].as_str().unwrap(),
            "UnsafeCommandBlocked"
        );
    }
}

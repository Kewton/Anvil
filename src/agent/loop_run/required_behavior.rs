//! `RequiredBehaviorContract` — minimal schema and deterministic extractor for
//! user-request behavior contracts (Issue #635).
//!
//! ## Layer / scope (CLAUDE.md DR3-001 / DR3-002)
//!
//! - All types and helpers are kept at `pub(super)` (`pub(crate)` for the few
//!   that need to cross `task_contract.rs`). The parent module
//!   `src/agent/loop_run.rs` MUST NOT `pub use` anything from here.
//! - Direction of dependency is `agent → session` only. We import
//!   [`crate::session::feedback::mask_secrets`] as the **SSOT** for redacting
//!   request-derived text before it ever lands in a schema field
//!   (Security Invariants in CLAUDE.md).
//! - The extractor is a **pure function**: no LLM call, no I/O, no access to
//!   `Agent` / `SessionSnapshot` / log payloads.
//!
//! ## Two-stage pipeline
//!
//! 1. [`extract`] builds a candidate [`RequiredBehaviorContract`] from
//!    deterministic keyword / token rules over a bounded, masked scan of
//!    the request text. Its responsibility is **discovery** — turning a
//!    free-form request into closed-enum / cap-bounded fields.
//! 2. [`filter_against_request`] re-validates each candidate field against
//!    the same request text, dropping any term that is not literally backed
//!    by the bounded masked scan. Its responsibility is **trust narrowing**:
//!    the candidate is treated as *untrusted* (as if it had come from an
//!    LLM), so [`MAX_ARRAY`] / [`MAX_STR`] / secret redaction are reapplied,
//!    `required_artifacts` is intersected with a freshly-derived
//!    request-backed set (CB-005), and `confidence` is recomputed.
//!
//! In Issue #635 scope the two stages share the same input source, so the
//! filter is effectively a no-op on candidates produced by [`extract`]. The
//! shape exists so a future LLM-backed candidate generator can be slotted
//! in at stage 1 without weakening the request-backing guarantee.
//!
//! A third helper, [`RequiredBehaviorContract::to_artifact_roles`], is a
//! pure **projection** of the schema's `required_artifacts` into the
//! [`ArtifactRole`] SSOT consumed by completion gates. It is intentionally
//! decoupled from extraction / filtering so Issue #636 can swap the
//! contract's artifact source without touching either pipeline stage.

use super::task_contract::ArtifactRole;

/// Upper bound for the byte slice of `request` that the extractor / filter
/// will scan (DR4-002). Both raw user request and any candidate string get
/// truncated to this many UTF-8 bytes (char boundary preserved) before any
/// keyword / token scan or `to_ascii_lowercase` allocation. The cap protects
/// against DoS-style long inputs without touching the original
/// `Agent::messages` content.
pub(super) const MAX_REQUEST_SCAN_BYTES: usize = 64 * 1024;

/// Maximum number of items kept in any `Option<Vec<_>>` field.
const MAX_ARRAY: usize = 8;

/// Maximum number of bytes kept per `domain_terms` string.
const MAX_STR: usize = 64;

/// Redaction sentinels emitted by [`mask_secrets`] / related helpers. Any
/// candidate term containing one of these is dropped from `domain_terms`.
const REDACTION_SENTINELS: &[&str] = &["***", "<REDACTED>"];

/// Behavior contract extracted from a user request.
///
/// `confidence` is an `f32`, so this type intentionally only derives
/// `PartialEq` (not `Eq`). `None` represents an unknown / not-extracted
/// field and is distinct from `Some(vec![])` (extracted but empty).
///
/// `#[derive(Deserialize)]` is **not** applied in Issue #635 scope: the
/// only input path is deterministic extraction from `request: &str`. A
/// Deserialize derive would invite JSON-shaped inputs from the session
/// layer (DR3-002 risk). LLM extensions can add the derive when a JSON
/// path actually exists.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RequiredBehaviorContract {
    pub(super) operations: Option<Vec<Operation>>,
    pub(super) domain_terms: Option<Vec<String>>,
    pub(super) interface_hints: Option<Vec<InterfaceHint>>,
    pub(super) required_artifacts: Option<Vec<ArtifactKind>>,
    pub(super) verification: Option<Vec<VerificationKind>>,
    pub(super) confidence: f32,
    /// Issue #651: hard gate that says "the user request literally asked for
    /// test execution evidence". SSOT predicate is
    /// `super::task_contract::request_asks_for_test_artifact`, computed once
    /// per `extract` / `filter_against_request` call against the bounded
    /// masked scan. Distinct from `verification.contains(VerificationKind::Test)`
    /// — that one only fires on the English `test` keyword, missing
    /// `spec` / `テストも実装` (design judgement #1).
    ///
    /// `Default` is intentionally NOT implemented for this struct;
    /// every literal construction site (7 in this module + 0 elsewhere)
    /// is updated explicitly so adding a new boolean field stays
    /// compile-time visible (design policy DR2-006 / DR2-008).
    pub(super) test_execution_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Create,
    Read,
    Update,
    Delete,
    Run,
    Validate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationKind {
    Test,
    Build,
    Run,
    Smoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactKind {
    Implementation,
    Test,
    UsageDocs,
    Setup,
}

impl ArtifactKind {
    /// Map behavior-schema `ArtifactKind` to the agent-loop `ArtifactRole`
    /// SSOT used by existing completion gates.
    pub(super) fn to_role(self) -> ArtifactRole {
        match self {
            ArtifactKind::Implementation => ArtifactRole::Implementation,
            ArtifactKind::Test => ArtifactRole::Test,
            ArtifactKind::UsageDocs => ArtifactRole::UsageDocs,
            ArtifactKind::Setup => ArtifactRole::Setup,
        }
    }
}

/// Closed allowlist of interface hints (DR1-005). Keeping this as an enum
/// means extractor / validator / tests only need to add a new variant when
/// the allowlist grows — there's no free-form `String` interface hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterfaceHint {
    Api,
    Cli,
    WebUi,
    Library,
}

/// Schema validation failure. Hand-rolled `Display` / `Error` impls keep
/// `Cargo.toml` untouched (no `thiserror` direct dependency exists today,
/// DR2-007 / DR3-001).
#[derive(Debug, PartialEq)]
pub(super) enum SchemaError {
    /// One of the `Option<Vec<_>>` fields exceeded [`MAX_ARRAY`].
    ArrayTooLong { field: &'static str, len: usize },
    /// A `domain_terms` entry exceeded [`MAX_STR`] bytes.
    DomainTermTooLong { len: usize },
    /// `confidence` was non-finite or outside `[0.0, 1.0]`.
    ConfidenceInvalid { value: f32 },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::ArrayTooLong { field, len } => {
                write!(f, "{field} array too long: {len} > {MAX_ARRAY}")
            }
            SchemaError::DomainTermTooLong { len } => {
                write!(f, "domain_term too long: {len} > {MAX_STR} bytes")
            }
            SchemaError::ConfidenceInvalid { value } => {
                write!(
                    f,
                    "confidence invalid (must be finite in [0.0, 1.0]): {value}"
                )
            }
        }
    }
}

impl std::error::Error for SchemaError {}

/// Helper that returns `ArrayTooLong` when `items` exceeds [`MAX_ARRAY`].
fn check_array_len<T>(field: &'static str, items: Option<&[T]>) -> Result<(), SchemaError> {
    if let Some(items) = items
        && items.len() > MAX_ARRAY
    {
        return Err(SchemaError::ArrayTooLong {
            field,
            len: items.len(),
        });
    }
    Ok(())
}

impl RequiredBehaviorContract {
    /// Validate that the contract obeys the closed schema invariants
    /// (`MAX_ARRAY` / `MAX_STR` / finite confidence in `[0.0, 1.0]`).
    ///
    /// In Issue #635 scope the extractor is the only producer and the
    /// caller is `TaskContract::from_request`, which constructs values
    /// with `extract()` (guaranteed to validate). The method is therefore
    /// invoked through `debug_assert!(...validate().is_ok())` at the
    /// construction site. Once a JSON / LLM input path lands, callers
    /// should switch to propagating the `Result`.
    pub(super) fn validate(&self) -> Result<(), SchemaError> {
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(SchemaError::ConfidenceInvalid {
                value: self.confidence,
            });
        }
        check_array_len("operations", self.operations.as_deref())?;
        if let Some(terms) = self.domain_terms.as_deref() {
            check_array_len("domain_terms", Some(terms))?;
            for term in terms {
                if term.len() > MAX_STR {
                    return Err(SchemaError::DomainTermTooLong { len: term.len() });
                }
            }
        }
        check_array_len("interface_hints", self.interface_hints.as_deref())?;
        check_array_len("required_artifacts", self.required_artifacts.as_deref())?;
        check_array_len("verification", self.verification.as_deref())?;
        Ok(())
    }

    /// Convert behavior schema's `required_artifacts` to
    /// [`ArtifactRole`] (the SSOT used by completion / recovery code).
    ///
    /// Returns an empty `Vec` when `required_artifacts` is `None`
    /// (unknown). In Issue #635 this getter is intentionally **not**
    /// called by `TaskContract::from_request` — the existing
    /// `request_asks_for_*` gates remain the source of truth for the
    /// `required_artifacts` field. Issue #636 will become the sole
    /// caller when it switches the contract to a derived-value path
    /// (design policy §7 #1).
    #[allow(dead_code)]
    pub(super) fn to_artifact_roles(&self) -> Vec<ArtifactRole> {
        let Some(kinds) = &self.required_artifacts else {
            return Vec::new();
        };
        let mut roles: Vec<ArtifactRole> =
            kinds.iter().copied().map(ArtifactKind::to_role).collect();
        roles.sort();
        roles.dedup();
        roles
    }

    /// Issue #636: judgement API — does `excerpt` hit any of the
    /// operation keywords backing `self.operations`?
    ///
    /// Short ASCII keywords (`read`, `run`) go through the token-boundary
    /// `keyword_hit` SSOT so `README` / `running` do not false-positive.
    /// `KeywordMatch` / `OPERATION_KEYWORDS` stay private to this module
    /// (DR1-005 / DR3-001 — no facade re-export). Returns `false` when
    /// `operations` is `None` or empty.
    pub(super) fn excerpt_hits_any_operation(&self, excerpt: &str) -> bool {
        let Some(ops) = self.operations.as_ref() else {
            return false;
        };
        if ops.is_empty() {
            return false;
        }
        let lower = excerpt.to_ascii_lowercase();
        for (needle, op, mode) in OPERATION_KEYWORDS {
            if ops.contains(op) && keyword_hit(&lower, needle, *mode) {
                return true;
            }
        }
        false
    }

    /// Issue #636: judgement API — does `excerpt` hit any of the
    /// `domain_terms`?
    ///
    /// `domain_terms` are user-derived vocabulary so we match by
    /// case-insensitive substring without applying the short-token
    /// boundary rule (CB-004 keeps schema vs request alignment). Returns
    /// `false` when `domain_terms` is `None` or empty.
    pub(super) fn excerpt_hits_any_domain_term(&self, excerpt: &str) -> bool {
        let Some(terms) = self.domain_terms.as_ref() else {
            return false;
        };
        if terms.is_empty() {
            return false;
        }
        let lower = excerpt.to_ascii_lowercase();
        terms
            .iter()
            .any(|term| !term.is_empty() && lower.contains(&term.to_ascii_lowercase()))
    }
}

// ---------------------------------------------------------------------------
// Stage 1: candidate generation (`extract`).
// ---------------------------------------------------------------------------

/// Build a [`RequiredBehaviorContract`] candidate from `request` and then
/// pin it down with [`filter_against_request`].
///
/// Pure function: no LLM call, no I/O, no `Agent` / `SessionSnapshot`
/// access. Long inputs are truncated to `MAX_REQUEST_SCAN_BYTES` UTF-8
/// bytes (char boundary preserved) and run through
/// [`mask_secrets`] before any field is built.
pub(super) fn extract(request: &str) -> RequiredBehaviorContract {
    let scan_text = bounded_masked_request(request);
    let scan = scan_text.as_str();
    let lower = scan.to_ascii_lowercase();
    // Issue #651: SSOT predicate is computed once and reused by both
    // `extract_required_artifacts` (via `asks_for_tests` inside that
    // helper) and the `test_execution_required` field below.
    // `request_asks_for_test_artifact` is invoked here exactly once; the
    // call inside `extract_required_artifacts` reads the same masked
    // scan so the two stay in lock-step.
    let asks_for_tests = super::task_contract::request_asks_for_test_artifact(scan, &lower);
    let operations = extract_operations(scan, &lower);
    let domain_terms = extract_domain_terms(scan);
    let interface_hints = extract_interface_hints(&lower);
    let required_artifacts = extract_required_artifacts(request, scan, &lower);
    let verification = extract_verification(&lower);
    let confidence = confidence_from_hits(
        operations.as_ref(),
        domain_terms.as_ref(),
        interface_hints.as_ref(),
        required_artifacts.as_ref(),
        verification.as_ref(),
    );
    let candidate = RequiredBehaviorContract {
        operations,
        domain_terms,
        interface_hints,
        required_artifacts,
        verification,
        confidence,
        test_execution_required: asks_for_tests,
    };
    debug_assert!(
        candidate.validate().is_ok(),
        "extract produced invalid schema"
    );
    // Stage 2: pin the candidate against the request text. In Issue #635
    // scope the inputs are the same so this is essentially a no-op, but
    // pinning the call here keeps the security contract symmetrical:
    // every value that leaves this module is request-backed and cap-safe.
    filter_against_request(&candidate, request)
}

/// Truncate `request` to `MAX_REQUEST_SCAN_BYTES` on a char boundary and
/// pass it through the session-layer redactor SSOT.
///
/// The redactor stack mirrors `redact_verifier_command_for_storage`
/// (DR4-002): first [`mask_secrets`] handles token / kv / URL redaction,
/// then [`mask_header_family`] strips `Authorization` / `Cookie` /
/// `X-API-Key` header credentials even when their tail does not match
/// the `kv_secret_regex` keyword set. Without the second pass a short
/// header credential (< MAX_STR bytes) inside a backtick span could
/// reach `domain_terms` verbatim — see CB-002.
fn bounded_masked_request(request: &str) -> String {
    let mut end = request.len().min(MAX_REQUEST_SCAN_BYTES);
    while end > 0 && !request.is_char_boundary(end) {
        end -= 1;
    }
    let s1 = crate::session::feedback::mask_secrets(&request[..end]);
    crate::session::feedback::mask_header_family(&s1)
}

/// `Operation` keyword table. ASCII short words (`read`, `run`) need
/// token-boundary checking; longer words can use plain substring matching.
const OPERATION_KEYWORDS: &[(&str, Operation, KeywordMatch)] = &[
    ("create", Operation::Create, KeywordMatch::Substring),
    ("read", Operation::Read, KeywordMatch::TokenAscii),
    ("update", Operation::Update, KeywordMatch::Substring),
    ("delete", Operation::Delete, KeywordMatch::Substring),
    ("run", Operation::Run, KeywordMatch::TokenAscii),
    ("validate", Operation::Validate, KeywordMatch::Substring),
];

/// `VerificationKind` keyword table.
const VERIFICATION_KEYWORDS: &[(&str, VerificationKind, KeywordMatch)] = &[
    ("test", VerificationKind::Test, KeywordMatch::Substring),
    ("build", VerificationKind::Build, KeywordMatch::Substring),
    ("run", VerificationKind::Run, KeywordMatch::TokenAscii),
    ("smoke", VerificationKind::Smoke, KeywordMatch::Substring),
];

/// `InterfaceHint` keyword table (closed enum, snake_case form).
const INTERFACE_HINT_KEYWORDS: &[(&str, InterfaceHint, KeywordMatch)] = &[
    ("api", InterfaceHint::Api, KeywordMatch::TokenAscii),
    ("cli", InterfaceHint::Cli, KeywordMatch::TokenAscii),
    ("web_ui", InterfaceHint::WebUi, KeywordMatch::Substring),
    ("webui", InterfaceHint::WebUi, KeywordMatch::Substring),
    ("library", InterfaceHint::Library, KeywordMatch::Substring),
];

#[derive(Clone, Copy)]
enum KeywordMatch {
    /// `haystack.contains(needle)` is good enough (the keyword is long
    /// enough not to clash with English words like `read` in `README`).
    Substring,
    /// `read` / `run` style short ASCII words need a token boundary
    /// check so `README` / `running` don't false-positive.
    TokenAscii,
}

fn keyword_hit(lower: &str, needle: &str, mode: KeywordMatch) -> bool {
    match mode {
        KeywordMatch::Substring => lower.contains(needle),
        KeywordMatch::TokenAscii => contains_ascii_token(lower, needle),
    }
}

fn extract_operations(_scan: &str, lower: &str) -> Option<Vec<Operation>> {
    let mut hits: Vec<Operation> = Vec::new();
    for (needle, op, mode) in OPERATION_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(op) {
            hits.push(*op);
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

fn extract_verification(lower: &str) -> Option<Vec<VerificationKind>> {
    let mut hits: Vec<VerificationKind> = Vec::new();
    for (needle, kind, mode) in VERIFICATION_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(kind) {
            hits.push(*kind);
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

fn extract_interface_hints(lower: &str) -> Option<Vec<InterfaceHint>> {
    let mut hits: Vec<InterfaceHint> = Vec::new();
    for (needle, hint, mode) in INTERFACE_HINT_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(hint) {
            hits.push(*hint);
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

/// Pull out "code-like" tokens from the bounded scan: backtick / quoted
/// spans, paths, CamelCase / snake_case identifiers. Each surviving term
/// must round-trip through [`mask_secrets`] unchanged and must not contain
/// any redaction sentinel, so raw secret-like values never reach the
/// schema (DR4-001). At most [`MAX_ARRAY`] terms are returned and each is
/// at most [`MAX_STR`] bytes.
fn extract_domain_terms(scan: &str) -> Option<Vec<String>> {
    let mut hits: Vec<String> = Vec::new();
    collect_delimited_terms(scan, '`', '`', &mut hits);
    collect_delimited_terms(scan, '"', '"', &mut hits);
    collect_delimited_terms(scan, '\'', '\'', &mut hits);
    if hits.len() < MAX_ARRAY {
        collect_identifier_terms(scan, &mut hits);
    }
    sanitize_domain_terms(hits)
}

/// Iterate `scan` once, pulling out `open ... close` spans in linear
/// order. Uses an absolute `cursor: usize` byte offset into `scan` so
/// that consecutive spans (`` `Foo` and `Bar` ``) all advance forward
/// without resetting (CB-001). The previous implementation rebuilt a
/// `char_indices()` iterator on the suffix but ignored the suffix
/// origin, which could either revisit the same delimiter or loop
/// forever when the same delimiter character appeared twice in
/// short succession.
fn collect_delimited_terms(scan: &str, open: char, close: char, hits: &mut Vec<String>) {
    let mut cursor: usize = 0;
    while cursor < scan.len() {
        let Some(rel_open) = scan[cursor..].find(open) else {
            return;
        };
        let open_byte = cursor + rel_open;
        let term_start = open_byte + open.len_utf8();
        if term_start > scan.len() {
            return;
        }
        let Some(rel_close) = scan[term_start..].find(close) else {
            return;
        };
        let close_byte = term_start + rel_close;
        let term = &scan[term_start..close_byte];
        if !term.is_empty() {
            push_domain_term(hits, term);
            if hits.len() >= MAX_ARRAY {
                return;
            }
        }
        // Always advance past the closing delimiter so the next iteration
        // makes forward progress. Even if `term` was empty / rejected we
        // must not revisit `close_byte`.
        cursor = close_byte + close.len_utf8();
    }
}

fn collect_identifier_terms(scan: &str, hits: &mut Vec<String>) {
    let mut current = String::new();
    for ch in scan.chars() {
        if is_identifier_char(ch) {
            current.push(ch);
        } else {
            consider_identifier(&current, hits);
            current.clear();
            if hits.len() >= MAX_ARRAY {
                return;
            }
        }
    }
    consider_identifier(&current, hits);
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '/' || ch == '-'
}

fn consider_identifier(candidate: &str, hits: &mut Vec<String>) {
    if hits.len() >= MAX_ARRAY {
        return;
    }
    if candidate.is_empty() {
        return;
    }
    if !looks_like_domain_term(candidate) {
        return;
    }
    push_domain_term(hits, candidate);
}

fn looks_like_domain_term(candidate: &str) -> bool {
    // Skip pure-ASCII lowercase common-word tokens; we want identifiers
    // that look "code-like": CamelCase, snake_case, paths, dotted names,
    // hyphenated names, or anything with a digit.
    let has_upper = candidate.chars().any(|c| c.is_ascii_uppercase());
    let has_struct_punct =
        candidate.contains('_') || candidate.contains('.') || candidate.contains('/');
    let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
    has_upper || has_struct_punct || has_digit
}

fn push_domain_term(hits: &mut Vec<String>, term: &str) {
    if hits.len() >= MAX_ARRAY {
        return;
    }
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.len() > MAX_STR {
        return;
    }
    let owned = trimmed.to_string();
    if hits.iter().any(|existing| existing == &owned) {
        return;
    }
    hits.push(owned);
}

/// Drop any term that `mask_secrets` would change or that contains a
/// redaction sentinel, so raw secret-like tokens never reach the schema.
fn sanitize_domain_terms(hits: Vec<String>) -> Option<Vec<String>> {
    let mut clean: Vec<String> = Vec::new();
    for term in hits {
        if term.len() > MAX_STR {
            continue;
        }
        if contains_redaction_sentinel(&term) {
            continue;
        }
        if crate::session::feedback::mask_secrets(&term) != term {
            continue;
        }
        clean.push(term);
        if clean.len() >= MAX_ARRAY {
            break;
        }
    }
    if clean.is_empty() { None } else { Some(clean) }
}

fn contains_redaction_sentinel(term: &str) -> bool {
    REDACTION_SENTINELS.iter().any(|s| term.contains(s))
}

/// Thin delegation to the existing `request_asks_for_*` gates in
/// `task_contract.rs` (DR1-004). The artifact gate is the canonical
/// source of truth; we just project it into closed-enum form so the
/// schema stays self-contained.
///
/// ## Inputs
///
/// - `_request` is currently unused; the bounded masked `scan` /
///   `lower` views are the only source of truth (CB-003). The argument
///   is retained for symmetry with [`filter_against_request`], which
///   needs the raw request for its own redaction pass.
/// - `scan` is the [`bounded_masked_request`] result.
/// - `lower` is `scan.to_ascii_lowercase()`.
///
/// ## CB-003 (bounded scan)
///
/// Both raw and lowercase inputs to the gates are sourced from the
/// bounded masked scan — never the unbounded raw `request`. This keeps
/// the schema honest for >64KiB requests where a trigger token past
/// the cap would otherwise still flip an artifact bit.
///
/// ## CB-004 (Install intent alignment)
///
/// Setup follows the **same** `Install`-only rule as
/// [`task_contract::TaskContract::from_request`]. The rule is
/// canonically defined by
/// [`task_contract::request_asks_for_code_work`]:
///
/// ```text
/// Install ⇔ asks_for_setup && !request_asks_for_code_work
/// ```
///
/// We reuse `request_asks_for_code_work` verbatim instead of
/// approximating it with `!asks_for_impl`. The approximation was
/// unsound for requests like `"add dependencies to package.json"`
/// where:
///
/// - support-aware `asks_for_impl` is `false` (only `add` matches and
///   the support branch needs a `production_action`), but
/// - support-blind `asks_for_code_work` is `true` (the unconstrained
///   `edit_action` branch picks up `add`).
///
/// Without the shared rule, the schema would have flagged Setup
/// `required` while `TaskContract` kept it `optional`.
fn extract_required_artifacts(
    _request: &str,
    scan: &str,
    lower: &str,
) -> Option<Vec<ArtifactKind>> {
    let asks_for_tests = super::task_contract::request_asks_for_test_artifact(scan, lower);
    let asks_for_usage_docs = super::task_contract::request_asks_for_usage_docs(scan, lower);
    let asks_for_setup = super::task_contract::request_asks_for_setup(scan, lower);
    let asks_for_impl = super::task_contract::request_asks_for_implementation_artifact(
        scan,
        lower,
        asks_for_tests,
        asks_for_usage_docs,
        asks_for_setup,
    );

    let mut kinds: Vec<ArtifactKind> = Vec::new();
    if asks_for_impl {
        kinds.push(ArtifactKind::Implementation);
    }
    if asks_for_tests {
        kinds.push(ArtifactKind::Test);
    }
    if asks_for_usage_docs {
        kinds.push(ArtifactKind::UsageDocs);
    }
    if asks_for_setup && setup_is_required(scan, lower) {
        kinds.push(ArtifactKind::Setup);
    }
    if kinds.is_empty() { None } else { Some(kinds) }
}

/// Mirror [`task_contract::TaskContract::from_request`]'s rule for
/// promoting Setup from `optional` to `required`: Setup is only
/// required when intent is `Install`, which
/// [`task_contract::infer_intent`] (the canonical site) defines as
/// `asks_for_setup && !request_asks_for_code_work` (CB-004).
///
/// This calls
/// [`task_contract::request_asks_for_code_work`] directly so the two
/// paths can never drift. The caller is responsible for the leading
/// `asks_for_setup` check.
fn setup_is_required(scan: &str, lower: &str) -> bool {
    !super::task_contract::request_asks_for_code_work(scan, lower)
}

fn confidence_from_hits(
    operations: Option<&Vec<Operation>>,
    domain_terms: Option<&Vec<String>>,
    interface_hints: Option<&Vec<InterfaceHint>>,
    required_artifacts: Option<&Vec<ArtifactKind>>,
    verification: Option<&Vec<VerificationKind>>,
) -> f32 {
    let any_hit = operations.map(|v| !v.is_empty()).unwrap_or(false)
        || domain_terms.map(|v| !v.is_empty()).unwrap_or(false)
        || interface_hints.map(|v| !v.is_empty()).unwrap_or(false)
        || required_artifacts.map(|v| !v.is_empty()).unwrap_or(false)
        || verification.map(|v| !v.is_empty()).unwrap_or(false);
    if any_hit { 1.0 } else { 0.0 }
}

/// Token-boundary `contains` for ASCII short words. Mirrors the helper
/// already used in `task_contract.rs`.
fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before = haystack[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        let after_idx = idx + needle.len();
        let after = haystack[after_idx..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        before && after
    })
}

// ---------------------------------------------------------------------------
// Stage 2: request-backed filter (`filter_against_request`).
// ---------------------------------------------------------------------------

/// Pin a candidate [`RequiredBehaviorContract`] against `request` text.
///
/// Algorithm:
///
/// - `operations`: keep variants whose snake_case keyword appears in the
///   bounded masked lowercase scan (token-boundary checked for short
///   ASCII words).
/// - `domain_terms`: each candidate term is treated as untrusted —
///   reapply `mask_secrets`, drop redaction sentinels and oversize
///   terms, then keep only those that appear in the bounded scan.
/// - `interface_hints` / `verification`: same keyword check as
///   `operations`.
/// - `required_artifacts`: closed-enum gate output, kept as-is (already
///   request-backed via the deterministic `request_asks_for_*`
///   delegation).
/// - `confidence`: recomputed from the surviving fields; the candidate
///   value is **not** trusted.
///
/// The function re-applies [`bounded_masked_request`] internally, so it
/// is safe to call with the original raw `request` even if the candidate
/// was built from a different source (e.g. a future LLM proposal).
pub(super) fn filter_against_request(
    candidate: &RequiredBehaviorContract,
    request: &str,
) -> RequiredBehaviorContract {
    let scan_text = bounded_masked_request(request);
    let scan = scan_text.as_str();
    let lower = scan.to_ascii_lowercase();

    let operations = candidate.operations.as_ref().and_then(|ops| {
        let mut kept: Vec<Operation> = Vec::new();
        for op in ops {
            if kept.contains(op) {
                continue;
            }
            if !operation_in_request(*op, &lower) {
                continue;
            }
            kept.push(*op);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    let domain_terms = candidate
        .domain_terms
        .as_ref()
        .and_then(|terms| filter_domain_terms(terms, scan));

    let interface_hints = candidate.interface_hints.as_ref().and_then(|hints| {
        let mut kept: Vec<InterfaceHint> = Vec::new();
        for hint in hints {
            if kept.contains(hint) {
                continue;
            }
            if !interface_hint_in_request(*hint, &lower) {
                continue;
            }
            kept.push(*hint);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    // CB-005: candidate.required_artifacts is treated as untrusted. We
    // recompute the request-backed artifact set against the bounded
    // masked scan (same source as the rest of the filter) and intersect
    // with the candidate. Anything the request does not back is
    // dropped, so e.g. a candidate Test against an "explain Foo"
    // request becomes empty.
    let backed_artifacts = extract_required_artifacts(request, scan, &lower);
    let required_artifacts = candidate.required_artifacts.as_ref().map(|arts| {
        let mut kept: Vec<ArtifactKind> = Vec::new();
        for art in arts {
            if kept.contains(art) {
                continue;
            }
            let backed = backed_artifacts.as_ref().is_some_and(|b| b.contains(art));
            if !backed {
                continue;
            }
            kept.push(*art);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        kept
    });
    let required_artifacts = required_artifacts.filter(|v| !v.is_empty());

    let verification = candidate.verification.as_ref().and_then(|kinds| {
        let mut kept: Vec<VerificationKind> = Vec::new();
        for kind in kinds {
            if kept.contains(kind) {
                continue;
            }
            if !verification_in_request(*kind, &lower) {
                continue;
            }
            kept.push(*kind);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    let confidence = confidence_from_hits(
        operations.as_ref(),
        domain_terms.as_ref(),
        interface_hints.as_ref(),
        required_artifacts.as_ref(),
        verification.as_ref(),
    );

    // Issue #651: `verification` may have lost the Test variant during
    // narrowing above; the test_execution_required gate is still derived
    // from the bounded masked request directly so a `spec` / `テストも実装`
    // request keeps `test_execution_required = true` even when the
    // `Test` verification kind drops out (design judgement #1).
    let test_execution_required =
        super::task_contract::request_asks_for_test_artifact(scan, &lower);

    let filtered = RequiredBehaviorContract {
        operations,
        domain_terms,
        interface_hints,
        required_artifacts,
        verification,
        confidence,
        test_execution_required,
    };
    debug_assert!(
        filtered.validate().is_ok(),
        "filter_against_request produced invalid schema"
    );
    filtered
}

fn operation_in_request(op: Operation, lower: &str) -> bool {
    for (needle, candidate, mode) in OPERATION_KEYWORDS {
        if *candidate == op && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn verification_in_request(kind: VerificationKind, lower: &str) -> bool {
    for (needle, candidate, mode) in VERIFICATION_KEYWORDS {
        if *candidate == kind && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn interface_hint_in_request(hint: InterfaceHint, lower: &str) -> bool {
    for (needle, candidate, mode) in INTERFACE_HINT_KEYWORDS {
        if *candidate == hint && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn filter_domain_terms(terms: &[String], scan: &str) -> Option<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for term in terms {
        if term.len() > MAX_STR {
            continue;
        }
        if contains_redaction_sentinel(term) {
            continue;
        }
        if crate::session::feedback::mask_secrets(term) != *term {
            continue;
        }
        if !scan.contains(term.as_str()) {
            continue;
        }
        if kept.iter().any(|existing| existing == term) {
            continue;
        }
        kept.push(term.clone());
        if kept.len() >= MAX_ARRAY {
            break;
        }
    }
    if kept.is_empty() { None } else { Some(kept) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Group A: schema validation (5 tests)
    // -----------------------------------------------------------------

    fn empty_contract() -> RequiredBehaviorContract {
        RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 0.0,
            // Issue #651: explicit default at every literal site so
            // adding a new boolean stays compile-time visible.
            test_execution_required: false,
        }
    }

    #[test]
    fn validate_rejects_confidence_above_one() {
        let mut c = empty_contract();
        c.confidence = 1.5;
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ConfidenceInvalid { .. })
        ));
    }

    #[test]
    fn validate_rejects_nan_confidence() {
        let mut c = empty_contract();
        c.confidence = f32::NAN;
        // f32::NAN != f32::NAN, so use matches! + is_nan() check.
        match c.validate() {
            Err(SchemaError::ConfidenceInvalid { value }) => {
                assert!(value.is_nan(), "expected NaN, got {value}");
            }
            other => panic!("expected ConfidenceInvalid, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_too_many_operations() {
        let mut c = empty_contract();
        c.operations = Some(vec![
            Operation::Create,
            Operation::Read,
            Operation::Update,
            Operation::Delete,
            Operation::Run,
            Operation::Validate,
            Operation::Create,
            Operation::Read,
            Operation::Update,
        ]);
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ArrayTooLong {
                field: "operations",
                ..
            })
        ));
    }

    #[test]
    fn validate_rejects_long_domain_term() {
        let mut c = empty_contract();
        c.domain_terms = Some(vec!["a".repeat(MAX_STR + 1)]);
        assert!(matches!(
            c.validate(),
            Err(SchemaError::DomainTermTooLong { .. })
        ));
    }

    #[test]
    fn validate_accepts_valid_minimal_schema() {
        let c = empty_contract();
        assert!(c.validate().is_ok());
        let mut c = empty_contract();
        c.confidence = 1.0;
        c.operations = Some(vec![Operation::Create]);
        c.domain_terms = Some(vec!["Task".to_string()]);
        assert!(c.validate().is_ok());
    }

    // -----------------------------------------------------------------
    // Group B: extract + filter (8 tests)
    // -----------------------------------------------------------------

    #[test]
    fn extract_finds_quoted_domain_term() {
        let c = extract("Build a `Task` API with create/read");
        let terms = c.domain_terms.unwrap_or_default();
        assert!(terms.contains(&"Task".to_string()), "got: {terms:?}");
    }

    #[test]
    fn extract_does_not_treat_readme_as_read_operation() {
        let c = extract("Add a README file");
        let ops = c.operations.unwrap_or_default();
        assert!(!ops.contains(&Operation::Read), "got ops: {ops:?}");
    }

    #[test]
    fn extract_returns_none_for_free_form_domain_term() {
        let c = extract("please make me happy");
        assert!(c.domain_terms.is_none(), "got: {:?}", c.domain_terms);
    }

    #[test]
    fn extract_handles_long_request_within_scan_budget() {
        // Place an identifier-shaped token beyond the scan budget; it
        // must NOT make it into domain_terms because we cap the scan.
        let padding = "a".repeat(MAX_REQUEST_SCAN_BYTES + 128);
        let request = format!("{padding} `PastCap`");
        let c = extract(&request);
        let terms = c.domain_terms.unwrap_or_default();
        assert!(
            !terms.contains(&"PastCap".to_string()),
            "expected PastCap to be beyond scan budget, got: {terms:?}"
        );
    }

    #[test]
    fn extract_does_not_keep_raw_secret_like_token() {
        let c = extract("Build `Task` with API_KEY=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(!joined.contains("sk-proj-"), "got: {terms:?}");
        assert!(!joined.contains("API_KEY=***"), "got: {terms:?}");
        assert!(!joined.contains("***"), "got: {terms:?}");
    }

    #[test]
    fn filter_against_request_drops_unbacked_term() {
        let candidate = RequiredBehaviorContract {
            operations: Some(vec![Operation::Create, Operation::Delete]),
            domain_terms: Some(vec!["Foo".to_string(), "Bar".to_string()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
        };
        // Request mentions create + Foo only; Delete / Bar are unbacked.
        let filtered = filter_against_request(&candidate, "create a Foo");
        assert_eq!(filtered.operations.unwrap(), vec![Operation::Create]);
        assert_eq!(filtered.domain_terms.unwrap(), vec!["Foo".to_string()]);
    }

    #[test]
    fn filter_against_request_truncates_oversized_candidate() {
        let oversized = "x".repeat(MAX_STR + 1);
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: Some(vec![oversized.clone()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
        };
        let request = format!("use {oversized}");
        let filtered = filter_against_request(&candidate, &request);
        assert!(
            filtered.domain_terms.is_none(),
            "oversized term must be dropped, got: {:?}",
            filtered.domain_terms
        );
    }

    #[test]
    fn filter_against_request_redacts_secret_in_candidate() {
        let token = "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: Some(vec![token.clone()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
        };
        let filtered = filter_against_request(&candidate, &format!("use token {token}"));
        assert!(
            filtered.domain_terms.is_none(),
            "secret-like candidate must be dropped, got: {:?}",
            filtered.domain_terms
        );
    }

    // -----------------------------------------------------------------
    // Group C: integration + signature lock (2 tests)
    // -----------------------------------------------------------------

    #[test]
    fn unknown_required_artifacts_does_not_clear_existing_gates() {
        // Free-form code work request: existing gate should require
        // Implementation. Behavior schema is allowed to leave its own
        // required_artifacts as None / partial, but the TaskContract
        // gate must still be populated by the existing deterministic path.
        let contract =
            super::super::task_contract::TaskContract::from_request("implement feature X");
        assert!(
            contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Implementation),
            "got: {:?}",
            contract.required_artifacts
        );
    }

    #[test]
    fn extract_signature_is_pure_function() {
        // Compile-time signature lock (DR1-007). These must NOT compile
        // if extract / filter_against_request ever start taking Agent /
        // SessionSnapshot or otherwise lose pure-function purity.
        let _: fn(&str) -> RequiredBehaviorContract = extract;
        let _: fn(&RequiredBehaviorContract, &str) -> RequiredBehaviorContract =
            filter_against_request;
    }

    // -----------------------------------------------------------------
    // Group D: Codex regression coverage (CB-001 .. CB-005)
    // -----------------------------------------------------------------

    /// CB-001 (High, potential DoS): the previous `collect_delimited_terms`
    /// rebuilt a `char_indices()` iterator on the suffix after each match
    /// but ignored the returned indices, so subsequent matches could either
    /// loop forever or visit the same delimiter twice. Multiple backtick /
    /// quote spans in a single request must be extracted in linear order
    /// without hanging and must terminate.
    #[test]
    fn extract_handles_multiple_delimited_domain_terms() {
        let c = extract("Build `Foo` and `Bar` then ship `Baz`");
        let terms = c.domain_terms.unwrap_or_default();
        assert!(terms.contains(&"Foo".to_string()), "got: {terms:?}");
        assert!(terms.contains(&"Bar".to_string()), "got: {terms:?}");
        assert!(terms.contains(&"Baz".to_string()), "got: {terms:?}");
    }

    /// CB-001 follow-up: mixed delimiters (backtick, single-quote,
    /// double-quote) must each yield their own terms when more than one
    /// span of each kind exists. This is the worst case for the previous
    /// iterator-restart bug because two `collect_delimited_terms` calls in
    /// a row both hit multiple-span input.
    #[test]
    fn extract_handles_mixed_multiple_delimiters() {
        let c = extract("`Alpha` 'Beta' \"Gamma\" `Delta` 'Epsilon' \"Zeta\"");
        let terms = c.domain_terms.unwrap_or_default();
        for expected in ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta"] {
            assert!(
                terms.contains(&expected.to_string()),
                "missing {expected}, got: {terms:?}"
            );
        }
    }

    /// CB-002 (High, secret leakage): a backtick-quoted `Authorization`
    /// header value short enough to fit MAX_STR was kept verbatim in
    /// `domain_terms`. Stacking `mask_header_family` on top of
    /// `mask_secrets` must redact the credential tail so no raw token
    /// reaches the schema.
    #[test]
    fn extract_drops_authorization_header_in_quoted_term() {
        let c = extract("Use `Authorization: Bearer abc123def456` in the API client");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(
            !joined.contains("Bearer abc123def456"),
            "raw Authorization credential leaked: {terms:?}"
        );
        assert!(
            !joined.contains("abc123def456"),
            "raw credential tail leaked: {terms:?}"
        );
    }

    /// CB-002 follow-up: same coverage for Cookie header credentials.
    #[test]
    fn extract_drops_cookie_header_in_quoted_term() {
        let c = extract("Send `Cookie: session=abcdef123456` along with each request");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(
            !joined.contains("session=abcdef123456"),
            "raw Cookie credential leaked: {terms:?}"
        );
    }

    /// CB-003 (Medium, DoS): the previous `extract_required_artifacts`
    /// passed the raw `request` straight to `request_asks_for_*` gates,
    /// so a 64KiB+ request with a trigger token past the scan cap still
    /// caused artifact detection. The bounded scan view must gate the
    /// substring search.
    #[test]
    fn extract_required_artifacts_respects_scan_budget() {
        // Place a Japanese "作成" trigger past the scan cap. Without the
        // bounded view, the gate fires; with it, it does not.
        let padding = "a".repeat(MAX_REQUEST_SCAN_BYTES + 64);
        let request = format!("{padding} 作成");
        let c = extract(&request);
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Implementation),
            "trigger past scan cap should not produce Implementation, got: {arts:?}"
        );
    }

    /// CB-004 (Medium): TaskContract treats Setup as **required** only
    /// when intent is `Install` (Setup-only request). When the request
    /// mixes Build + Setup (e.g. "FastAPI を作成して dependencies を追加")
    /// Setup is `optional` in TaskContract, so the behavior schema must
    /// not promote it to `required_artifacts` either.
    #[test]
    fn extract_setup_required_only_for_install_intent() {
        // Mixed Build + Setup: Setup must NOT be in required_artifacts.
        let c = extract(
            "FastAPI のサーバを作成して dependencies を requirements.txt に追加してください",
        );
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must be optional when intent is Build, got: {arts:?}"
        );
        assert!(
            arts.contains(&ArtifactKind::Implementation),
            "Implementation must still be required, got: {arts:?}"
        );

        // Pure Install intent: Setup IS required.
        let c2 = extract("Install the dependencies from requirements.txt");
        let arts2 = c2.required_artifacts.unwrap_or_default();
        assert!(
            arts2.contains(&ArtifactKind::Setup),
            "Setup must be required for pure Install intent, got: {arts2:?}"
        );
    }

    /// CB-004 round-2: the `!asks_for_impl` shortcut that the schema
    /// previously used to gate Setup diverged from
    /// `TaskContract::from_request` for requests like
    /// `"add dependencies to package.json"`:
    ///
    /// - support-aware `asks_for_impl` is `false` because the support
    ///   branch needs a `production_action` keyword and `add` is only
    ///   an `edit_action`,
    /// - but support-blind `request_asks_for_code_work` is `true` (the
    ///   `edit_action` branch picks up `add`), so `TaskContract` infers
    ///   intent = `Build`/`Modify` and keeps Setup `optional`.
    ///
    /// The fix routes the schema through the canonical
    /// `request_asks_for_code_work` so both paths agree: Setup stays
    /// off the required list here.
    #[test]
    fn extract_setup_not_required_for_modify_setup_request() {
        let c = extract("add dependencies to package.json");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must be optional when the request has a code-work signal, got: {arts:?}"
        );
        // Cross-check against the canonical TaskContract path so the
        // two sources of truth cannot drift again.
        let contract = super::super::task_contract::TaskContract::from_request(
            "add dependencies to package.json",
        );
        assert!(
            !contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Setup),
            "TaskContract must agree that Setup is optional here, got: {:?}",
            contract.required_artifacts
        );
    }

    /// CB-004 round-2: a pure Install request (Setup keyword, no code-work
    /// signal) must still promote Setup to `required_artifacts`. This is
    /// the positive counterpart to
    /// `extract_setup_not_required_for_modify_setup_request` and locks in
    /// the canonical `Install` rule
    /// `asks_for_setup && !request_asks_for_code_work`.
    #[test]
    fn extract_setup_required_for_install_request() {
        let c = extract("Install dependencies from requirements.txt");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            arts.contains(&ArtifactKind::Setup),
            "Setup must be required for pure Install intent, got: {arts:?}"
        );
        // Cross-check against the canonical TaskContract path.
        let contract = super::super::task_contract::TaskContract::from_request(
            "Install dependencies from requirements.txt",
        );
        assert!(
            contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Setup),
            "TaskContract must agree that Setup is required here, got: {:?}",
            contract.required_artifacts
        );
    }

    /// CB-004 round-2: a pure Build request (e.g. "implement feature X")
    /// must NOT carry Setup in `required_artifacts` regardless of
    /// whether any Setup keyword appears — the schema and TaskContract
    /// agree that Setup is irrelevant when the request is code-work
    /// only. This pins the third corner of the Build / Modify / Install
    /// matrix together with the two tests above.
    #[test]
    fn extract_setup_absent_for_pure_build_request() {
        let c = extract("implement feature X");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must not appear for a pure Build request, got: {arts:?}"
        );
        assert!(
            arts.contains(&ArtifactKind::Implementation),
            "Implementation must be required for a pure Build request, got: {arts:?}"
        );
    }

    /// CB-005 (Low): `filter_against_request` must not blindly trust
    /// `candidate.required_artifacts`. When the request does not ask for
    /// a Test artifact, a Test entry from the candidate must be dropped.
    #[test]
    fn filter_against_request_drops_unbacked_required_artifacts() {
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: Some(vec![ArtifactKind::Test, ArtifactKind::Implementation]),
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
        };
        // The request is an explain-only / read-only ask. It backs
        // neither Test nor Implementation, so both must be filtered out.
        let filtered = filter_against_request(&candidate, "explain Foo");
        let arts = filtered.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Test),
            "unbacked Test must be dropped, got: {arts:?}"
        );
        assert!(
            !arts.contains(&ArtifactKind::Implementation),
            "unbacked Implementation must be dropped, got: {arts:?}"
        );
    }

    // -----------------------------------------------------------------
    // Group E: Issue #636 judgement APIs (5 tests)
    // -----------------------------------------------------------------

    fn contract_with_ops(ops: Vec<Operation>) -> RequiredBehaviorContract {
        let mut c = empty_contract();
        c.operations = Some(ops);
        c
    }

    fn contract_with_terms(terms: Vec<String>) -> RequiredBehaviorContract {
        let mut c = empty_contract();
        c.domain_terms = Some(terms);
        c
    }

    #[test]
    fn excerpt_hits_any_operation_matches_with_token_boundary_for_short_keyword() {
        let c = contract_with_ops(vec![Operation::Read]);
        // `README` should NOT count as a `read` operation.
        assert!(!c.excerpt_hits_any_operation("Update README only"));
        // Bare `read` token boundary matches.
        assert!(c.excerpt_hits_any_operation("we will Read the user record"));
        // `run` token boundary
        let c = contract_with_ops(vec![Operation::Run]);
        assert!(!c.excerpt_hits_any_operation("the running task"));
        assert!(c.excerpt_hits_any_operation("run the verifier"));
    }

    #[test]
    fn excerpt_hits_any_operation_matches_substring_for_long_keyword() {
        for op in [
            Operation::Create,
            Operation::Update,
            Operation::Delete,
            Operation::Validate,
        ] {
            let c = contract_with_ops(vec![op]);
            let excerpt = match op {
                Operation::Create => "We CREATE a new entity here",
                Operation::Update => "Update the existing record",
                Operation::Delete => "delete the row from storage",
                Operation::Validate => "Validate the payload structure",
                _ => unreachable!(),
            };
            assert!(
                c.excerpt_hits_any_operation(excerpt),
                "op {op:?} should match {excerpt:?}"
            );
        }
    }

    #[test]
    fn excerpt_hits_any_operation_returns_false_when_operations_is_none() {
        let c = empty_contract();
        assert!(!c.excerpt_hits_any_operation("create read update delete"));
        let mut c2 = empty_contract();
        c2.operations = Some(Vec::new());
        assert!(!c2.excerpt_hits_any_operation("create read update delete"));
    }

    #[test]
    fn excerpt_hits_any_domain_term_matches_case_insensitive() {
        let c = contract_with_terms(vec!["Task".to_string(), "api/v1".to_string()]);
        assert!(c.excerpt_hits_any_domain_term("def list_tasks():\n    return Task.all()"));
        assert!(c.excerpt_hits_any_domain_term("GET /api/v1/items"));
        assert!(!c.excerpt_hits_any_domain_term("nothing relevant here"));
    }

    #[test]
    fn excerpt_hits_any_domain_term_returns_false_when_domain_terms_is_none() {
        let c = empty_contract();
        assert!(!c.excerpt_hits_any_domain_term("Task api"));
        let mut c2 = empty_contract();
        c2.domain_terms = Some(Vec::new());
        assert!(!c2.excerpt_hits_any_domain_term("Task api"));
    }

    // -----------------------------------------------------------------
    // Group F (Issue #651): test_execution_required is SSOT-aligned
    // with `super::task_contract::request_asks_for_test_artifact`.
    // -----------------------------------------------------------------

    /// Helper: assert that `extract(request).test_execution_required`
    /// matches the SSOT predicate `request_asks_for_test_artifact`
    /// applied to the same bounded masked scan.
    fn assert_test_execution_required_matches_ssot(request: &str) {
        let c = extract(request);
        let scan = bounded_masked_request(request);
        let lower = scan.to_ascii_lowercase();
        let expected = super::super::task_contract::request_asks_for_test_artifact(&scan, &lower);
        assert_eq!(
            c.test_execution_required, expected,
            "request={request:?} test_execution_required diverged from SSOT predicate"
        );
        assert!(
            expected,
            "request={request:?} should request test execution (SSOT must be true)"
        );
    }

    #[test]
    fn extract_marks_test_execution_required_for_english_test_keyword() {
        assert_test_execution_required_matches_ssot("Please add a unit test for the API handler");
    }

    #[test]
    fn extract_marks_test_execution_required_for_pytest_keyword() {
        assert_test_execution_required_matches_ssot("write pytest cases for the new module");
    }

    #[test]
    fn extract_marks_test_execution_required_for_unittest_keyword() {
        assert_test_execution_required_matches_ssot("add unittest coverage for the parser");
    }

    #[test]
    fn extract_marks_test_execution_required_for_spec_keyword() {
        assert_test_execution_required_matches_ssot("write a spec describing the CRUD flow");
    }

    #[test]
    fn extract_marks_test_execution_required_for_japanese_test_keyword() {
        assert_test_execution_required_matches_ssot(
            "FastAPIでCRUDのAPIを開発してください。テストも実装してください。",
        );
    }
}

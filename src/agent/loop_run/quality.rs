use std::path::{Path, PathBuf};

use crate::session::store::ConversationMessage;

const NEXT_VERSION: &str = "14.2.35";
const REACT_VERSION: &str = "18.2.0";
const REACT_TYPES_VERSION: &str = "18.2.66";
const REACT_DOM_TYPES_VERSION: &str = "18.2.22";
const NODE_TYPES_VERSION: &str = "20.11.30";
const TYPESCRIPT_VERSION: &str = "5.4.5";
const VITE_VERSION: &str = "5.4.11";
const VITE_REACT_PLUGIN_VERSION: &str = "4.3.4";
const NUXT_VERSION: &str = "3.13.2";
const VUE_VERSION: &str = "3.5.13";
const SVELTE_VERSION: &str = "4.2.19";
const SVELTEKIT_VERSION: &str = "2.7.7";
const SVELTE_VITE_PLUGIN_VERSION: &str = "3.1.2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameworkKind {
    Next,
    React,
    Nuxt,
    SvelteKit,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestOperation {
    Create,
    Improve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestIntent {
    framework: FrameworkKind,
    interactive_experience: bool,
    game_experience: bool,
    operation: RequestOperation,
    asks_for_app_or_ui: bool,
}

impl RequestIntent {
    fn from_request(request: &str) -> Self {
        let lower = request.to_lowercase();
        let framework = if lower.contains("next.js") || lower.contains("nextjs") {
            FrameworkKind::Next
        } else if lower.contains("nuxt.js") || lower.contains("nuxt") {
            FrameworkKind::Nuxt
        } else if lower.contains("react.js") || lower.contains("react") {
            FrameworkKind::React
        } else if lower.contains("sveltekit")
            || lower.contains("svelte kit")
            || lower.contains("svelte")
        {
            FrameworkKind::SvelteKit
        } else {
            FrameworkKind::Unknown
        };
        let game_experience = lower.contains("game")
            || request.contains("ゲーム")
            || request_matches_any(&lower, request, BREAKOUT_GAME_KEYWORDS)
            || request_matches_any(&lower, request, FALLING_BLOCK_GAME_KEYWORDS);
        let interactive_experience = game_experience
            || lower.contains("playable")
            || lower.contains("interactive")
            || request_matches_any(&lower, request, INTERACTIVE_UI_KEYWORDS)
            || request.contains("プレイ")
            || request.contains("操作")
            || request.contains("反応");
        let explicit_create = lower.contains("create")
            || lower.contains("build")
            || lower.contains("develop")
            || lower.contains("implement")
            || lower.contains("scaffold")
            || request.contains("作って")
            || request.contains("作成")
            || request.contains("開発")
            || request.contains("実装")
            || request.contains("生成");
        let improvement_intent = lower.contains("improve")
            || lower.contains("polish")
            || lower.contains("enhance")
            || request.contains("改善")
            || request.contains("品質")
            || request.contains("上げ")
            || request.contains("既存")
            || (request.contains("つ目") && request.contains("追加"))
            || request.contains("より")
            || request.contains("カッコ")
            || request.contains("かっこ");
        let operation = if improvement_intent && !explicit_create {
            RequestOperation::Improve
        } else {
            RequestOperation::Create
        };
        let asks_for_app_or_ui = lower.contains("app")
            || lower.contains("ui")
            || lower.contains("next.js")
            || lower.contains("nextjs")
            || lower.contains("react")
            || lower.contains("nuxt")
            || lower.contains("svelte")
            || request.contains("アプリ")
            || request.contains("起動");
        Self {
            framework,
            interactive_experience,
            game_experience,
            operation,
            asks_for_app_or_ui,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FeatureProfile {
    form: bool,
    list: bool,
    validation: bool,
    calculation: bool,
    visualization: bool,
    persistence: bool,
    accessibility: bool,
    test_support: bool,
}

impl FeatureProfile {
    fn from_request(request: &str) -> Self {
        let lower = request.to_lowercase();
        Self {
            form: request_matches_any(&lower, request, FORM_FEATURE_KEYWORDS),
            list: request_matches_any(&lower, request, LIST_FEATURE_KEYWORDS),
            validation: request_matches_any(&lower, request, VALIDATION_FEATURE_KEYWORDS),
            calculation: request_matches_any(&lower, request, CALCULATION_FEATURE_KEYWORDS),
            visualization: request_matches_any(&lower, request, VISUALIZATION_FEATURE_KEYWORDS),
            persistence: request_matches_any(&lower, request, PERSISTENCE_FEATURE_KEYWORDS),
            accessibility: request_matches_any(&lower, request, ACCESSIBILITY_FEATURE_KEYWORDS),
            test_support: request_matches_any(&lower, request, TEST_FEATURE_KEYWORDS),
        }
    }

    fn requires_semantic_business_slice(self) -> bool {
        self.validation || self.calculation || self.visualization || self.persistence
    }

    fn requires_strict_semantic_quality(self, request_lower: &str) -> bool {
        self.validation
            || self.calculation
            || self.visualization
            || (self.persistence && !request_lower.contains("browser"))
    }
}

pub(super) fn request_needs_playable_ui_quality_gate(request: &str) -> bool {
    let intent = RequestIntent::from_request(request);
    request_targets_playable_ui_or_app(request, intent)
}

pub(super) fn request_is_playable_ui_improvement(request: &str) -> bool {
    let intent = RequestIntent::from_request(request);
    intent.operation == RequestOperation::Improve
        && request_targets_playable_ui_or_app(request, intent)
}

pub(super) fn request_allows_fast_polish_fallback(request: &str) -> bool {
    if !request_is_playable_ui_improvement(request) {
        return false;
    }
    let lower = request.to_lowercase();
    request_matches_any(&lower, request, SAFE_POLISH_FALLBACK_KEYWORDS)
}

fn request_targets_playable_ui_or_app(request: &str, intent: RequestIntent) -> bool {
    let lower = request.to_lowercase();
    if intent.framework != FrameworkKind::Unknown
        || intent.interactive_experience
        || intent.game_experience
    {
        return !request_matches_any(&lower, request, NON_UI_CODING_KEYWORDS);
    }
    let profile = FeatureProfile::from_request(request);
    let has_ui_features = profile.form
        || profile.list
        || profile.requires_semantic_business_slice()
        || profile.accessibility
        || profile.test_support;
    if intent.asks_for_app_or_ui {
        return has_ui_features || intent.operation == RequestOperation::Improve;
    }
    intent.operation == RequestOperation::Improve
        && has_ui_features
        && !request_matches_any(&lower, request, NON_UI_CODING_KEYWORDS)
}

pub(super) fn repo_change_request_text(
    active_task: Option<&str>,
    messages: &[ConversationMessage],
) -> Option<String> {
    active_task
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .and_then(extract_original_user_request)
        .or_else(|| {
            active_task
                .map(str::trim)
                .filter(|task| !task.is_empty())
                .filter(|task| !is_plan_wrapper_or_approval_text(task))
                .map(ToString::to_string)
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .filter(|message| message.role == "user")
                .filter_map(|message| extract_original_user_request(&message.content))
                .next()
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .filter(|message| message.role == "user")
                .filter(|message| !is_plan_wrapper_or_approval_text(&message.content))
                .find(|message| message.role == "user")
                .map(|message| message.content.trim().to_string())
                .filter(|content| !content.is_empty())
        })
}

fn extract_original_user_request(text: &str) -> Option<String> {
    let marker = "User request:";
    let (_, rest) = text.split_once(marker)?;
    let request = rest.trim();
    if request.is_empty() {
        return None;
    }
    Some(request.to_string())
}

fn is_plan_wrapper_or_approval_text(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "yes" | "y" | "no" | "n" | "approve" | "/approve"
    ) || lower.starts_with("the user approved the plan")
        || lower.starts_with("create an implementation plan")
}

// ---------------------------------------------------------------------------
// Issue #580: first-pass observation types (SSoT for quality-gate analysis)
// ---------------------------------------------------------------------------

/// Reason why the quality gate's 5-step first-pass fast-fail short-circuited
/// without reaching the count-evidence verdict. These are *deterministic*
/// failures that the second-pass LLM should NOT be allowed to override
/// (DR1-005). Per `QualityFirstPassObservation` semantics they are mutually
/// exclusive with `ConfirmationEligible`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityEarlyFailReason {
    /// (1) `looks_like_ui_marker_spam` detected superficial marker spam.
    UiMarkerSpam,
    /// (2) `placeholder_hits >= 2` — scaffold / placeholder markers persist.
    Placeholder,
    /// (4) `requires_strict_semantic_quality` and `semantic_hits < 5`.
    StrictSemantic,
    /// (5) Game request whose body is a low-fidelity slice.
    LowFidelityGame,
}

impl QualityEarlyFailReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UiMarkerSpam => "ui_marker_spam",
            Self::Placeholder => "placeholder",
            Self::StrictSemantic => "strict_semantic",
            Self::LowFidelityGame => "low_fidelity_game",
        }
    }
}

/// Whether second-pass confirmation is allowed for this first-pass outcome.
///
/// * `ConfirmationEligible` — adapter MAY consult the sidecar LLM (the
///   final go/no-go also depends on `should_request_quality_confirmation`
///   which inspects the count tuple).
/// * `EarlyFail { reason }` — quality.rs already decided this is not
///   interactive UI for a deterministic reason; second-pass must NOT be
///   invoked (DR1-005 / S7-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityFirstPassGate {
    ConfirmationEligible,
    EarlyFail { reason: QualityEarlyFailReason },
}

impl QualityFirstPassGate {
    pub fn confirmation_eligible(&self) -> bool {
        matches!(self, Self::ConfirmationEligible)
    }
}

/// Aggregated first-pass observation. `issue` retains the original wrapper
/// semantics (returned by `implementation_quality_issue_for_request`), while
/// `gate` is the independent eligibility signal for `quality_confirm.rs`.
///
/// DR2-002: the two are intentionally orthogonal — Type-B rescue
/// (state_hits=0 but other categories hit) produces
/// `issue = Some(_), gate = ConfirmationEligible` so the adapter can flip
/// `issue` to `None` after LLM confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityFirstPassObservation {
    pub issue: Option<String>,
    pub interaction_hits: usize,
    pub state_hits: usize,
    pub feedback_hits: usize,
    pub gate: QualityFirstPassGate,
}

/// Issue #580 / DR1-010 SSoT: full 5-step first-pass analysis of a generated
/// UI implementation. `implementation_quality_issue_for_request` is now a
/// thin wrapper around this function — they share the same logic exactly
/// once.
pub fn quality_first_pass_observation(request: &str, content: &str) -> QualityFirstPassObservation {
    let intent = RequestIntent::from_request(request);
    let profile = FeatureProfile::from_request(request);
    let request_lower = request.to_lowercase();
    let normalized = content.to_lowercase();

    // (1) UI marker spam (early fail).
    if looks_like_ui_marker_spam(&normalized) {
        return QualityFirstPassObservation {
            issue: Some(
                "it contains superficial UI quality marker spam without executable interaction, state, and feedback evidence"
                    .to_string(),
            ),
            interaction_hits: count_ui_interaction_hits(&normalized),
            state_hits: count_ui_state_hits(&normalized),
            feedback_hits: count_ui_feedback_hits(&normalized),
            gate: QualityFirstPassGate::EarlyFail {
                reason: QualityEarlyFailReason::UiMarkerSpam,
            },
        };
    }

    // (2) Placeholder hits (early fail).
    let placeholder_hits = placeholder_hit_count(intent.framework, &normalized);
    if placeholder_hits >= 2 {
        return QualityFirstPassObservation {
            issue: Some(
                "it still contains multiple scaffold or generic placeholder markers".to_string(),
            ),
            interaction_hits: count_ui_interaction_hits(&normalized),
            state_hits: count_ui_state_hits(&normalized),
            feedback_hits: count_ui_feedback_hits(&normalized),
            gate: QualityFirstPassGate::EarlyFail {
                reason: QualityEarlyFailReason::Placeholder,
            },
        };
    }

    // (3) Count-evidence — second-pass eligible (Type-B rescue lane).
    let interaction_hits = count_ui_interaction_hits(&normalized);
    let state_hits = count_ui_state_hits(&normalized);
    let feedback_hits = count_ui_feedback_hits(&normalized);
    if interaction_hits == 0 || state_hits == 0 || feedback_hits == 0 {
        let issue = Some(format!(
            "it lacks an interactive vertical slice; expected executable input handling, state, and visible feedback evidence (input={interaction_hits}, state={state_hits}, feedback={feedback_hits})"
        ));
        // Confirmation eligible: at least one category may still be Some — the
        // adapter consults `should_request_quality_confirmation` for the final
        // gate (all_zero → skip, all_strong is impossible here, otherwise call LLM).
        return QualityFirstPassObservation {
            issue,
            interaction_hits,
            state_hits,
            feedback_hits,
            gate: QualityFirstPassGate::ConfirmationEligible,
        };
    }

    // (4) Strict semantic quality (early fail).
    if profile.requires_strict_semantic_quality(&request_lower) {
        let semantic_hits = count_any(
            &normalized,
            &[
                "calculated total",
                "projectedtotal",
                "target",
                "targetmet",
                "math.max",
                "math.min",
                "width:",
                "aria-live",
                "localstorage",
                "role=\"alert\"",
            ],
        );
        if semantic_hits < 5 {
            return QualityFirstPassObservation {
                issue: Some(format!(
                    "it lacks requested semantic business primitives; expected validation, calculation, visualization, persistence, and accessible feedback markers (semantic_hits={semantic_hits})"
                )),
                interaction_hits,
                state_hits,
                feedback_hits,
                gate: QualityFirstPassGate::EarlyFail {
                    reason: QualityEarlyFailReason::StrictSemantic,
                },
            };
        }
    }

    // (5) Low-fidelity game slice (early fail).
    if intent.game_experience && looks_like_low_fidelity_game_slice(&normalized) {
        return QualityFirstPassObservation {
            issue: Some(
                "it is a low-fidelity game slice; expected a real-time render loop with canvas, keyboard input, and visible restart/status feedback"
                    .to_string(),
            ),
            interaction_hits,
            state_hits,
            feedback_hits,
            gate: QualityFirstPassGate::EarlyFail {
                reason: QualityEarlyFailReason::LowFidelityGame,
            },
        };
    }

    // First-pass pass — second-pass eligible (Type-A rescue lane: LLM may
    // override None → Some if counts look strong but UI is actually static).
    QualityFirstPassObservation {
        issue: None,
        interaction_hits,
        state_hits,
        feedback_hits,
        gate: QualityFirstPassGate::ConfirmationEligible,
    }
}

pub(super) fn implementation_quality_issue_for_request(
    request: &str,
    content: &str,
) -> Option<String> {
    quality_first_pass_observation(request, content).issue
}

pub(super) fn deterministic_playable_ui_fallback(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    let intent = RequestIntent::from_request(request);
    if !intent.asks_for_app_or_ui {
        return None;
    }
    let normalized = current_content.to_lowercase();
    let placeholder_hits = placeholder_hit_count(intent.framework, &normalized);
    if placeholder_hits == 0
        && !looks_like_incomplete_playable_slice(&normalized)
        && !(intent.game_experience && looks_like_low_fidelity_game_slice(&normalized))
    {
        return None;
    }

    let path = target_path.to_string_lossy().to_ascii_lowercase();
    if path.ends_with(".vue") {
        return if intent.game_experience {
            Some(vue_canvas_game_template(GameKind::from_request(request)))
        } else {
            Some(vue_business_app_template(request))
        };
    }
    if path.ends_with(".svelte") {
        return if intent.game_experience {
            Some(svelte_canvas_game_template(GameKind::from_request(request)))
        } else {
            Some(svelte_business_app_template(request))
        };
    }
    if path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".ts")
        || path.ends_with(".js")
    {
        return if intent.game_experience {
            Some(react_canvas_game_template(GameKind::from_request(request)))
        } else {
            Some(react_business_app_template(request))
        };
    }
    None
}

pub(super) fn deterministic_playable_ui_polish_fallback(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    if !request_is_playable_ui_improvement(request)
        || current_content.contains("data-anvil-polish=\"v1\"")
    {
        return None;
    }

    let path = target_path.to_string_lossy().to_ascii_lowercase();
    if path.ends_with(".vue") {
        if request_needs_business_data_improvement(request)
            && let Some(updated) = polish_vue_business_app(current_content)
        {
            return Some(updated);
        }
        return polish_vue_playable_ui(current_content);
    }
    if path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".ts")
        || path.ends_with(".js")
    {
        if request_needs_business_data_improvement(request)
            && let Some(updated) = polish_react_business_app(current_content)
        {
            return Some(updated);
        }
        return polish_react_playable_ui(current_content);
    }
    None
}

pub(super) fn deterministic_empty_framework_game_files(
    request: &str,
) -> Option<Vec<(PathBuf, String)>> {
    let intent = RequestIntent::from_request(request);
    if !intent.game_experience || !intent.asks_for_app_or_ui {
        return None;
    }

    let port = requested_port(request).unwrap_or(3011);
    let game = GameKind::from_request(request);
    match intent.framework {
        FrameworkKind::Next => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "next dev -p {port}",
    "build": "next build",
    "start": "next start",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "next": "{NEXT_VERSION}",
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}",
    "@types/node": "{NODE_TYPES_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("src/app/layout.tsx"),
                "import type { ReactNode } from \"react\";\n\nexport default function RootLayout({ children }: { children: ReactNode }) {\n  return <html lang=\"en\"><body>{children}</body></html>;\n}\n".to_string(),
            ),
            (
                PathBuf::from("src/app/page.tsx"),
                react_canvas_game_template(game),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
        ]),
        FrameworkKind::React => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "node scripts/dev.mjs",
    "build": "vite build",
    "preview": "vite preview --host 0.0.0.0 --port {port}",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}",
    "@vitejs/plugin-react": "{VITE_REACT_PLUGIN_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}",
    "vite": "{VITE_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("scripts/dev.mjs"),
                vite_dev_wrapper_script(port),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
            (
                PathBuf::from("tsconfig.json"),
                react_vite_tsconfig().to_string(),
            ),
            (
                PathBuf::from("vite.config.ts"),
                react_vite_config().to_string(),
            ),
            (
                PathBuf::from("index.html"),
                "<div id=\"root\"></div><script type=\"module\" src=\"/src/main.tsx\"></script>\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/main.tsx"),
                "import React from 'react';\nimport { createRoot } from 'react-dom/client';\nimport App from './App';\n\ncreateRoot(document.getElementById('root')!).render(<App />);\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/App.tsx"),
                react_canvas_game_template(game),
            ),
        ]),
        FrameworkKind::Nuxt => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "nuxt dev -p {port}",
    "build": "nuxt build",
    "preview": "nuxt preview",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "nuxt": "{NUXT_VERSION}",
    "vue": "{VUE_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("nuxt.config.ts"),
                "export default defineNuxtConfig({ ssr: false });\n".to_string(),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
            (PathBuf::from("app.vue"), vue_canvas_game_template(game)),
        ]),
        FrameworkKind::SvelteKit => Some(vec![
            (PathBuf::from("package.json"), sveltekit_package_json(port)),
            (
                PathBuf::from("svelte.config.js"),
                "import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';\n\nexport default { preprocess: vitePreprocess() };\n".to_string(),
            ),
            (
                PathBuf::from("vite.config.ts"),
                sveltekit_vite_config().to_string(),
            ),
            (
                PathBuf::from("src/routes/+layout.svelte"),
                "<slot />\n".to_string(),
            ),
            (
                PathBuf::from("src/routes/+page.svelte"),
                svelte_canvas_game_template(game),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
        ]),
        FrameworkKind::Unknown => None,
    }
}

pub(super) fn deterministic_empty_framework_app_files(
    request: &str,
) -> Option<Vec<(PathBuf, String)>> {
    let intent = RequestIntent::from_request(request);
    if intent.game_experience {
        return deterministic_empty_framework_game_files(request);
    }
    if !intent.asks_for_app_or_ui {
        return None;
    }

    let port = requested_port(request).unwrap_or(3011);
    match intent.framework {
        FrameworkKind::Next => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "next dev -p {port}",
    "build": "next build",
    "start": "next start",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "next": "{NEXT_VERSION}",
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}",
    "@types/node": "{NODE_TYPES_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("src/app/layout.tsx"),
                "import type { ReactNode } from \"react\";\n\nexport default function RootLayout({ children }: { children: ReactNode }) {\n  return <html lang=\"en\"><body>{children}</body></html>;\n}\n".to_string(),
            ),
            (
                PathBuf::from("src/app/page.tsx"),
                react_business_app_template(request),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
        ]),
        FrameworkKind::React => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "node scripts/dev.mjs",
    "build": "vite build",
    "preview": "vite preview --host 0.0.0.0 --port {port}",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}",
    "@vitejs/plugin-react": "{VITE_REACT_PLUGIN_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}",
    "vite": "{VITE_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("scripts/dev.mjs"),
                vite_dev_wrapper_script(port),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
            (
                PathBuf::from("tsconfig.json"),
                react_vite_tsconfig().to_string(),
            ),
            (
                PathBuf::from("vite.config.ts"),
                react_vite_config().to_string(),
            ),
            (
                PathBuf::from("index.html"),
                "<div id=\"root\"></div><script type=\"module\" src=\"/src/main.tsx\"></script>\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/main.tsx"),
                "import React from 'react';\nimport { createRoot } from 'react-dom/client';\nimport App from './App';\n\ncreateRoot(document.getElementById('root')!).render(<App />);\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/App.tsx"),
                react_business_app_template(request),
            ),
        ]),
        FrameworkKind::Nuxt => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "nuxt dev -p {port}",
    "build": "nuxt build",
    "preview": "nuxt preview",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "nuxt": "{NUXT_VERSION}",
    "vue": "{VUE_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("nuxt.config.ts"),
                "export default defineNuxtConfig({ ssr: false });\n".to_string(),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
            (PathBuf::from("app.vue"), vue_business_app_template(request)),
        ]),
        FrameworkKind::SvelteKit => Some(vec![
            (PathBuf::from("package.json"), sveltekit_package_json(port)),
            (
                PathBuf::from("svelte.config.js"),
                "import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';\n\nexport default { preprocess: vitePreprocess() };\n".to_string(),
            ),
            (
                PathBuf::from("vite.config.ts"),
                sveltekit_vite_config().to_string(),
            ),
            (
                PathBuf::from("src/routes/+layout.svelte"),
                "<slot />\n".to_string(),
            ),
            (
                PathBuf::from("src/routes/+page.svelte"),
                svelte_business_app_template(request),
            ),
            (
                PathBuf::from("scripts/smoke-test.mjs"),
                smoke_test_script().to_string(),
            ),
        ]),
        FrameworkKind::Unknown => None,
    }
}

pub(super) fn deterministic_empty_python_cli_files(
    request: &str,
) -> Option<Vec<(PathBuf, String)>> {
    deterministic_empty_python_cli_files_with_names(request, None, None)
}

pub(super) fn deterministic_empty_python_cli_files_with_names(
    request: &str,
    script_name: Option<&str>,
    sample_name: Option<&str>,
) -> Option<Vec<(PathBuf, String)>> {
    let lower = request.to_ascii_lowercase();
    let asks_python =
        lower.contains("python") || lower.contains("python3") || lower.contains(".py");
    let asks_cli = lower.contains("cli")
        || lower.contains("command")
        || lower.contains("terminal")
        || lower.contains("コマンド")
        || lower.contains("集計");
    let asks_csv = lower.contains("csv");
    let disallowed_ui = [
        "next.js",
        "nextjs",
        "react",
        "nuxt",
        "vue",
        "vite",
        "typescript",
        "javascript",
        "web app",
        "ui",
        "画面",
        "アプリ",
    ];
    if !asks_python || !asks_cli || !asks_csv || disallowed_ui.iter().any(|kw| lower.contains(kw)) {
        return None;
    }
    let script_name = script_name
        .and_then(|name| safe_generated_filename(name, ".py"))
        .unwrap_or_else(|| "analyze_csv.py".to_string());
    let sample_name = sample_name
        .and_then(|name| safe_generated_filename(name, ".csv"))
        .unwrap_or_else(|| "sample.csv".to_string());

    Some(vec![
        (
            PathBuf::from(&script_name),
            r#"#!/usr/bin/env python3
import argparse
import csv
from collections import defaultdict
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser(description="Summarize Amount totals from a CSV file.")
    parser.add_argument("csv_path", type=Path, help="CSV file with Category and Amount columns")
    parser.add_argument("--category", default="Category", help="category column name")
    parser.add_argument("--amount", default="Amount", help="amount column name")
    return parser.parse_args()


def main():
    args = parse_args()
    totals = defaultdict(float)
    with args.csv_path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        missing = {args.category, args.amount} - set(reader.fieldnames or [])
        if missing:
            raise SystemExit(f"missing required column(s): {', '.join(sorted(missing))}")
        for row in reader:
            category = (row.get(args.category) or "Uncategorized").strip() or "Uncategorized"
            raw_amount = (row.get(args.amount) or "0").replace(",", "").strip()
            totals[category] += float(raw_amount)

    grand_total = sum(totals.values())
    print("Category,Total")
    for category, total in sorted(totals.items()):
        print(f"{category},{total:.2f}")
    print(f"Grand Total,{grand_total:.2f}")


if __name__ == "__main__":
    main()
"#
            .to_string(),
        ),
        (
            PathBuf::from(&sample_name),
            "Category,Amount\nFood,1200\nTransport,450\nFood,800\nBooks,2500\n".to_string(),
        ),
        (
            PathBuf::from("README.md"),
            format!(
                r#"# CSV Summary CLI

Run:

```bash
python3 {script_name} {sample_name}
```

The input CSV must include `Category` and `Amount` columns. Use `--category` and `--amount` to point the CLI at different column names.
"#
            ),
        ),
    ])
}

fn safe_generated_filename(candidate: &str, required_suffix: &str) -> Option<String> {
    let trimmed = candidate
        .trim()
        .trim_matches(['`', '"', '\'', '。', '、', ',', '.']);
    if trimmed.len() > 80
        || !trimmed.ends_with(required_suffix)
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.starts_with('.')
    {
        return None;
    }
    let valid = trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    valid.then(|| trimmed.to_string())
}

pub(super) fn deterministic_empty_docs_files(request: &str) -> Option<Vec<(PathBuf, String)>> {
    let lower = request.to_ascii_lowercase();
    let asks_docs = lower.contains("readme")
        || lower.contains("markdown")
        || lower.contains("docs")
        || lower.contains("documentation")
        || lower.contains("ドキュメント")
        || lower.contains("設計書")
        || lower.contains("仕様書");
    let asks_create_or_update = lower.contains("create")
        || lower.contains("write")
        || lower.contains("update")
        || lower.contains("作成")
        || lower.contains("更新")
        || lower.contains("書いて");
    let answer_only = lower.contains("変更しない")
        || lower.contains("編集しない")
        || lower.contains("do not modify")
        || lower.contains("no file changes");
    if !asks_docs || !asks_create_or_update || answer_only {
        return None;
    }

    Some(vec![(
        PathBuf::from("README.md"),
        r#"# Project Overview

## Purpose

This repository contains the requested local project artifacts.

## Usage

Document the primary command or workflow here after implementation details are available.

## Verification

Record the commands used to validate behavior and quality.

## Notes

Keep this document focused on concrete project behavior, setup steps, and maintenance guidance.
"#
        .to_string(),
    )])
}

pub(super) fn request_mentions_unsupported_ui_framework(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    [
        "astro", "solid", "solidjs", "solid.js", "qwik", "ember", "remix",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}

pub(super) fn workspace_has_unsupported_ui_framework(work_root: &Path) -> bool {
    scaffold_search_roots(work_root).into_iter().any(|root| {
        [
            "svelte.config.js",
            "svelte.config.ts",
            "astro.config.mjs",
            "astro.config.ts",
            "solid.config.ts",
            "solid.config.js",
        ]
        .iter()
        .any(|relative| root.join(relative).is_file())
    })
}

pub(super) fn request_explicitly_requires_tests(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    lower.contains("test")
        || lower.contains("pytest")
        || lower.contains("unittest")
        || request.contains("テスト")
        || request.contains("検証")
}

pub(super) fn package_json_with_requested_port(
    request: &str,
    package_content: &str,
) -> Option<String> {
    let port = requested_port(request)?;
    let mut package: serde_json::Value = serde_json::from_str(package_content).ok()?;
    let framework = package_framework(&package)?;
    let scripts = package
        .as_object_mut()?
        .entry("scripts")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()?;
    let dev = match framework {
        FrameworkKind::Next => format!("next dev -p {port}"),
        FrameworkKind::React => "node scripts/dev.mjs".to_string(),
        FrameworkKind::Nuxt => format!("nuxt dev -p {port}"),
        FrameworkKind::SvelteKit => format!("vite --host 0.0.0.0 --port {port}"),
        FrameworkKind::Unknown => return None,
    };
    if scripts.get("dev").and_then(|value| value.as_str()) == Some(dev.as_str()) {
        return None;
    }
    scripts.insert("dev".to_string(), serde_json::Value::String(dev));
    serde_json::to_string_pretty(&package)
        .ok()
        .map(|json| format!("{json}\n"))
}

pub(super) fn react_dev_wrapper_for_requested_port(
    request: &str,
    package_content: &str,
) -> Option<String> {
    let port = requested_port(request)?;
    let package: serde_json::Value = serde_json::from_str(package_content).ok()?;
    (package_framework(&package)? == FrameworkKind::React).then(|| vite_dev_wrapper_script(port))
}

fn vite_dev_wrapper_script(default_port: u16) -> String {
    format!(
        r#"import {{ spawn }} from "node:child_process";

const args = process.argv.slice(2);
const viteArgs = ["--host", "0.0.0.0"];
let hasPort = false;

for (let index = 0; index < args.length; index += 1) {{
  const arg = args[index];
  if (arg === "-p" && args[index + 1]) {{
    viteArgs.push("--port", args[index + 1]);
    hasPort = true;
    index += 1;
  }} else {{
    if (arg === "--port" || arg.startsWith("--port=")) {{
      hasPort = true;
    }}
    viteArgs.push(arg);
  }}
}}

if (!hasPort) {{
  viteArgs.push("--port", "{default_port}");
}}

const child = spawn("vite", viteArgs, {{
  stdio: "inherit",
  shell: process.platform === "win32",
}});

child.on("exit", (code) => process.exit(code ?? 0));
child.on("error", (error) => {{
  console.error(error);
  process.exit(1);
}});
"#
    )
}

fn smoke_test_script() -> &'static str {
    r#"import { existsSync, readFileSync } from 'node:fs';

const candidates = ['src/App.tsx', 'src/app/page.tsx', 'app/page.tsx', 'app.vue', 'src/routes/+page.svelte'];
const target = candidates.find((path) => existsSync(path));
if (!target) {
  console.error('No application entry file found.');
  process.exit(1);
}

const source = readFileSync(target, 'utf8');
const isGame = source.includes('canvas') || source.includes('requestAnimationFrame');
const required = isGame
  ? ['canvas', 'requestAnimationFrame', 'addEventListener']
  : ['role="alert"', 'localStorage', 'Score history', 'Calculated total', 'Target', 'aria-live', 'Math.max', 'Math.min'];
const missing = required.filter((token) => !source.includes(token));
if (isGame && !source.includes('<button') && !source.includes('@click')) {
  missing.push('restart control');
}
if (missing.length > 0) {
  console.error(`Missing expected ${isGame ? 'game' : 'app'} quality markers: ${missing.join(', ')}`);
  process.exit(1);
}

console.log(`smoke ok: ${target}; quality layers ok: L1 structure, L2 runnable scripts, L3 interaction, L4 functional primitives`);
"#
}

fn react_vite_tsconfig() -> &'static str {
    r#"{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["DOM", "DOM.Iterable", "ES2020"],
    "allowJs": false,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "allowSyntheticDefaultImports": true,
    "strict": true,
    "forceConsistentCasingInFileNames": true,
    "module": "ESNext",
    "moduleResolution": "Node",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx"
  },
  "include": ["src"],
  "references": []
}
"#
}

fn react_vite_config() -> &'static str {
    "import { defineConfig } from 'vite';\nimport react from '@vitejs/plugin-react';\n\nexport default defineConfig({ plugins: [react()] });\n"
}

fn sveltekit_package_json(port: u16) -> String {
    format!(
        r#"{{
  "scripts": {{
    "dev": "vite --host 0.0.0.0 --port {port}",
    "build": "vite build",
    "preview": "vite preview --host 0.0.0.0 --port {port}",
    "test": "node scripts/smoke-test.mjs",
    "audit": "npm audit --audit-level=critical"
  }},
  "dependencies": {{
    "@sveltejs/kit": "{SVELTEKIT_VERSION}",
    "@sveltejs/vite-plugin-svelte": "{SVELTE_VITE_PLUGIN_VERSION}",
    "svelte": "{SVELTE_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}",
    "vite": "{VITE_VERSION}"
  }},
  "devDependencies": {{}}
}}
"#
    )
}

fn sveltekit_vite_config() -> &'static str {
    "import { sveltekit } from '@sveltejs/kit/vite';\nimport { defineConfig } from 'vite';\n\nexport default defineConfig({ plugins: [sveltekit()] });\n"
}

fn package_framework(package: &serde_json::Value) -> Option<FrameworkKind> {
    let has_dep = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .filter_map(|section| package.get(*section))
            .filter_map(|section| section.as_object())
            .any(|deps| deps.contains_key(name))
    };
    if has_dep("next") {
        Some(FrameworkKind::Next)
    } else if has_dep("nuxt") {
        Some(FrameworkKind::Nuxt)
    } else if has_dep("@sveltejs/kit") || has_dep("svelte") {
        Some(FrameworkKind::SvelteKit)
    } else if has_dep("vite") || has_dep("@vitejs/plugin-react") || has_dep("react") {
        Some(FrameworkKind::React)
    } else {
        None
    }
}

fn requested_port(request: &str) -> Option<u16> {
    let chars: Vec<(usize, char)> = request.char_indices().collect();
    let mut index = 0;
    while index < chars.len() {
        let (start_byte, ch) = chars[index];
        if !ch.is_ascii_digit() {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && chars[end].1.is_ascii_digit() {
            end += 1;
        }
        let end_byte = chars
            .get(end)
            .map(|(byte, _)| *byte)
            .unwrap_or_else(|| request.len());
        let candidate = &request[start_byte..end_byte];
        let value = candidate.parse::<u16>().ok();
        if !(2..=5).contains(&candidate.len()) {
            index = end;
            continue;
        }
        let prefix_start = chars
            .get(index.saturating_sub(10))
            .map(|(byte, _)| *byte)
            .unwrap_or(0);
        let suffix_end = chars
            .get((end + 8).min(chars.len()))
            .map(|(byte, _)| *byte)
            .unwrap_or_else(|| request.len());
        let prefix = request[prefix_start..start_byte].to_ascii_lowercase();
        let suffix = request[end_byte..suffix_end].to_ascii_lowercase();
        if prefix.contains("port")
            || suffix.contains("port")
            || request[prefix_start..start_byte].contains("ポート")
            || request[end_byte..suffix_end].contains("ポート")
        {
            return value;
        }
        index = end;
    }
    None
}

fn looks_like_incomplete_playable_slice(normalized_content: &str) -> bool {
    let compact_len = normalized_content.trim().chars().count();
    if compact_len == 0 {
        return true;
    }
    if compact_len > 1_500 {
        return false;
    }

    let has_interaction = count_any(
        normalized_content,
        &[
            "onclick",
            "onkeydown",
            "addeventlistener",
            "button",
            "canvas",
            "requestanimationframe",
            "usestate",
            "ref(",
            "reactive",
        ],
    ) > 0;
    if has_interaction {
        return false;
    }

    count_any(
        normalized_content,
        &[
            "<template",
            "<script",
            "export default",
            "function app",
            "function page",
            "const app",
            "const page",
            "import ",
            "component",
        ],
    ) > 0
}

fn count_ui_interaction_hits(normalized_content: &str) -> usize {
    count_any(
        normalized_content,
        &[
            "onclick=",
            "onclick={",
            "onchange=",
            "onchange={",
            "oninput=",
            "oninput={",
            "onsubmit=",
            "onsubmit={",
            "onkeydown=",
            "onkeydown={",
            "onkeyup=",
            "onpointer",
            "onmouse",
            "@click=",
            "@input=",
            "@submit=",
            "on:click=",
            "on:input=",
            "addeventlistener(",
            "addeventlistener(\"",
            "addeventlistener('",
            "<button",
            "<input",
            "<select",
            "<textarea",
            "<form",
            "<canvas",
        ],
    )
}

fn count_ui_state_hits(normalized_content: &str) -> usize {
    count_any(
        normalized_content,
        &[
            "usestate(",
            "usereducer(",
            "useref(",
            "computed(",
            "reactive(",
            "ref(",
            "$state(",
            "let ",
            "const ",
            "var ",
            "data()",
            "localstorage",
            "sessionstorage",
            ".dataset",
            ".value",
            "value=",
            "name=",
            "required",
            "checked",
            "selected",
            "score",
            "status",
            "progress",
            "current",
            "active",
        ],
    )
}

fn count_ui_feedback_hits(normalized_content: &str) -> usize {
    count_any(
        normalized_content,
        &[
            "textcontent",
            "innerhtml",
            "insertadjacenthtml",
            "setattribute(",
            ".classlist",
            ".style",
            "aria-live",
            "role=",
            "<output",
            "<label",
            "disabled",
            "class=",
            "classname",
            "style=",
            "result",
            "message",
            "status",
            "progress",
            "error",
            "success",
            "score",
        ],
    )
}

fn count_ui_runtime_evidence(normalized_content: &str) -> usize {
    count_any(
        normalized_content,
        &[
            "=>",
            "function ",
            "addEventListener(",
            "addeventlistener(",
            "document.queryselector",
            "document.getelementbyid",
            "setstate",
            "setstatus",
            "setprogress",
            "setscore",
            ".textcontent =",
            ".innerhtml =",
            ".value =",
            ".classlist.",
            ".style.",
            "requestanimationframe(",
            "<script",
            "<form",
        ],
    )
}

fn looks_like_ui_marker_spam(normalized_content: &str) -> bool {
    let marker_words = count_any(
        normalized_content,
        &[
            "input handling",
            "visible feedback",
            "feedback markers",
            "state markers",
            "interactive vertical slice",
            "requestanimationframe",
            "addeventlistener",
            "onclick",
            "state",
            "status",
            "progress",
            "canvas",
        ],
    );
    if marker_words < 4 {
        return false;
    }
    count_ui_interaction_hits(normalized_content) == 0
        && count_ui_runtime_evidence(normalized_content) == 0
}

fn looks_like_low_fidelity_game_slice(normalized_content: &str) -> bool {
    let has_canvas = normalized_content.contains("<canvas")
        || normalized_content.contains("canvasref")
        || normalized_content.contains("getcontext(\"2d\")")
        || normalized_content.contains("getcontext('2d')");
    let has_animation_loop = normalized_content.contains("requestanimationframe(");
    let has_direct_input = count_any(
        normalized_content,
        &[
            "addeventlistener(\"keydown\"",
            "addeventlistener('keydown'",
            "onkeydown",
            "onpointer",
            "pointermove",
            "pointerdown",
        ],
    ) > 0;
    let has_recovery_or_status = count_any(
        normalized_content,
        &[
            "restart",
            "start",
            "score",
            "lives",
            "level",
            "status",
            "game over",
        ],
    ) > 0;

    !(has_canvas && has_animation_loop && has_direct_input && has_recovery_or_status)
}

fn polish_react_playable_ui(current_content: &str) -> Option<String> {
    let mut updated = current_content.to_string();
    let mut changed = false;

    let main = "<main className=\"min-h-screen bg-[#050711] px-5 py-6 text-white\">";
    if updated.contains(main) {
        updated = updated.replacen(
            main,
            "<main data-anvil-polish=\"v1\" className=\"relative min-h-screen overflow-hidden bg-[#050711] px-5 py-6 text-white\">",
            1,
        );
        let overlay = "      <div aria-hidden=\"true\" className=\"pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_18%_12%,rgba(34,211,238,0.24),transparent_28%),radial-gradient(circle_at_82%_18%,rgba(244,114,182,0.18),transparent_30%),linear-gradient(135deg,rgba(250,204,21,0.07),transparent_42%)]\" />\n      <div aria-hidden=\"true\" className=\"pointer-events-none absolute inset-x-0 top-0 h-px bg-cyan-200/70 shadow-[0_0_30px_rgba(103,232,249,0.9)]\" />\n";
        updated = updated.replacen(
            "<section className=\"mx-auto flex max-w-6xl flex-col gap-4\">",
            &format!("{overlay}      <section className=\"relative mx-auto flex max-w-6xl flex-col gap-4\">"),
            1,
        );
        changed = true;
    } else if updated.contains("<main ") {
        updated = updated.replacen("<main ", "<main data-anvil-polish=\"v1\" ", 1);
        changed = true;
    }

    let header = "flex flex-wrap items-end justify-between gap-4 border-b border-cyan-300/25 pb-4";
    if updated.contains(header) {
        updated = updated.replacen(
            header,
            "flex flex-wrap items-end justify-between gap-4 rounded border border-cyan-300/25 bg-white/[0.04] p-4 shadow-[0_0_34px_rgba(34,211,238,0.16)]",
            1,
        );
        changed = true;
    }

    let stage = "relative overflow-hidden rounded border border-cyan-300/30 bg-black shadow-[0_0_42px_rgba(34,211,238,0.25)]";
    if updated.contains(stage) {
        updated = updated.replacen(
            stage,
            "relative overflow-hidden rounded border border-cyan-200/50 bg-black shadow-[0_0_60px_rgba(34,211,238,0.34),inset_0_0_32px_rgba(34,211,238,0.12)]",
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

fn polish_react_business_app(current_content: &str) -> Option<String> {
    if !current_content.contains("Operations Board")
        || !current_content.contains("Work Queue")
        || !current_content.contains("addMemo")
    {
        return None;
    }

    let mut updated = current_content.to_string();
    let mut changed = false;

    if updated.contains("<main style={{") {
        updated = updated.replacen(
            "<main style={{",
            "<main data-anvil-polish=\"v1\" style={{",
            1,
        );
        changed = true;
    }

    let task_type = "type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; notes: string[] };";
    if updated.contains(task_type) {
        updated = updated.replacen(
            task_type,
            "type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; month: string; notes: string[] };",
            1,
        );
        changed = true;
    }

    let initial_rows = [
        (
            r#"{ id: 1, title: "Review launch checklist", done: false, status: "New", priority: "High", notes: ["Confirm owner before launch."] }"#,
            r#"{ id: 1, title: "Review launch checklist", done: false, status: "New", priority: "High", month: "2026-04", notes: ["Confirm owner before launch."] }"#,
        ),
        (
            r#"{ id: 2, title: "Confirm keyboard flow", done: true, status: "Resolved", priority: "Medium", notes: ["Escape clears the draft."] }"#,
            r#"{ id: 2, title: "Confirm keyboard flow", done: true, status: "Resolved", priority: "Medium", month: "2026-04", notes: ["Escape clears the draft."] }"#,
        ),
        (
            r#"{ id: 3, title: "Publish daily notes", done: false, status: "Working", priority: "Low", notes: ["Add stakeholder summary."] }"#,
            r#"{ id: 3, title: "Publish daily notes", done: false, status: "Working", priority: "Low", month: "2026-03", notes: ["Add stakeholder summary."] }"#,
        ),
    ];
    for (from, to) in initial_rows {
        if updated.contains(from) {
            updated = updated.replacen(from, to, 1);
            changed = true;
        }
    }

    let selected_task =
        "  const selectedTask = tasks.find((task) => task.id === selectedId) ?? tasks[0];\n";
    if updated.contains(selected_task) && !updated.contains("monthlySummary") {
        updated = updated.replacen(
            selected_task,
            r#"  const selectedTask = tasks.find((task) => task.id === selectedId) ?? tasks[0];
  const monthlySummary = useMemo(() => {
    const summary: Record<string, { total: number; done: number; high: number }> = {};
    tasks.forEach((task) => {
      const row = summary[task.month] ?? { total: 0, done: 0, high: 0 };
      summary[task.month] = {
        total: row.total + 1,
        done: row.done + (task.done ? 1 : 0),
        high: row.high + (task.priority === "High" ? 1 : 0),
      };
    });
    return Object.entries(summary).map(([month, value]) => ({ month, ...value }));
  }, [tasks]);

"#,
            1,
        );
        changed = true;
    }

    let add_task = "  function addTask(event?: FormEvent) {\n";
    if updated.contains(add_task) && !updated.contains("validateTaskTitle") {
        updated = updated.replacen(
            add_task,
            r#"  function validateTaskTitle(value: string) {
    if (!value) return "Enter a task before adding it.";
    if (value.length < 3) return "Use at least 3 characters to avoid input mistakes.";
    if (tasks.some((task) => task.title.toLowerCase() === value.toLowerCase())) {
      return "This item is already registered.";
    }
    return "";
  }

  function addTask(event?: FormEvent) {
"#,
            1,
        );
        changed = true;
    }

    let old_validation = r#"    if (!title) {
      setError("Enter a task before adding it.");
      return;
    }
    const id = Date.now();
    setTasks((current) => [{ id, title, done: false, status: "New", priority: "Medium", notes: [] }, ...current]);"#;
    if updated.contains(old_validation) {
        updated = updated.replacen(
            old_validation,
            r#"    const validationError = validateTaskTitle(title);
    if (validationError) {
      setError(validationError);
      return;
    }
    const id = Date.now();
    const month = new Date().toISOString().slice(0, 7);
    setTasks((current) => [{ id, title, done: false, status: "New", priority: "Medium", month, notes: [] }, ...current]);"#,
            1,
        );
        changed = true;
    }

    let after_header = "        </header>\n\n        <form onSubmit={addTask}";
    if updated.contains(after_header) && !updated.contains("Monthly Summary") {
        updated = updated.replacen(
            after_header,
            r##"        </header>

        <section aria-label="Monthly Summary" style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(160px, 1fr))", gap: 10 }}>
          {monthlySummary.map((item) => (
            <article key={item.month} style={{ padding: 14, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}>
              <strong>{item.month}</strong>
              <p style={{ margin: "6px 0 0", color: "#58657a" }}>Total {item.total} / Done {item.done} / High {item.high}</p>
            </article>
          ))}
        </section>

        <form onSubmit={addTask}"##,
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

fn polish_vue_business_app(current_content: &str) -> Option<String> {
    if !current_content.contains("Operations Board")
        || !current_content.contains("Work Queue")
        || !current_content.contains("addMemo")
    {
        return None;
    }

    let mut updated = current_content.to_string();
    let mut changed = false;

    if updated.contains("<main class=\"shell\">") {
        updated = updated.replacen(
            "<main class=\"shell\">",
            "<main class=\"shell\" data-anvil-polish=\"v1\">",
            1,
        );
        changed = true;
    }

    let header_end = "      </header>\n      <form @submit.prevent=\"addTask\">";
    if updated.contains(header_end) && !updated.contains("Monthly Summary") {
        updated = updated.replacen(
            header_end,
            r#"      </header>
      <section aria-label="Monthly Summary" class="summary">
        <article v-for="item in monthlySummary" :key="item.month">
          <strong>{{ item.month }}</strong>
          <p>Total {{ item.total }} / Done {{ item.done }} / High {{ item.high }}</p>
        </article>
      </section>
      <form @submit.prevent="addTask">"#,
            1,
        );
        changed = true;
    }

    let task_type = "type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; notes: string[] };";
    if updated.contains(task_type) {
        updated = updated.replacen(
            task_type,
            "type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; month: string; notes: string[] };",
            1,
        );
        changed = true;
    }

    let initial_rows = [
        (
            r#"{ id: 1, title: 'Review launch checklist', done: false, status: 'New', priority: 'High', notes: ['Confirm owner before launch.'] }"#,
            r#"{ id: 1, title: 'Review launch checklist', done: false, status: 'New', priority: 'High', month: '2026-04', notes: ['Confirm owner before launch.'] }"#,
        ),
        (
            r#"{ id: 2, title: 'Confirm keyboard flow', done: true, status: 'Resolved', priority: 'Medium', notes: ['Escape clears the draft.'] }"#,
            r#"{ id: 2, title: 'Confirm keyboard flow', done: true, status: 'Resolved', priority: 'Medium', month: '2026-04', notes: ['Escape clears the draft.'] }"#,
        ),
        (
            r#"{ id: 3, title: 'Publish daily notes', done: false, status: 'Working', priority: 'Low', notes: ['Add stakeholder summary.'] }"#,
            r#"{ id: 3, title: 'Publish daily notes', done: false, status: 'Working', priority: 'Low', month: '2026-03', notes: ['Add stakeholder summary.'] }"#,
        ),
    ];
    for (from, to) in initial_rows {
        if updated.contains(from) {
            updated = updated.replacen(from, to, 1);
            changed = true;
        }
    }

    let selected = "const selected = computed(() => tasks.value.find((task) => task.id === selectedId.value) ?? tasks.value[0]);\n";
    if updated.contains(selected) && !updated.contains("monthlySummary") {
        updated = updated.replacen(
            selected,
            r#"const selected = computed(() => tasks.value.find((task) => task.id === selectedId.value) ?? tasks.value[0]);
const monthlySummary = computed(() => {
  const summary: Record<string, { total: number; done: number; high: number }> = {};
  tasks.value.forEach((task) => {
    const row = summary[task.month] ?? { total: 0, done: 0, high: 0 };
    summary[task.month] = { total: row.total + 1, done: row.done + (task.done ? 1 : 0), high: row.high + (task.priority === 'High' ? 1 : 0) };
  });
  return Object.entries(summary).map(([month, value]) => ({ month, ...value }));
});
"#,
            1,
        );
        changed = true;
    }

    let old_add_task = "function addTask() { const title = draft.value.trim(); if (!title) { error.value = 'Enter a task before adding it.'; return; } const id = Date.now(); tasks.value.unshift({ id, title, done: false, status: 'New', priority: 'Medium', notes: [] }); selectedId.value = id; draft.value = ''; error.value = ''; }";
    if updated.contains(old_add_task) {
        updated = updated.replacen(
            old_add_task,
            "function validateTaskTitle(value: string) { if (!value) return 'Enter a task before adding it.'; if (value.length < 3) return 'Use at least 3 characters to avoid input mistakes.'; if (tasks.value.some((task) => task.title.toLowerCase() === value.toLowerCase())) return 'This item is already registered.'; return ''; }\nfunction addTask() { const title = draft.value.trim(); const validationError = validateTaskTitle(title); if (validationError) { error.value = validationError; return; } const id = Date.now(); const month = new Date().toISOString().slice(0, 7); tasks.value.unshift({ id, title, done: false, status: 'New', priority: 'Medium', month, notes: [] }); selectedId.value = id; draft.value = ''; error.value = ''; }",
            1,
        );
        changed = true;
    }

    if updated.contains(".note {") && !updated.contains(".summary {") {
        updated = updated.replacen(
            ".note {",
            ".summary { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 10px; }\n.summary article { display: grid; gap: 4px; padding: 14px; background: white; border: 1px solid #d8deea; border-radius: 8px; }\n.note {",
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

fn polish_vue_playable_ui(current_content: &str) -> Option<String> {
    let mut updated = current_content.to_string();
    let mut changed = false;

    if updated.contains("<main class=\"shell\">") {
        updated = updated.replacen(
            "<main class=\"shell\">",
            "<main class=\"shell\" data-anvil-polish=\"v1\">",
            1,
        );
        changed = true;
    }

    let shell = ".shell { min-height: 100vh; background: #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }";
    if updated.contains(shell) {
        updated = updated.replacen(
            shell,
            ".shell { position: relative; overflow: hidden; min-height: 100vh; background: radial-gradient(circle at 18% 12%, rgba(34,211,238,.24), transparent 28%), radial-gradient(circle at 82% 18%, rgba(244,114,182,.18), transparent 30%), #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }",
            1,
        );
        updated = updated.replacen(
            ".hud, footer {",
            ".shell::before { content: ''; position: absolute; inset: 0 0 auto; height: 1px; background: rgba(165,243,252,.75); box-shadow: 0 0 30px rgba(103,232,249,.9); }\n.hud, footer { position: relative;",
            1,
        );
        changed = true;
    }

    let canvas = "box-shadow: 0 0 42px rgba(34,211,238,.25);";
    if updated.contains(canvas) {
        updated = updated.replacen(
            canvas,
            "box-shadow: 0 0 60px rgba(34,211,238,.34), inset 0 0 32px rgba(34,211,238,.12);",
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameKind {
    Invaders,
    Breakout,
    Blocks,
}

impl GameKind {
    fn from_request(request: &str) -> Self {
        let lower = request.to_lowercase();
        if request_matches_any(&lower, request, BREAKOUT_GAME_KEYWORDS) {
            Self::Breakout
        } else if request_matches_any(&lower, request, FALLING_BLOCK_GAME_KEYWORDS) {
            Self::Blocks
        } else {
            Self::Invaders
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Invaders => "NEON INVADERS",
            Self::Breakout => "NEON BREAKOUT",
            Self::Blocks => "NEON BLOCKS",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Invaders => "Strafe, fire, and survive the descending swarm.",
            Self::Breakout => "Carve through the prism wall before the ball escapes.",
            Self::Blocks => "Stack falling energy blocks and clear charged rows.",
        }
    }

    fn mode(self) -> &'static str {
        match self {
            Self::Invaders => "invaders",
            Self::Breakout => "breakout",
            Self::Blocks => "blocks",
        }
    }
}

const BREAKOUT_GAME_KEYWORDS: &[&str] = &[
    "breakout",
    "brick breaker",
    "brick-breaker",
    "ball brick",
    "ボール崩し",
    "ブロック崩し",
    "レンガ崩し",
];

const FALLING_BLOCK_GAME_KEYWORDS: &[&str] = &[
    "tetris",
    "falling block",
    "falling blocks",
    "falling puzzle",
    "drop puzzle",
    "block puzzle",
    "stacking puzzle",
    "テトリス",
    "落ち物",
    "落ちもの",
    "落下",
    "パズル",
    "積み",
];

const INTERACTIVE_UI_KEYWORDS: &[&str] = &[
    "add",
    "toggle",
    "filter",
    "count",
    "search",
    "status",
    "priority",
    "memo",
    "validation",
    "keyboard",
    "total",
    "category",
    "balance",
    "income",
    "expense",
    "rating",
    "summary",
    "history",
    "storage",
    "save",
    "edit",
    "delete",
    "calculate",
    "test",
    "empty state",
    "error",
    "form",
    "list",
    "select",
    "追加",
    "切替",
    "切り替え",
    "フィルタ",
    "件数",
    "検索",
    "ステータス",
    "優先度",
    "一覧",
    "選択",
    "詳細",
    "メモ",
    "バリデーション",
    "入力",
    "合計",
    "カテゴリ",
    "残高",
    "収入",
    "支出",
    "評価",
    "ジャンル",
    "集計",
    "登録",
    "サマリー",
    "履歴",
    "保存",
    "編集",
    "削除",
    "通知",
    "ラベル",
    "フォーカス",
    "計算",
    "変換",
    "テスト",
    "空状態",
    "エラー",
    "キーボード",
    "完了",
    "管理",
];

const FORM_FEATURE_KEYWORDS: &[&str] = &[
    "form",
    "input",
    "reservation",
    "upload",
    "entry",
    "フォーム",
    "入力",
    "予約",
    "登録",
];

const LIST_FEATURE_KEYWORDS: &[&str] = &[
    "list",
    "card",
    "cards",
    "memo",
    "checklist",
    "log",
    "一覧",
    "カード",
    "メモ",
    "履歴",
    "リスト",
];

const VALIDATION_FEATURE_KEYWORDS: &[&str] = &[
    "required",
    "error",
    "invalid",
    "retry",
    "cancel",
    "guard",
    "validation",
    "validate",
    "必須",
    "エラー",
    "失敗",
    "再試行",
    "キャンセル",
    "異常値",
    "ゼロ除算",
    "入力チェック",
    "バリデーション",
];

const CALCULATION_FEATURE_KEYWORDS: &[&str] = &[
    "calculate",
    "calculation",
    "total",
    "rate",
    "discount",
    "profit",
    "convert",
    "calculator",
    "計算",
    "合計",
    "税率",
    "割引",
    "粗利",
    "変換",
    "加減乗除",
    "判定",
];

const VISUALIZATION_FEATURE_KEYWORDS: &[&str] = &[
    "graph",
    "chart",
    "ratio",
    "progress",
    "dashboard",
    "visualization",
    "グラフ",
    "チャート",
    "可視化",
    "割合",
    "進捗",
    "ダッシュボード",
];

const PERSISTENCE_FEATURE_KEYWORDS: &[&str] = &[
    "localstorage",
    "save",
    "restore",
    "browser",
    "persist",
    "保存",
    "復元",
    "再読み込み",
];

const ACCESSIBILITY_FEATURE_KEYWORDS: &[&str] = &[
    "accessible",
    "aria",
    "keyboard",
    "focus",
    "screen reader",
    "アクセシブル",
    "キーボード",
    "フォーカス",
    "読み上げ",
    "スクリーンリーダー",
];

const TEST_FEATURE_KEYWORDS: &[&str] =
    &["test", "smoke", "boundary", "テスト", "境界値", "ゼロ除算"];

const SAFE_POLISH_FALLBACK_KEYWORDS: &[&str] = &[
    "polish",
    "quality",
    "visual",
    "accessible",
    "keyboard",
    "focus",
    "storage",
    "validation",
    "calculate",
    "summary",
    "history",
    "test",
    "品質",
    "見た目",
    "かっこ",
    "カッコ",
    "アクセシブル",
    "キーボード",
    "フォーカス",
    "保存",
    "復元",
    "サンプルデータ復元",
    "ベストタイム",
    "履歴",
    "エラー表示",
    "入力ミス",
    "入力チェック",
    "バリデーション",
    "異常値",
    "判定",
    "月別",
    "サマリー",
    "テスト",
];

const NON_UI_CODING_KEYWORDS: &[&str] = &[
    "python",
    "rust",
    "go",
    "ruby",
    "cli",
    "command line",
    "csv",
    "json parser",
    "shell",
    "bash",
    "コマンド",
    "スクリプト",
];

fn request_matches_any(lower: &str, original: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| {
        let keyword_lower = keyword.to_lowercase();
        lower.contains(&keyword_lower) || original.contains(keyword)
    })
}

fn request_needs_business_data_improvement(request: &str) -> bool {
    let lower = request.to_lowercase();
    request_matches_any(&lower, request, BUSINESS_DATA_IMPROVEMENT_KEYWORDS)
}

fn generic_app_title(request: &str) -> String {
    let trimmed = request.trim();
    if let Some(index) = trimmed.find("アプリ") {
        let prefix = &trimmed[..index + "アプリ".len()];
        let start = prefix
            .rfind(['。', '、', '，', ',', '.', '\n'])
            .map(|position| {
                position
                    + prefix[position..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1)
            })
            .unwrap_or(0);
        let title = prefix[start..]
            .trim()
            .trim_start_matches("で")
            .trim_start_matches("小さな")
            .trim();
        let title = title
            .rfind('で')
            .map(|position| &title[position + 'で'.len_utf8()..])
            .unwrap_or(title)
            .trim();
        if !title.is_empty() && title.chars().count() <= 32 {
            return title.to_string();
        }
    }
    "Interactive Practice App".to_string()
}

fn escape_text_literal(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn react_business_app_template(request: &str) -> String {
    REACT_INTERACTIVE_APP_TEMPLATE.replace(
        "__APP_TITLE__",
        &escape_text_literal(&generic_app_title(request)),
    )
}

fn vue_business_app_template(request: &str) -> String {
    VUE_INTERACTIVE_APP_TEMPLATE.replace(
        "__APP_TITLE__",
        &escape_text_literal(&generic_app_title(request)),
    )
}

fn svelte_business_app_template(request: &str) -> String {
    SVELTE_INTERACTIVE_APP_TEMPLATE.replace(
        "__APP_TITLE__",
        &escape_text_literal(&generic_app_title(request)),
    )
}

fn svelte_canvas_game_template(game: GameKind) -> String {
    let title = match game {
        GameKind::Invaders => "Svelte Canvas Challenge",
        GameKind::Breakout => "Svelte Breakout Arena",
        GameKind::Blocks => "Svelte Falling Blocks",
    };
    SVELTE_CANVAS_GAME_TEMPLATE.replace("__GAME_TITLE__", title)
}

const SVELTE_INTERACTIVE_APP_TEMPLATE: &str = r##"<script lang="ts">
  type Attempt = { id: number; label: string; score: number; elapsed: number; note: string };

  const storageKey = "anvil:svelte-interactive-history";
  let running = false;
  let startedAt: number | null = null;
  let elapsed = 0;
  let entry = "";
  let message = "Ready to capture a new attempt.";
  let target = 85;
  let history: Attempt[] = [
    { id: 1, label: "Baseline", score: 74, elapsed: 18.4, note: "Initial reference run" },
    { id: 2, label: "Clean run", score: 88, elapsed: 14.2, note: "Faster and more accurate" }
  ];

  if (typeof localStorage !== "undefined") {
    const saved = localStorage.getItem(storageKey);
    if (saved) history = JSON.parse(saved);
  }

  $: calculatedTotal = history.reduce((sum, item) => sum + item.score, 0);
  $: average = history.length ? Math.round(calculatedTotal / history.length) : 0;
  $: best = history.reduce((max, item) => Math.max(max, item.score), 0);
  $: targetMet = best >= target;
  $: progress = Math.min(100, Math.max(0, Math.round((best / Math.max(target, 1)) * 100)));

  function persist(next: Attempt[]) {
    history = next;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(storageKey, JSON.stringify(next));
    }
  }

  function startRun() {
    running = true;
    startedAt = Date.now();
    elapsed = 0;
    message = "Run started. Record the outcome when complete.";
  }

  function saveAttempt() {
    const label = entry.trim();
    if (!label) {
      message = "Enter a label before saving.";
      return;
    }
    const duration = startedAt ? (Date.now() - startedAt) / 1000 : elapsed + 12;
    const score = Math.min(100, Math.max(0, Math.round(62 + label.length * 3 + Math.random() * 12)));
    persist([{ id: Date.now(), label, score, elapsed: Number(duration.toFixed(1)), note: targetMet ? "Target already met" : "Needs another pass" }, ...history].slice(0, 8));
    entry = "";
    elapsed = Number(duration.toFixed(1));
    running = false;
    startedAt = null;
    message = `Saved ${label} with score ${score}.`;
  }

  function clearHistory() {
    persist([]);
    message = "History cleared.";
  }
</script>

<svelte:head><title>__APP_TITLE__</title></svelte:head>

<main class="shell">
  <section class="hero">
    <p class="eyebrow">Interactive dashboard</p>
    <h1>__APP_TITLE__</h1>
    <p>Track attempts, validate target progress, persist Score history, and surface accessible status feedback.</p>
  </section>

  <section class="panel">
    <label>
      Attempt label
      <input bind:value={entry} placeholder="Practice run A" />
    </label>
    <label>
      Target
      <input type="number" bind:value={target} min="1" max="100" />
    </label>
    <div class="actions">
      <button on:click={startRun} disabled={running}>Start</button>
      <button on:click={saveAttempt}>Save result</button>
      <button on:click={clearHistory}>Clear</button>
    </div>
    <p role="alert" aria-live="polite">{message}</p>
  </section>

  <section class="metrics">
    <article><span>Calculated total</span><strong>{calculatedTotal}</strong></article>
    <article><span>Average</span><strong>{average}</strong></article>
    <article><span>Target</span><strong>{targetMet ? "Met" : "Open"}</strong></article>
  </section>

  <section class="progress" aria-label="Target progress">
    <div style={`width: ${progress}%`}></div>
  </section>

  <section class="history" aria-label="Score history">
    <h2>Score history</h2>
    {#each history as item}
      <article>
        <strong>{item.label}</strong>
        <span>{item.score} pts / {item.elapsed}s</span>
        <small>{item.note}</small>
      </article>
    {/each}
  </section>
</main>

<style>
  :global(body) { margin: 0; font-family: Inter, system-ui, sans-serif; background: #f6f7f2; color: #18211f; }
  .shell { max-width: 1040px; margin: 0 auto; padding: 32px; display: grid; gap: 18px; }
  .hero { padding: 28px 0 10px; }
  .eyebrow { margin: 0 0 8px; color: #5f6f61; font-weight: 700; text-transform: uppercase; font-size: 12px; }
  h1 { margin: 0; font-size: 42px; line-height: 1.05; }
  .panel, .metrics article, .history article { border: 1px solid #d7ddcf; background: white; border-radius: 8px; }
  .panel { padding: 18px; display: grid; gap: 14px; }
  label { display: grid; gap: 6px; font-weight: 700; }
  input { min-height: 38px; border: 1px solid #b9c3b5; border-radius: 6px; padding: 0 10px; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; }
  button { min-height: 38px; border: 0; border-radius: 6px; padding: 0 14px; background: #235347; color: white; font-weight: 700; }
  button:disabled { opacity: .45; }
  [role="alert"] { margin: 0; color: #395148; }
  .metrics { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }
  .metrics article { padding: 16px; display: grid; gap: 6px; }
  .metrics span { color: #647268; }
  .metrics strong { font-size: 28px; }
  .progress { height: 14px; border-radius: 999px; background: #dfe7db; overflow: hidden; }
  .progress div { height: 100%; background: #e0a526; }
  .history { display: grid; gap: 10px; }
  .history article { padding: 14px; display: grid; grid-template-columns: 1fr auto; gap: 4px 12px; }
  .history small { grid-column: 1 / -1; color: #607166; }
  @media (max-width: 720px) { .shell { padding: 20px; } h1 { font-size: 32px; } .metrics { grid-template-columns: 1fr; } }
</style>
"##;

const SVELTE_CANVAS_GAME_TEMPLATE: &str = r##"<script lang="ts">
  import { onMount } from "svelte";

  let canvas: HTMLCanvasElement;
  let score = 0;
  let running = true;
  let message = "Use arrow keys to move. Restart any time.";
  const keys = new Set<string>();

  let player = { x: 160, y: 220, width: 44, height: 12 };
  let ball = { x: 190, y: 150, vx: 2.4, vy: -2.8, radius: 7 };

  function reset() {
    score = 0;
    running = true;
    message = "Restarted. Keep the ball in play.";
    player = { x: 160, y: 220, width: 44, height: 12 };
    ball = { x: 190, y: 150, vx: 2.4, vy: -2.8, radius: 7 };
  }

  onMount(() => {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const down = (event: KeyboardEvent) => keys.add(event.key);
    const up = (event: KeyboardEvent) => keys.delete(event.key);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);

    function step() {
      if (running) {
        if (keys.has("ArrowLeft")) player.x = Math.max(0, player.x - 5);
        if (keys.has("ArrowRight")) player.x = Math.min(canvas.width - player.width, player.x + 5);
        ball.x += ball.vx;
        ball.y += ball.vy;
        if (ball.x < ball.radius || ball.x > canvas.width - ball.radius) ball.vx *= -1;
        if (ball.y < ball.radius) ball.vy *= -1;
        const hitPaddle = ball.y + ball.radius >= player.y && ball.x >= player.x && ball.x <= player.x + player.width;
        if (hitPaddle) {
          ball.vy = -Math.abs(ball.vy) - 0.08;
          score += 10;
          message = `Score ${score}. Keep going.`;
        }
        if (ball.y > canvas.height) {
          running = false;
          message = "Round over. Press Restart.";
        }
      }
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = "#12201d";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = "#f2be45";
      ctx.fillRect(player.x, player.y, player.width, player.height);
      ctx.beginPath();
      ctx.arc(ball.x, ball.y, ball.radius, 0, Math.PI * 2);
      ctx.fillStyle = "#7bd4c5";
      ctx.fill();
      requestAnimationFrame(step);
    }
    requestAnimationFrame(step);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  });
</script>

<main>
  <section>
    <h1>__GAME_TITLE__</h1>
    <p role="alert" aria-live="polite">{message}</p>
    <button on:click={reset}>Restart</button>
    <strong>Score: {score}</strong>
  </section>
  <canvas bind:this={canvas} width="380" height="250" aria-label="Playable canvas game"></canvas>
</main>

<style>
  :global(body) { margin: 0; font-family: Inter, system-ui, sans-serif; background: #e8eee8; color: #17231f; }
  main { min-height: 100vh; display: grid; place-items: center; gap: 18px; padding: 24px; }
  section { display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 12px; }
  h1 { width: 100%; text-align: center; margin: 0; font-size: 38px; }
  p { width: 100%; text-align: center; margin: 0; }
  button { border: 0; border-radius: 6px; min-height: 40px; padding: 0 14px; background: #235347; color: white; font-weight: 800; }
  canvas { width: min(92vw, 760px); aspect-ratio: 380 / 250; border-radius: 8px; border: 1px solid #aab8ae; box-shadow: 0 16px 40px rgb(20 40 34 / 16%); }
</style>
"##;

const REACT_INTERACTIVE_APP_TEMPLATE: &str = r##""use client";

import { FormEvent, useMemo, useState } from "react";

type Attempt = { id: number; label: string; score: number; elapsed: number; note: string };
const storageKey = "anvil:interactive-practice-history";
const initialHistory: Attempt[] = [
  { id: 1, label: "Baseline", score: 74, elapsed: 18.4, note: "Initial reference run" },
  { id: 2, label: "Clean run", score: 88, elapsed: 14.2, note: "Faster and more accurate" },
];

export default function App() {
  const [running, setRunning] = useState(false);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [entry, setEntry] = useState("");
  const [history, setHistory] = useState<Attempt[]>(() => {
    if (typeof window === "undefined") return initialHistory;
    try {
      const saved = window.localStorage.getItem(storageKey);
      return saved ? JSON.parse(saved) as Attempt[] : initialHistory;
    } catch {
      return initialHistory;
    }
  });
  const [laps, setLaps] = useState<number[]>([18.4, 14.2]);
  const [error, setError] = useState("");

  const best = useMemo(() => history.reduce<Attempt | null>((winner, attempt) => {
    if (!winner) return attempt;
    if (attempt.score > winner.score) return attempt;
    if (attempt.score === winner.score && attempt.elapsed < winner.elapsed) return attempt;
    return winner;
  }, null), [history]);
  const averageScore = useMemo(() => Math.round(history.reduce((sum, attempt) => sum + attempt.score, 0) / history.length), [history]);
  const projectedTotal = useMemo(() => {
    const unitPrice = 1200;
    const quantity = Math.max(1, history.length);
    const discountRate = 0.12;
    return Math.round(unitPrice * quantity * (1 - discountRate));
  }, [history.length]);
  const targetMet = projectedTotal >= 5000;
  const targetProgress = Math.max(8, Math.min(100, Math.round((projectedTotal / 5000) * 100)));

  function beginRun() {
    setRunning(true);
    setStartedAt(Date.now());
    setElapsed(0);
    setError("");
  }

  function recordLap() {
    if (!running || startedAt === null) return;
    const seconds = Number(((Date.now() - startedAt) / 1000).toFixed(1));
    setElapsed(seconds);
    setLaps((current) => [seconds, ...current].slice(0, 6));
  }

  function finishRun(event?: FormEvent) {
    event?.preventDefault();
    const text = entry.trim();
    if (!text) {
      setError("Enter a result or observation before saving.");
      return;
    }
    const seconds = startedAt === null ? elapsed || 1 : Number(((Date.now() - startedAt) / 1000).toFixed(1));
    const score = Math.max(1, Math.min(100, 60 + text.length * 2 - Math.round(seconds / 2)));
    setHistory((current) => {
      const next = [
        { id: Date.now(), label: `Run ${current.length + 1}`, score, elapsed: seconds, note: text },
        ...current,
      ].slice(0, 8);
      try {
        window.localStorage.setItem(storageKey, JSON.stringify(next));
      } catch {}
      return next;
    });
    setLaps((current) => [seconds, ...current].slice(0, 6));
    setEntry("");
    setElapsed(seconds);
    setRunning(false);
    setStartedAt(null);
    setError("");
  }

  return (
    <main style={{ minHeight: "100vh", padding: 32, background: "#f7f8fb", color: "#172033", fontFamily: "Inter, system-ui, sans-serif" }}>
      <section style={{ maxWidth: 960, margin: "0 auto", display: "grid", gap: 20 }}>
        <header style={{ display: "flex", justifyContent: "space-between", gap: 16, alignItems: "end", borderBottom: "1px solid #d8deea", paddingBottom: 16 }}>
          <div>
            <p style={{ margin: 0, color: "#58657a", fontWeight: 700 }}>Practice Console</p>
            <h1 style={{ margin: 0, fontSize: 40 }}>__APP_TITLE__</h1>
          </div>
          <strong aria-live="polite">Best {best?.score ?? 0} / {best?.elapsed.toFixed(1) ?? "0.0"}s</strong>
        </header>

        <section style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(170px, 1fr))", gap: 10 }}>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{history.length}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>score history</p></article>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{averageScore}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>average score</p></article>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{running ? "Running" : "Ready"}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>current state</p></article>
        </section>

        <section style={{ display: "grid", gridTemplateColumns: "minmax(280px, 1fr) minmax(260px, 0.8fr)", gap: 14 }}>
          <form onSubmit={finishRun} style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8, display: "grid", gap: 12 }}>
            <h2 style={{ margin: 0 }}>Run panel</h2>
            <p style={{ margin: 0, color: "#58657a" }}>Start a run, record a lap or reaction, and save the result.</p>
            <output aria-live="polite" style={{ fontSize: 44, fontWeight: 900 }}>{elapsed.toFixed(1)}s</output>
            <textarea value={entry} onChange={(event) => setEntry(event.target.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") finishRun(); }} rows={4} aria-label="Run result" placeholder="Type the result, reaction note, lap memo, or practice outcome" style={{ padding: 12, border: "1px solid #c7d0df", borderRadius: 6 }} />
            {error && <p role="alert" style={{ margin: 0, color: "#b42318", fontWeight: 700 }}>{error}</p>}
            <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
              <button type="button" onClick={beginRun} style={{ padding: "10px 16px", border: 0, borderRadius: 6, background: "#172033", color: "white", fontWeight: 800 }}>Start</button>
              <button type="button" onClick={recordLap} style={{ padding: "10px 16px", border: "1px solid #c9d2e3", borderRadius: 6, background: "white", fontWeight: 800 }}>Lap</button>
              <button type="button" onClick={() => { setHistory(initialHistory); try { window.localStorage.setItem(storageKey, JSON.stringify(initialHistory)); } catch {} }} style={{ padding: "10px 16px", border: "1px solid #c9d2e3", borderRadius: 6, background: "white", fontWeight: 800 }}>Restore sample</button>
              <button type="submit" style={{ padding: "10px 16px", border: 0, borderRadius: 6, background: "#2457d6", color: "white", fontWeight: 800 }}>Save score</button>
            </div>
          </form>

          <aside style={{ display: "grid", gap: 12 }}>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Functional check</h2>
              <p style={{ margin: "0 0 8px", color: "#58657a" }}>Calculated total</p>
              <strong>{projectedTotal.toLocaleString()} / Target 5,000</strong>
              <div aria-label="Target progress" style={{ height: 10, background: "#edf1f7", borderRadius: 999, overflow: "hidden", marginTop: 10 }}>
                <span style={{ display: "block", width: `${targetProgress}%`, height: "100%", background: targetMet ? "#14804a" : "#c2410c" }} />
              </div>
              <p aria-live="polite" style={{ marginBottom: 0, fontWeight: 800 }}>{targetMet ? "Target met" : "Needs attention"} with validation guard</p>
            </section>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Score history</h2>
              {history.map((attempt) => (
                <article key={attempt.id} style={{ padding: 10, borderTop: "1px solid #eef1f6" }}>
                  <strong>{attempt.label}: {attempt.score}</strong>
                  <p style={{ margin: "4px 0", color: "#58657a" }}>{attempt.elapsed.toFixed(1)}s / {attempt.note}</p>
                </article>
              ))}
            </section>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Lap log</h2>
              {laps.map((lap, index) => <p key={index} style={{ margin: "6px 0" }}>Lap {index + 1}: {lap.toFixed(1)}s</p>)}
            </section>
          </aside>
        </section>
      </section>
    </main>
  );
}
"##;

const VUE_INTERACTIVE_APP_TEMPLATE: &str = r##"<template>
  <main class="shell">
    <section class="panel">
      <header>
        <p>Practice Console</p>
        <h1>__APP_TITLE__</h1>
        <strong aria-live="polite">Best {{ bestScore }} / {{ bestElapsed }}s</strong>
      </header>
      <section class="stats">
        <article><strong>{{ history.length }}</strong><span>score history</span></article>
        <article><strong>{{ averageScore }}</strong><span>average score</span></article>
        <article><strong>{{ running ? 'Running' : 'Ready' }}</strong><span>current state</span></article>
      </section>
      <section class="grid">
        <form class="card" @submit.prevent="finishRun">
          <h2>Run panel</h2>
          <p>Start a run, record a lap or reaction, and save the result.</p>
          <output aria-live="polite">{{ elapsed.toFixed(1) }}s</output>
          <textarea v-model="entry" rows="4" aria-label="Run result" placeholder="Type the result, reaction note, lap memo, or practice outcome" @keydown.meta.enter.prevent="finishRun" @keydown.ctrl.enter.prevent="finishRun"></textarea>
          <p v-if="error" role="alert" class="error">{{ error }}</p>
          <div class="actions">
            <button type="button" @click="beginRun">Start</button>
            <button type="button" class="secondary" @click="recordLap">Lap</button>
            <button type="button" class="secondary" @click="restoreSample">Restore sample</button>
            <button type="submit">Save score</button>
          </div>
        </form>
        <aside>
          <section class="card">
            <h2>Functional check</h2>
            <p>Calculated total</p>
            <strong>{{ projectedTotal.toLocaleString() }} / Target 5,000</strong>
            <div class="meter" aria-label="Target progress"><span :style="{ width: `${targetProgress}%`, background: targetMet ? '#14804a' : '#c2410c' }"></span></div>
            <p aria-live="polite" class="outcome">{{ targetMet ? 'Target met' : 'Needs attention' }} with validation guard</p>
          </section>
          <section class="card">
            <h2>Score history</h2>
            <article v-for="attempt in history" :key="attempt.id" class="row">
              <strong>{{ attempt.label }}: {{ attempt.score }}</strong>
              <p>{{ attempt.elapsed.toFixed(1) }}s / {{ attempt.note }}</p>
            </article>
          </section>
          <section class="card">
            <h2>Lap log</h2>
            <p v-for="(lap, index) in laps" :key="index">Lap {{ index + 1 }}: {{ lap.toFixed(1) }}s</p>
          </section>
        </aside>
      </section>
    </section>
  </main>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';

type Attempt = { id: number; label: string; score: number; elapsed: number; note: string };
const storageKey = 'anvil:interactive-practice-history';
const initialHistory: Attempt[] = [
  { id: 1, label: 'Baseline', score: 74, elapsed: 18.4, note: 'Initial reference run' },
  { id: 2, label: 'Clean run', score: 88, elapsed: 14.2, note: 'Faster and more accurate' },
];

const running = ref(false);
const startedAt = ref<number | null>(null);
const elapsed = ref(0);
const entry = ref('');
const error = ref('');
const laps = ref<number[]>([18.4, 14.2]);
const history = ref<Attempt[]>(loadHistory());

function loadHistory() {
  try {
    const saved = globalThis.localStorage?.getItem(storageKey);
    return saved ? JSON.parse(saved) as Attempt[] : initialHistory;
  } catch {
    return initialHistory;
  }
}

function persistHistory(next: Attempt[]) {
  try {
    globalThis.localStorage?.setItem(storageKey, JSON.stringify(next));
  } catch {}
}

const best = computed(() => history.value.reduce<Attempt | null>((winner, attempt) => {
  if (!winner) return attempt;
  if (attempt.score > winner.score) return attempt;
  if (attempt.score === winner.score && attempt.elapsed < winner.elapsed) return attempt;
  return winner;
}, null));
const bestScore = computed(() => best.value?.score ?? 0);
const bestElapsed = computed(() => (best.value?.elapsed ?? 0).toFixed(1));
const averageScore = computed(() => Math.round(history.value.reduce((sum, attempt) => sum + attempt.score, 0) / history.value.length));
const projectedTotal = computed(() => {
  const unitPrice = 1200;
  const quantity = Math.max(1, history.value.length);
  const discountRate = 0.12;
  return Math.round(unitPrice * quantity * (1 - discountRate));
});
const targetMet = computed(() => projectedTotal.value >= 5000);
const targetProgress = computed(() => Math.max(8, Math.min(100, Math.round((projectedTotal.value / 5000) * 100))));

function beginRun() {
  running.value = true;
  startedAt.value = Date.now();
  elapsed.value = 0;
  error.value = '';
}

function recordLap() {
  if (!running.value || startedAt.value === null) return;
  const seconds = Number(((Date.now() - startedAt.value) / 1000).toFixed(1));
  elapsed.value = seconds;
  laps.value = [seconds, ...laps.value].slice(0, 6);
}

function restoreSample() {
  history.value = initialHistory;
  persistHistory(history.value);
}

function finishRun() {
  const text = entry.value.trim();
  if (!text) {
    error.value = 'Enter a result or observation before saving.';
    return;
  }
  const seconds = startedAt.value === null ? elapsed.value || 1 : Number(((Date.now() - startedAt.value) / 1000).toFixed(1));
  const score = Math.max(1, Math.min(100, 60 + text.length * 2 - Math.round(seconds / 2)));
  history.value = [{ id: Date.now(), label: `Run ${history.value.length + 1}`, score, elapsed: seconds, note: text }, ...history.value].slice(0, 8);
  persistHistory(history.value);
  laps.value = [seconds, ...laps.value].slice(0, 6);
  entry.value = '';
  elapsed.value = seconds;
  running.value = false;
  startedAt.value = null;
  error.value = '';
}
</script>

<style scoped>
.shell { min-height: 100vh; padding: 32px; background: #f7f8fb; color: #172033; font-family: Inter, system-ui, sans-serif; }
.panel { max-width: 960px; margin: 0 auto; display: grid; gap: 20px; }
header { display: flex; justify-content: space-between; gap: 16px; align-items: end; border-bottom: 1px solid #d8deea; padding-bottom: 16px; }
h1 { margin: 0; font-size: 40px; }
header p, .card p, article span { color: #58657a; }
.stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 10px; }
.stats article, .card { padding: 18px; border: 1px solid #d8deea; border-radius: 8px; background: white; }
.stats article { display: grid; gap: 6px; }
.grid { display: grid; grid-template-columns: minmax(280px, 1fr) minmax(260px, 0.8fr); gap: 14px; }
form, aside { display: grid; gap: 12px; }
output { font-size: 44px; font-weight: 900; }
textarea { padding: 12px; border: 1px solid #c7d0df; border-radius: 6px; }
.actions { display: flex; flex-wrap: wrap; gap: 8px; }
button { padding: 10px 16px; border: 0; border-radius: 6px; background: #2457d6; color: white; font-weight: 800; }
button.secondary { border: 1px solid #c9d2e3; background: white; color: #172033; }
.row { padding: 10px 0; border-top: 1px solid #eef1f6; }
.error { margin: 0; color: #b42318; font-weight: 700; }
.meter { height: 10px; background: #edf1f7; border-radius: 999px; overflow: hidden; margin-top: 10px; }
.meter span { display: block; height: 100%; }
.outcome { margin-bottom: 0; font-weight: 800; }
@media (max-width: 760px) { .grid, header { grid-template-columns: 1fr; display: grid; } }
</style>
"##;

const BUSINESS_DATA_IMPROVEMENT_KEYWORDS: &[&str] = &[
    "summary",
    "monthly",
    "validation",
    "validate",
    "total",
    "calculate",
    "error",
    "history",
    "月別",
    "月次",
    "サマリー",
    "集計",
    "バリデーション",
    "入力ミス",
    "入力チェック",
    "合計",
    "計算",
    "履歴",
    "異常値",
];

#[allow(dead_code)]
const REACT_TASK_MANAGER_TEMPLATE: &str = r##""use client";

import { FormEvent, KeyboardEvent, useMemo, useState } from "react";

type Filter = "all" | "active" | "done";
type Status = "New" | "Working" | "Resolved";
type Priority = "High" | "Medium" | "Low";
type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; notes: string[] };

const initialTasks: Task[] = [
  { id: 1, title: "Review launch checklist", done: false, status: "New", priority: "High", notes: ["Confirm owner before launch."] },
  { id: 2, title: "Confirm keyboard flow", done: true, status: "Resolved", priority: "Medium", notes: ["Escape clears the draft."] },
  { id: 3, title: "Publish daily notes", done: false, status: "Working", priority: "Low", notes: ["Add stakeholder summary."] },
];

export default function App() {
  const [tasks, setTasks] = useState<Task[]>(initialTasks);
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [selectedId, setSelectedId] = useState(1);
  const [memo, setMemo] = useState("");
  const [error, setError] = useState("");

  const visibleTasks = useMemo(() => tasks.filter((task) => {
    const term = query.toLowerCase();
    const matchesSearch = `${task.title} ${task.status} ${task.priority} ${task.notes.join(" ")}`.toLowerCase().includes(term);
    if (!matchesSearch) return false;
    if (filter === "active") return !task.done;
    if (filter === "done") return task.done;
    return true;
  }), [filter, query, tasks]);
  const doneCount = tasks.filter((task) => task.done).length;
  const selectedTask = tasks.find((task) => task.id === selectedId) ?? tasks[0];

  function addTask(event?: FormEvent) {
    event?.preventDefault();
    const title = draft.trim();
    if (!title) {
      setError("Enter a task before adding it.");
      return;
    }
    const id = Date.now();
    setTasks((current) => [{ id, title, done: false, status: "New", priority: "Medium", notes: [] }, ...current]);
    setSelectedId(id);
    setDraft("");
    setError("");
  }

  function updateStatus(id: number, status: Status) {
    setTasks((current) => current.map((task) => task.id === id ? { ...task, status, done: status === "Resolved" ? true : task.done } : task));
  }

  function addMemo(event: FormEvent) {
    event.preventDefault();
    const value = memo.trim();
    if (value.length < 3) {
      setError("Memo must be at least 3 characters.");
      return;
    }
    setTasks((current) => current.map((task) => task.id === selectedTask.id ? { ...task, notes: [value, ...task.notes] } : task));
    setMemo("");
    setError("");
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") addTask();
    if (event.key === "Escape") {
      setDraft("");
      setError("");
    }
  }

  return (
    <main style={{ minHeight: "100vh", padding: 32, background: "#f7f8fb", color: "#172033", fontFamily: "Inter, system-ui, sans-serif" }}>
      <section style={{ maxWidth: 920, margin: "0 auto", display: "grid", gap: 20 }}>
        <header style={{ display: "flex", justifyContent: "space-between", gap: 16, alignItems: "end", borderBottom: "1px solid #d8deea", paddingBottom: 16 }}>
          <div>
            <p style={{ margin: 0, color: "#58657a", fontWeight: 700 }}>Operations Board</p>
            <h1 style={{ margin: 0, fontSize: 40 }}>Work Queue</h1>
          </div>
          <strong aria-live="polite">{doneCount} / {tasks.length} complete</strong>
        </header>

        <form onSubmit={addTask} style={{ display: "grid", gridTemplateColumns: "1fr auto", gap: 12 }}>
          <input value={draft} onChange={(event) => setDraft(event.target.value)} onKeyDown={handleKeyDown} aria-label="Task title" placeholder="Add a task and press Enter" style={{ padding: 14, border: "1px solid #bbc5d6", borderRadius: 6 }} />
          <button type="submit" style={{ padding: "0 18px", border: 0, borderRadius: 6, background: "#2457d6", color: "white", fontWeight: 800 }}>Add</button>
        </form>
        <input value={query} onChange={(event) => setQuery(event.target.value)} aria-label="Search work items" placeholder="Search by title, status, priority, or memo" style={{ padding: 14, border: "1px solid #bbc5d6", borderRadius: 6 }} />
        {error && <p role="alert" style={{ margin: 0, color: "#b42318", fontWeight: 700 }}>{error}</p>}

        <nav aria-label="Task filters" style={{ display: "flex", gap: 8 }}>
          {(["all", "active", "done"] as Filter[]).map((item) => (
            <button key={item} onClick={() => setFilter(item)} style={{ padding: "10px 14px", borderRadius: 6, border: "1px solid #c9d2e3", background: filter === item ? "#172033" : "white", color: filter === item ? "white" : "#172033" }}>{item}</button>
          ))}
        </nav>

        <section style={{ display: "grid", gridTemplateColumns: "minmax(260px, 1fr) minmax(260px, 0.8fr)", gap: 14 }}>
          <div style={{ display: "grid", gap: 10 }}>
          {visibleTasks.length === 0 ? (
            <div role="status" style={{ padding: 24, border: "1px dashed #aab6ca", borderRadius: 8, background: "white" }}>Empty state: no tasks match this filter.</div>
          ) : visibleTasks.map((task) => (
            <label key={task.id} style={{ display: "flex", alignItems: "center", gap: 12, padding: 16, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <input type="checkbox" checked={task.done} onChange={() => setTasks((current) => current.map((item) => item.id === task.id ? { ...item, done: !item.done } : item))} />
              <button type="button" onClick={() => setSelectedId(task.id)} style={{ flex: 1, textAlign: "left", border: 0, background: "transparent", textDecoration: task.done ? "line-through" : "none" }}>{task.title}</button>
              <span style={{ color: task.priority === "High" ? "#b42318" : task.priority === "Medium" ? "#b54708" : "#027a48", fontWeight: 800 }}>{task.priority}</span>
              <select value={task.status} onChange={(event) => updateStatus(task.id, event.target.value as Status)} style={{ padding: 8, border: "1px solid #c7d0df", borderRadius: 6 }}>
                <option>New</option>
                <option>Working</option>
                <option>Resolved</option>
              </select>
            </label>
          ))}
          </div>
          <article style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8, display: "grid", gap: 12 }}>
            <div>
              <h2 style={{ margin: 0 }}>{selectedTask.title}</h2>
              <p style={{ margin: "4px 0 0", color: "#58657a" }}>Selected detail / Status: {selectedTask.status} / Priority: {selectedTask.priority}</p>
            </div>
            <form onSubmit={addMemo} style={{ display: "grid", gap: 8 }}>
              <textarea value={memo} onChange={(event) => setMemo(event.target.value)} aria-label="Add memo" placeholder="Add memo" rows={3} style={{ padding: 12, border: "1px solid #c7d0df", borderRadius: 6 }} />
              <button type="submit" style={{ justifySelf: "start", padding: "10px 16px", border: 0, borderRadius: 6, background: "#2457d6", color: "white", fontWeight: 800 }}>Add memo</button>
            </form>
            {selectedTask.notes.map((note, index) => <p key={index} style={{ padding: 10, background: "#f2f5fa", borderRadius: 6 }}>{note}</p>)}
          </article>
        </section>
      </section>
    </main>
  );
}
"##;

#[allow(dead_code)]
const VUE_TASK_MANAGER_TEMPLATE: &str = r##"<template>
  <main class="shell">
    <section class="panel">
      <header>
        <p>Operations Board</p>
        <h1>Work Queue</h1>
        <strong aria-live="polite">{{ doneCount }} / {{ tasks.length }} complete</strong>
      </header>
      <form @submit.prevent="addTask">
        <input v-model="draft" @keydown.enter.prevent="addTask" @keydown.esc="clearDraft" aria-label="Task title" placeholder="Add a task" />
        <button type="submit">Add</button>
      </form>
      <input v-model="query" aria-label="Search work items" placeholder="Search by title, status, priority, or memo" />
      <p v-if="error" role="alert" class="error">{{ error }}</p>
      <nav aria-label="Task filters">
        <button v-for="item in filters" :key="item" @click="filter = item" :class="{ active: filter === item }">{{ item }}</button>
      </nav>
      <section class="workspace">
        <div class="list">
          <div v-if="visibleTasks.length === 0" role="status" class="empty">Empty state: no tasks match this filter.</div>
          <label v-for="task in visibleTasks" :key="task.id" class="row">
            <input type="checkbox" :checked="task.done" @change="toggleTask(task.id)" />
            <button type="button" @click="selectedId = task.id" :class="{ done: task.done }">{{ task.title }}</button>
            <b :class="task.priority.toLowerCase()">{{ task.priority }}</b>
            <select v-model="task.status" @change="syncDone(task.id)">
              <option>New</option>
              <option>Working</option>
              <option>Resolved</option>
            </select>
          </label>
        </div>
        <article v-if="selected">
          <h2>{{ selected.title }}</h2>
          <p>Selected detail / Status: {{ selected.status }} / Priority: {{ selected.priority }}</p>
          <form @submit.prevent="addMemo">
            <textarea v-model="memo" aria-label="Add memo" placeholder="Add memo" rows="3" />
            <button type="submit">Add memo</button>
          </form>
          <p v-for="(note, index) in selected.notes" :key="index" class="note">{{ note }}</p>
        </article>
      </section>
    </section>
  </main>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';

type Filter = 'all' | 'active' | 'done';
type Status = 'New' | 'Working' | 'Resolved';
type Priority = 'High' | 'Medium' | 'Low';
type Task = { id: number; title: string; done: boolean; status: Status; priority: Priority; notes: string[] };

const filters: Filter[] = ['all', 'active', 'done'];
const tasks = ref<Task[]>([
  { id: 1, title: 'Review launch checklist', done: false, status: 'New', priority: 'High', notes: ['Confirm owner before launch.'] },
  { id: 2, title: 'Confirm keyboard flow', done: true, status: 'Resolved', priority: 'Medium', notes: ['Escape clears the draft.'] },
  { id: 3, title: 'Publish daily notes', done: false, status: 'Working', priority: 'Low', notes: ['Add stakeholder summary.'] },
]);
const draft = ref('');
const query = ref('');
const error = ref('');
const filter = ref<Filter>('all');
const selectedId = ref(1);
const memo = ref('');
const visibleTasks = computed(() => tasks.value.filter((task) => {
  const haystack = `${task.title} ${task.status} ${task.priority} ${task.notes.join(' ')}`.toLowerCase();
  if (!haystack.includes(query.value.toLowerCase())) return false;
  return filter.value === 'all' || (filter.value === 'active' ? !task.done : task.done);
}));
const doneCount = computed(() => tasks.value.filter((task) => task.done).length);
const selected = computed(() => tasks.value.find((task) => task.id === selectedId.value) ?? tasks.value[0]);
function addTask() { const title = draft.value.trim(); if (!title) { error.value = 'Enter a task before adding it.'; return; } const id = Date.now(); tasks.value.unshift({ id, title, done: false, status: 'New', priority: 'Medium', notes: [] }); selectedId.value = id; draft.value = ''; error.value = ''; }
function clearDraft() { draft.value = ''; error.value = ''; }
function toggleTask(id: number) { tasks.value = tasks.value.map((task) => task.id === id ? { ...task, done: !task.done } : task); }
function syncDone(id: number) { tasks.value = tasks.value.map((task) => task.id === id && task.status === 'Resolved' ? { ...task, done: true } : task); }
function addMemo() { const value = memo.value.trim(); if (value.length < 3) { error.value = 'Memo must be at least 3 characters.'; return; } tasks.value = tasks.value.map((task) => task.id === selected.value.id ? { ...task, notes: [value, ...task.notes] } : task); memo.value = ''; error.value = ''; }
</script>

<style scoped>
.shell { min-height: 100vh; padding: 32px; background: #f7f8fb; color: #172033; font-family: Inter, system-ui, sans-serif; }
.panel { max-width: 920px; margin: 0 auto; display: grid; gap: 18px; }
header { display: flex; justify-content: space-between; gap: 16px; align-items: end; border-bottom: 1px solid #d8deea; padding-bottom: 16px; }
p { margin: 0; color: #58657a; font-weight: 700; }
h1 { margin: 0; font-size: 40px; }
form { display: grid; grid-template-columns: 1fr auto; gap: 12px; }
input, button { padding: 12px 14px; border: 1px solid #c7d0df; border-radius: 6px; }
button { background: white; font-weight: 800; }
button.active, form button { background: #2457d6; color: white; border-color: #2457d6; }
nav { display: flex; gap: 8px; flex-wrap: wrap; }
.workspace { display: grid; grid-template-columns: minmax(260px, 1fr) minmax(260px, .8fr); gap: 14px; }
.list { display: grid; }
.row, .empty, article { display: flex; align-items: center; gap: 12px; padding: 16px; background: white; border: 1px solid #d8deea; border-radius: 8px; }
article { display: grid; align-items: start; }
.row button { flex: 1; border: 0; background: transparent; text-align: left; }
select, textarea { padding: 10px; border: 1px solid #c7d0df; border-radius: 6px; }
.done { text-decoration: line-through; }
.error { color: #b42318; }
.high { color: #b42318; } .medium { color: #b54708; } .low { color: #027a48; }
.note { padding: 10px; background: #f2f5fa; border-radius: 6px; color: #172033; }
</style>
"##;

fn react_canvas_game_template(game: GameKind) -> String {
    format!(
        r##""use client";

import {{ useEffect, useRef, useState }} from "react";

const MODE = "{mode}" as "invaders" | "breakout" | "blocks";
const WIDTH = 900;
const HEIGHT = 620;

type Shot = {{ x: number; y: number; vy: number; from: "player" | "enemy" }};
type Foe = {{ x: number; y: number; alive: boolean; hue: number }};
type Brick = {{ x: number; y: number; alive: boolean; hue: number }};
type Block = {{ x: number; y: number; hue: number }};

export default function App() {{
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [hud, setHud] = useState({{ score: 0, lives: 3, level: 1, state: "READY" }});
  const [seed, setSeed] = useState(0);

  useEffect(() => {{
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const keys = new Set<string>();
    let raf = 0;
    let last = performance.now();
    let tick = 0;
    let score = 0;
    let lives = 3;
    let level = 1;
    let running = true;
    let player = {{ x: WIDTH / 2, y: HEIGHT - 54, w: 72, h: 16 }};
    let ball = {{ x: WIDTH / 2, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }};
    let shots: Shot[] = [];
    let foes: Foe[] = Array.from({{ length: 30 }}, (_, i) => ({{
      x: 95 + (i % 10) * 74,
      y: 86 + Math.floor(i / 10) * 56,
      alive: true,
      hue: 190 + (i % 5) * 24,
    }}));
    let bricks: Brick[] = Array.from({{ length: 45 }}, (_, i) => ({{
      x: 66 + (i % 9) * 86,
      y: 76 + Math.floor(i / 9) * 34,
      alive: true,
      hue: 170 + (i % 9) * 12,
    }}));
    let blocks: Block[] = [];
    let active: Block = {{ x: 5, y: 0, hue: 190 }};

    const sync = (state = "LIVE") => setHud({{ score, lives, level, state }});
    const reset = () => {{
      score = 0; lives = 3; level = 1; tick = 0; shots = []; blocks = [];
      player = {{ x: WIDTH / 2, y: HEIGHT - 54, w: 72, h: 16 }};
      ball = {{ x: WIDTH / 2, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }};
      foes = foes.map((f, i) => ({{ ...f, x: 95 + (i % 10) * 74, y: 86 + Math.floor(i / 10) * 56, alive: true }}));
      bricks = bricks.map((b) => ({{ ...b, alive: true }}));
      active = {{ x: 5, y: 0, hue: 190 }};
      sync();
    }};
    reset();

    const down = (event: KeyboardEvent) => {{ keys.add(event.key.toLowerCase()); if (event.key === " ") event.preventDefault(); }};
    const up = (event: KeyboardEvent) => keys.delete(event.key.toLowerCase());
    const pointer = (event: PointerEvent) => {{
      const rect = canvas.getBoundingClientRect();
      player.x = ((event.clientX - rect.left) / rect.width) * WIDTH;
    }};
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    canvas.addEventListener("pointermove", pointer);
    canvas.addEventListener("pointerdown", pointer);

    const drawBackdrop = () => {{
      const gradient = ctx.createLinearGradient(0, 0, WIDTH, HEIGHT);
      gradient.addColorStop(0, "#06131f");
      gradient.addColorStop(0.5, "#11102a");
      gradient.addColorStop(1, "#08120c");
      ctx.fillStyle = gradient;
      ctx.fillRect(0, 0, WIDTH, HEIGHT);
      for (let i = 0; i < 80; i++) {{
        const x = (i * 97 + tick * (0.4 + (i % 5) * 0.12)) % WIDTH;
        const y = (i * 53 + tick * (0.7 + (i % 7) * 0.1)) % HEIGHT;
        ctx.fillStyle = `hsla(${{180 + (i % 8) * 20}}, 90%, 70%, ${{0.16 + (i % 3) * 0.07}})`;
        ctx.fillRect(x, y, 2, 2);
      }}
    }};

    const drawPlayer = () => {{
      ctx.shadowColor = "#22d3ee";
      ctx.shadowBlur = 20;
      ctx.fillStyle = "#67e8f9";
      ctx.fillRect(player.x - player.w / 2, player.y, player.w, player.h);
      ctx.fillStyle = "#fde047";
      ctx.fillRect(player.x - 9, player.y - 18, 18, 18);
      ctx.shadowBlur = 0;
    }};

    const frame = (now: number) => {{
      const dt = Math.min(32, now - last);
      last = now;
      tick += dt / 16;
      drawBackdrop();
      if (!running) return;

      if (keys.has("arrowleft") || keys.has("a")) player.x -= 8;
      if (keys.has("arrowright") || keys.has("d")) player.x += 8;
      player.x = Math.max(42, Math.min(WIDTH - 42, player.x));

      if (MODE === "breakout") {{
        ball.x += ball.vx; ball.y += ball.vy;
        if (ball.x < ball.r || ball.x > WIDTH - ball.r) ball.vx *= -1;
        if (ball.y < 34) ball.vy *= -1;
        if (ball.y > HEIGHT) {{ lives -= 1; ball = {{ x: player.x, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }}; }}
        if (Math.abs(ball.x - player.x) < player.w / 2 && ball.y > player.y - 10 && ball.y < player.y + 22) {{
          ball.vy = -Math.abs(ball.vy) - 0.1; ball.vx += (ball.x - player.x) * 0.035;
        }}
        bricks.forEach((brick) => {{
          if (!brick.alive) return;
          if (ball.x > brick.x && ball.x < brick.x + 70 && ball.y > brick.y && ball.y < brick.y + 22) {{
            brick.alive = false; ball.vy *= -1; score += 80;
          }}
          ctx.fillStyle = `hsl(${{brick.hue}}, 88%, 58%)`;
          ctx.fillRect(brick.x, brick.y, 70, 22);
        }});
        ctx.fillStyle = "#fef08a";
        ctx.beginPath(); ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2); ctx.fill();
        if (bricks.every((b) => !b.alive)) {{ level += 1; bricks.forEach((b) => b.alive = true); ball.vy -= 0.5; }}
      }} else if (MODE === "blocks") {{
        if (keys.has("arrowleft") || keys.has("a")) active.x = Math.max(0, active.x - 0.12);
        if (keys.has("arrowright") || keys.has("d")) active.x = Math.min(9, active.x + 0.12);
        active.y += keys.has("arrowdown") ? 0.18 : 0.055 + level * 0.004;
        if (active.y >= 17 || blocks.some((b) => Math.round(b.x) === Math.round(active.x) && Math.round(b.y) === Math.round(active.y + 1))) {{
          blocks.push({{ x: Math.round(active.x), y: Math.floor(active.y), hue: active.hue }});
          active = {{ x: 4 + Math.floor(Math.random() * 2), y: 0, hue: 170 + Math.random() * 90 }};
          score += 35;
        }}
        for (let y = 17; y >= 0; y--) {{
          const row = blocks.filter((b) => b.y === y);
          if (row.length >= 10) {{ blocks = blocks.filter((b) => b.y !== y).map((b) => b.y < y ? {{ ...b, y: b.y + 1 }} : b); score += 420; level += 1; }}
        }}
        [...blocks, active].forEach((b) => {{
          ctx.fillStyle = `hsl(${{b.hue}}, 90%, 58%)`;
          ctx.fillRect(246 + Math.round(b.x) * 38, 42 + Math.round(b.y) * 30, 34, 26);
        }});
        if (blocks.some((b) => b.y <= 0)) lives = 0;
      }} else {{
        if ((keys.has(" ") || keys.has("arrowup") || keys.has("w")) && tick % 8 < 1) shots.push({{ x: player.x, y: player.y - 14, vy: -9, from: "player" }});
        foes.forEach((foe) => {{ if (foe.alive) foe.x += Math.sin(tick / 22 + foe.y) * 0.7; }});
        if (Math.random() < 0.035 + level * 0.004) {{
          const live = foes.filter((f) => f.alive);
          const f = live[Math.floor(Math.random() * live.length)];
          if (f) shots.push({{ x: f.x, y: f.y + 20, vy: 5 + level * 0.2, from: "enemy" }});
        }}
        shots = shots.map((s) => ({{ ...s, y: s.y + s.vy }})).filter((s) => s.y > -20 && s.y < HEIGHT + 24);
        shots.forEach((shot) => {{
          if (shot.from === "player") foes.forEach((foe) => {{
            if (foe.alive && Math.abs(shot.x - foe.x) < 25 && Math.abs(shot.y - foe.y) < 22) {{ foe.alive = false; shot.y = -60; score += 120; }}
          }});
          if (shot.from === "enemy" && Math.abs(shot.x - player.x) < 34 && Math.abs(shot.y - player.y) < 24) {{ shot.y = HEIGHT + 80; lives -= 1; }}
        }});
        foes.forEach((foe) => {{ if (!foe.alive) return; ctx.fillStyle = `hsl(${{foe.hue}}, 90%, 62%)`; ctx.fillRect(foe.x - 22, foe.y - 14, 44, 28); }});
        shots.forEach((s) => {{ ctx.fillStyle = s.from === "player" ? "#fde047" : "#fb7185"; ctx.fillRect(s.x - 3, s.y - 10, 6, 18); }});
        if (foes.every((f) => !f.alive)) {{ level += 1; foes.forEach((f) => {{ f.alive = true; f.y += 10; }}); }}
      }}

      drawPlayer();
      if (lives <= 0) {{ running = false; sync("GAME OVER"); }} else sync();
      raf = requestAnimationFrame(frame);
    }};

    raf = requestAnimationFrame(frame);
    return () => {{
      running = false; cancelAnimationFrame(raf);
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      canvas.removeEventListener("pointermove", pointer);
      canvas.removeEventListener("pointerdown", pointer);
    }};
  }}, [seed]);

  return (
    <main className="min-h-screen bg-[#050711] px-5 py-6 text-white">
      <section className="mx-auto flex max-w-6xl flex-col gap-4">
        <div className="flex flex-wrap items-end justify-between gap-4 border-b border-cyan-300/25 pb-4">
          <div>
            <p className="text-sm font-bold uppercase tracking-[0.24em] text-cyan-200">{subtitle}</p>
            <h1 className="text-4xl font-black uppercase text-cyan-100 sm:text-6xl">{title}</h1>
          </div>
          <div className="grid grid-cols-4 gap-3 text-right text-xs uppercase text-cyan-100 sm:text-sm">
            <span>Score<br /><b className="text-2xl text-yellow-300">{{hud.score}}</b></span>
            <span>Lives<br /><b className="text-2xl text-rose-300">{{hud.lives}}</b></span>
            <span>Level<br /><b className="text-2xl text-violet-300">{{hud.level}}</b></span>
            <span>Status<br /><b className="text-2xl text-emerald-300">{{hud.state}}</b></span>
          </div>
        </div>
        <div className="relative overflow-hidden rounded border border-cyan-300/30 bg-black shadow-[0_0_42px_rgba(34,211,238,0.25)]">
          <canvas ref={{canvasRef}} width={{WIDTH}} height={{HEIGHT}} className="aspect-[90/62] w-full touch-none" />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3 text-sm text-cyan-100/80">
          <span>Move: Arrow keys or A/D. Action: Space, W, Up, or pointer.</span>
          <button onClick={{() => setSeed((value) => value + 1)}} className="border border-cyan-300/50 px-4 py-2 font-bold uppercase text-cyan-100 hover:bg-cyan-300/10">Restart</button>
        </div>
      </section>
    </main>
  );
}}
"##,
        mode = game.mode(),
        title = game.title(),
        subtitle = game.subtitle()
    )
}

fn vue_canvas_game_template(game: GameKind) -> String {
    format!(
        r##"<template>
  <main class="shell">
    <section class="hud">
      <div>
        <p>{subtitle}</p>
        <h1>{title}</h1>
      </div>
      <div class="stats">
        <span>Score <b>{{{{ score }}}}</b></span>
        <span>Lives <b>{{{{ lives }}}}</b></span>
        <span>Level <b>{{{{ level }}}}</b></span>
        <span>Status <b>{{{{ state }}}}</b></span>
      </div>
    </section>
    <canvas ref="canvas" width="900" height="620" @pointermove="aim" @pointerdown="aim" />
    <footer>
      <span>Move: Arrow keys or A/D. Action: Space, W, Up, or pointer.</span>
      <button @click="restart">Restart</button>
    </footer>
  </main>
</template>

<script setup lang="ts">
import {{ onBeforeUnmount, onMounted, ref }} from 'vue';

const mode = '{mode}';
const canvas = ref<HTMLCanvasElement | null>(null);
const score = ref(0);
const lives = ref(3);
const level = ref(1);
const state = ref('READY');
let raf = 0;
let tick = 0;
let player = {{ x: 450, y: 566, w: 78 }};
let ball = {{ x: 450, y: 506, vx: 4.5, vy: -5.5, r: 8 }};
let keys = new Set<string>();
let cells: Array<{{ x: number; y: number; hue: number }}> = [];

function sync(next = 'LIVE') {{ state.value = next; }}
function restart() {{ score.value = 0; lives.value = 3; level.value = 1; tick = 0; cells = []; ball = {{ x: 450, y: 506, vx: 4.5, vy: -5.5, r: 8 }}; sync(); }}
function aim(event: PointerEvent) {{ const rect = (event.target as HTMLCanvasElement).getBoundingClientRect(); player.x = ((event.clientX - rect.left) / rect.width) * 900; }}
function down(event: KeyboardEvent) {{ keys.add(event.key.toLowerCase()); if (event.key === ' ') event.preventDefault(); }}
function up(event: KeyboardEvent) {{ keys.delete(event.key.toLowerCase()); }}

function draw(ctx: CanvasRenderingContext2D) {{
  tick += 1;
  const g = ctx.createLinearGradient(0, 0, 900, 620);
  g.addColorStop(0, '#06131f'); g.addColorStop(0.5, '#14102e'); g.addColorStop(1, '#07140f');
  ctx.fillStyle = g; ctx.fillRect(0, 0, 900, 620);
  for (let i = 0; i < 80; i++) {{ ctx.fillStyle = `hsla(${{170 + i * 7}}, 90%, 70%, .22)`; ctx.fillRect((i * 89 + tick) % 900, (i * 47 + tick * .8) % 620, 2, 2); }}
  if (keys.has('arrowleft') || keys.has('a')) player.x -= 8;
  if (keys.has('arrowright') || keys.has('d')) player.x += 8;
  player.x = Math.max(42, Math.min(858, player.x));
  if (mode === 'blocks') {{
    const active = {{ x: Math.floor((tick / 18) % 10), y: Math.floor((tick / 18) % 17), hue: 190 + (tick % 80) }};
    if (tick % 40 === 0) cells.push(active);
    cells = cells.slice(-120);
    cells.concat(active).forEach((b) => {{ ctx.fillStyle = `hsl(${{b.hue}}, 90%, 58%)`; ctx.fillRect(250 + b.x * 38, 44 + b.y * 30, 34, 26); }});
    score.value += tick % 90 === 0 ? 120 : 0;
  }} else {{
    ball.x += ball.vx; ball.y += ball.vy;
    if (ball.x < 12 || ball.x > 888) ball.vx *= -1;
    if (ball.y < 30) ball.vy *= -1;
    if (Math.abs(ball.x - player.x) < player.w / 2 && ball.y > 548 && ball.y < 588) {{ ball.vy = -Math.abs(ball.vy); score.value += 15; }}
    if (ball.y > 620) {{ lives.value -= 1; ball = {{ x: player.x, y: 506, vx: 4.5, vy: -5.5, r: 8 }}; }}
    for (let i = 0; i < 36; i++) {{ const x = 80 + (i % 9) * 84; const y = 74 + Math.floor(i / 9) * 34; ctx.fillStyle = `hsl(${{170 + i * 9}}, 86%, 58%)`; ctx.fillRect(x, y, 66, 22); }}
    ctx.fillStyle = '#fde047'; ctx.beginPath(); ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2); ctx.fill();
  }}
  ctx.fillStyle = '#67e8f9'; ctx.fillRect(player.x - player.w / 2, player.y, player.w, 16);
  if (lives.value <= 0) sync('GAME OVER'); else sync();
  raf = requestAnimationFrame(() => draw(ctx));
}}

onMounted(() => {{
  const ctx = canvas.value?.getContext('2d');
  if (!ctx) return;
  restart();
  window.addEventListener('keydown', down);
  window.addEventListener('keyup', up);
  raf = requestAnimationFrame(() => draw(ctx));
}});
onBeforeUnmount(() => {{ cancelAnimationFrame(raf); window.removeEventListener('keydown', down); window.removeEventListener('keyup', up); }});
</script>

<style scoped>
.shell {{ min-height: 100vh; background: #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }}
.hud, footer {{ max-width: 1120px; margin: 0 auto 16px; display: flex; justify-content: space-between; gap: 16px; flex-wrap: wrap; }}
p {{ color: #a5f3fc; text-transform: uppercase; letter-spacing: .18em; font-weight: 800; }}
h1 {{ margin: 0; color: #cffafe; font-size: clamp(36px, 7vw, 72px); }}
.stats {{ display: grid; grid-template-columns: repeat(4, minmax(70px, 1fr)); gap: 12px; text-align: right; text-transform: uppercase; }}
b {{ display: block; color: #fde047; font-size: 24px; }}
canvas {{ display: block; max-width: 1120px; width: 100%; aspect-ratio: 90 / 62; margin: 0 auto; border: 1px solid rgba(103,232,249,.35); background: black; touch-action: none; box-shadow: 0 0 42px rgba(34,211,238,.25); }}
button {{ border: 1px solid rgba(103,232,249,.55); background: transparent; color: #cffafe; padding: 10px 16px; font-weight: 800; text-transform: uppercase; }}
</style>
"##,
        mode = game.mode(),
        title = game.title(),
        subtitle = game.subtitle()
    )
}

pub(super) fn first_existing_impl_target(work_root: &Path) -> Option<PathBuf> {
    [
        "app/page.tsx",
        "src/app/page.tsx",
        "src/App.tsx",
        "src/App.jsx",
        "src/main.tsx",
        "src/main.jsx",
        "app.vue",
        "app/app.vue",
        "pages/index.vue",
        "src/pages/index.vue",
    ]
    .iter()
    .find_map(|relative| {
        scaffold_search_roots(work_root)
            .into_iter()
            .map(|root| root.join(relative))
            .find(|candidate| candidate.is_file())
    })
}

fn count_any(normalized: &str, markers: &[&str]) -> usize {
    markers
        .iter()
        .filter(|marker| normalized.contains(**marker))
        .count()
}

fn placeholder_markers_for(framework: FrameworkKind) -> &'static [&'static str] {
    match framework {
        FrameworkKind::Next => &[
            "next.svg",
            "vercel",
            "documentation",
            "create next app",
            "local-first coding agent",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::React => &[
            "react.svg",
            "vite.svg",
            "learn react",
            "vite + react",
            "click on the vite and react logos",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::Nuxt => &[
            "nuxt",
            "welcome",
            "documentation",
            "deploy",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::SvelteKit => &[
            "svelte",
            "welcome",
            "documentation",
            "create-svelte",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::Unknown => &[
            "documentation",
            "welcome",
            "start experience",
            "core interaction",
        ],
    }
}

fn placeholder_hit_count(framework: FrameworkKind, normalized_content: &str) -> usize {
    let marker_groups = [
        placeholder_markers_for(framework),
        placeholder_markers_for(FrameworkKind::Next),
        placeholder_markers_for(FrameworkKind::React),
        placeholder_markers_for(FrameworkKind::Nuxt),
        placeholder_markers_for(FrameworkKind::SvelteKit),
        placeholder_markers_for(FrameworkKind::Unknown),
    ];
    marker_groups
        .iter()
        .flat_map(|markers| markers.iter())
        .filter(|token| normalized_content.contains(**token))
        .count()
}

fn scaffold_search_roots(work_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![work_root.to_path_buf()];
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return roots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | ".anvil-state" | "node_modules" | "target")
        ) {
            continue;
        }
        if is_nested_project_root(&path) {
            roots.push(path);
        }
    }
    roots
}

fn is_nested_project_root(path: &Path) -> bool {
    path.join("package.json").is_file()
        || path.join("next.config.ts").is_file()
        || path.join("next.config.js").is_file()
        || path.join("nuxt.config.ts").is_file()
        || path.join("nuxt.config.js").is_file()
        || path.join("vite.config.ts").is_file()
        || path.join("vite.config.js").is_file()
        || path.join("app/page.tsx").is_file()
        || path.join("src/app/page.tsx").is_file()
        || path.join("src/App.tsx").is_file()
        || path.join("src/App.jsx").is_file()
        || path.join("app.vue").is_file()
        || path.join("app/app.vue").is_file()
        || path.join("pages/index.vue").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_profile_classifies_abstract_business_primitives() {
        let profile = FeatureProfile::from_request(
            "売上シミュレーターを入力値の異常値チェック、目標達成判定、グラフ風表示、保存付きで改善して下さい。",
        );
        assert!(profile.validation);
        assert!(profile.calculation);
        assert!(profile.visualization);
        assert!(profile.persistence);
        assert!(profile.requires_semantic_business_slice());
    }

    #[test]
    fn fast_polish_fallback_is_limited_to_safe_quality_edits() {
        assert!(request_allows_fast_polish_fallback(
            "1つ目の管理画面に、入力ミスを防ぐバリデーションと月別サマリーを追加して品質を上げて下さい。"
        ));
        assert!(!request_allows_fast_polish_fallback(
            "既存の認証フローをOAuth連携に置き換えて下さい。"
        ));
        assert!(!request_allows_fast_polish_fallback(
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。"
        ));
    }

    #[test]
    fn deterministic_framework_fallback_does_not_claim_non_ts_or_non_coding_work() {
        assert!(
            deterministic_empty_framework_app_files(
                "PythonでCSVを読み込んでカテゴリ別合計を出すCLIを作って下さい。"
            )
            .is_none()
        );
        let svelte_files = deterministic_empty_framework_app_files(
            "SvelteKitで予約フォームアプリを作って下さい。入力チェックと保存も入れて下さい。",
        )
        .expect("sveltekit files");
        assert!(
            svelte_files
                .iter()
                .any(|(path, _)| path.ends_with("src/routes/+page.svelte"))
        );
        assert!(!request_needs_playable_ui_quality_gate(
            "READMEをわかりやすく改善してください"
        ));
        assert!(!request_allows_fast_polish_fallback(
            "既存のPython CLIにCSV出力を追加して品質を上げて下さい。"
        ));
    }

    #[test]
    fn unsupported_ui_framework_detection_is_explicit() {
        assert!(!request_mentions_unsupported_ui_framework(
            "SvelteKitで小さなメモアプリを作って下さい。"
        ));
        assert!(request_mentions_unsupported_ui_framework(
            "Astroでドキュメントサイトを作って下さい。"
        ));
        assert!(!request_mentions_unsupported_ui_framework(
            "Next.jsで小さなメモアプリを作って下さい。"
        ));
    }

    #[test]
    fn deterministic_python_cli_fallback_is_narrow() {
        let files =
            deterministic_empty_python_cli_files("PythonでCSVを集計するCLIを作成してください。")
                .expect("python cli files");
        let paths = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"analyze_csv.py".to_string()));
        assert!(paths.contains(&"sample.csv".to_string()));
        assert!(
            deterministic_empty_python_cli_files(
                "PythonではなくNext.jsでCSVを表示するUIアプリを作成してください。"
            )
            .is_none()
        );
    }

    #[test]
    fn deterministic_python_cli_accepts_safe_project_names() {
        let files = deterministic_empty_python_cli_files_with_names(
            "PythonでCSVを集計するCLIを作成してください。",
            Some("project_csv_tool.py"),
            Some("example.csv"),
        )
        .expect("python cli files");
        assert!(
            files
                .iter()
                .any(|(path, _)| path == Path::new("project_csv_tool.py"))
        );
        assert!(
            files
                .iter()
                .any(|(path, _)| path == Path::new("example.csv"))
        );
        let readme = files
            .iter()
            .find(|(path, _)| path == Path::new("README.md"))
            .map(|(_, content)| content)
            .expect("README");
        assert!(readme.contains("python3 project_csv_tool.py example.csv"));
    }

    #[test]
    fn deterministic_docs_fallback_rejects_answer_only() {
        assert!(
            deterministic_empty_docs_files("READMEを作成してください。")
                .expect("docs files")
                .iter()
                .any(|(path, _)| path == Path::new("README.md"))
        );
        assert!(
            deterministic_empty_docs_files(
                "READMEを要約してください。ファイルは変更しないでください。"
            )
            .is_none()
        );
    }

    #[test]
    fn deterministic_fallback_replaces_next_placeholder_game() {
        let request =
            "最高に面白くかっこいいシューティングゲームをnext.jsアプリとして開発してください";
        let current = r#"import Image from "next/image";
export default function Home() {
  return <a href="https://nextjs.org/docs">Documentation</a>;
}
"#;
        let output =
            deterministic_playable_ui_fallback(request, Path::new("src/app/page.tsx"), current)
                .expect("fallback");
        assert!(output.contains("\"use client\""));
        assert!(output.contains("NEON INVADERS"));
        assert!(output.contains("useState"));
        assert!(output.contains("pointermove"));
        assert!(!output.contains("next.svg"));
    }

    #[test]
    fn deterministic_fallback_replaces_cross_framework_scaffold_game() {
        let request =
            "最高に面白くかっこいいブロック崩しゲームをreact.jsアプリとして開発してください";
        let current = r#"import Image from "next/image";
export default function Home() {
  return <a href="https://vercel.com/templates">Templates</a>;
}
"#;
        let output =
            deterministic_playable_ui_fallback(request, Path::new("src/app/page.tsx"), current)
                .expect("fallback");
        assert!(output.contains("NEON BREAKOUT"));
        assert!(output.contains("requestAnimationFrame"));
    }

    #[test]
    fn deterministic_fallback_replaces_nuxt_placeholder_game() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = "<template><NuxtWelcome /><a>Documentation</a></template>";
        let output = deterministic_playable_ui_fallback(request, Path::new("app.vue"), current)
            .expect("fallback");
        assert!(output.contains("<script setup"));
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("canvas"));
    }

    #[test]
    fn deterministic_fallback_replaces_nuxt_shell_with_missing_component() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div id="tetris-app">
    <NuxtRouteAnnouncer />
    <TetrisGame />
  </div>
</template>

<script setup lang="ts">
import TetrisGame from '~/components/TetrisGame.vue'
</script>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected incomplete shell to fail quality gate");
        assert!(
            issue.contains("interactive vertical slice")
                || issue.contains("placeholder markers")
                || issue.contains("marker spam"),
            "got: {issue}"
        );
        let output = deterministic_playable_ui_fallback(request, Path::new("app/app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(!output.contains("TetrisGame"));
    }

    #[test]
    fn deterministic_fallback_replaces_malformed_nuxt_placeholder_partial() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div class="tetris-container">
    <h1>テトリス</h1>
    <button @click="startGame">スタート</button>
  </div>
</template>
  <div>
    <NuxtRouteAnnouncer />
    <NuxtWelcome />
  </div>
</template>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected malformed scaffold partial to fail quality gate");
        assert!(issue.contains("interactive vertical slice") || issue.contains("placeholder"));
        let output = deterministic_playable_ui_fallback(request, Path::new("app/app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(!output.contains("NuxtWelcome"));
    }

    #[test]
    fn deterministic_fallback_replaces_low_fidelity_nuxt_tetris() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div class="tetris-container">
    <h1>Tetris</h1>
    <div class="game-board">
      <div v-for="(row, y) in board" :key="y" class="row"></div>
    </div>
    <button @click="startGame">Start / Restart</button>
    <div class="score">Score: {{ score }}</div>
  </div>
</template>

<script setup>
import { ref, onMounted } from 'vue'
const board = ref([])
const score = ref(0)
const currentPiece = ref(null)
function startGame() { score.value = 0; currentPiece.value = {} }
onMounted(() => window.addEventListener('keydown', () => {}))
</script>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected low fidelity game slice to fail");
        assert!(issue.contains("low-fidelity game slice"), "got: {issue}");
        let output = deterministic_playable_ui_fallback(request, Path::new("app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(output.contains("canvas"));
    }

    #[test]
    fn deterministic_empty_framework_game_files_materialize_nuxt_canvas_game() {
        let request = "最高に面白くかっこいいテトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください";
        let files = deterministic_empty_framework_game_files(request).expect("files");
        let paths = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                "package.json",
                "nuxt.config.ts",
                "scripts/smoke-test.mjs",
                "app.vue"
            ]
        );
        let package = files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("package");
        assert!(package.contains("nuxt dev -p 3011"));
        assert!(package.contains(r#""test": "node scripts/smoke-test.mjs""#));
        assert!(package.contains(r#""audit": "npm audit --audit-level=critical""#));
        assert!(package.contains(r#""nuxt": "3.13.2""#));
        assert!(package.contains(r#""typescript": "5.4.5""#));
        let app = files
            .iter()
            .find(|(path, _)| path == Path::new("app.vue"))
            .map(|(_, content)| content)
            .expect("app");
        assert!(app.contains("NEON BLOCKS"));
        assert!(app.contains("requestAnimationFrame"));
        assert!(app.contains("keydown"));
    }

    #[test]
    fn deterministic_empty_framework_game_files_classifies_falling_puzzle_terms() {
        let request = "Nuxt.jsで落ち物パズルゲームを作って下さい。起動ポートは3011にして下さい。";
        let files = deterministic_empty_framework_game_files(request).expect("files");
        let app = files
            .iter()
            .find(|(path, _)| path == Path::new("app.vue"))
            .map(|(_, content)| content)
            .expect("app");
        assert!(app.contains("NEON BLOCKS"));
        assert!(app.contains("const mode = 'blocks'"));
        assert!(!app.contains("NEON INVADERS"));
    }

    #[test]
    fn deterministic_empty_framework_app_files_materialize_next_task_manager() {
        let request = "Next.jsで小さなタスク管理アプリを作って下さい。追加、完了切替、フィルタ、件数表示を入れ、起動ポートは3011にして下さい。";
        let files = deterministic_empty_framework_app_files(request).expect("files");
        let page = files
            .iter()
            .find(|(path, _)| path == Path::new("src/app/page.tsx"))
            .map(|(_, content)| content)
            .expect("page");
        assert!(page.contains("タスク管理アプリ"));
        assert!(page.contains("Score history"));
        assert!(page.contains("recordLap"));
        assert!(page.contains("finishRun"));
        assert!(page.contains("Best"));
        assert!(page.contains("localStorage"));
        assert!(files.iter().any(
            |(path, content)| path == Path::new("scripts/smoke-test.mjs")
                && content.contains("localStorage")
                && content.contains("app/page.tsx")
        ));
    }

    #[test]
    fn deterministic_empty_framework_app_files_accepts_generic_app_wording() {
        let next_files = deterministic_empty_framework_app_files(
            "Next.jsでタイピング練習アプリを作って下さい。ポートは3007で起動できるようにして下さい。",
        )
        .expect("next files");
        assert!(
            next_files.iter().any(
                |(path, content)| path == Path::new("package.json") && content.contains("3007")
            )
        );
        assert!(
            next_files
                .iter()
                .any(|(path, content)| path == Path::new("src/app/page.tsx")
                    && content.contains("タイピング練習アプリ")
                    && content.contains("Score history")
                    && content.contains("localStorage")
                    && content.contains("Best"))
        );

        let punctuation_files = deterministic_empty_framework_app_files(
            "Next.jsで、毎日の気分とメモを残せる小さな記録アプリを作って下さい。3011で起動してください。",
        )
        .expect("punctuation files");
        assert!(punctuation_files.iter().any(|(path, content)| {
            path == Path::new("src/app/page.tsx")
                && content.contains("毎日の気分とメモを残せる小さな記録アプリ")
        }));

        let nuxt_files = deterministic_empty_framework_app_files(
            "Nuxt.jsでストップウォッチ兼ラップ記録アプリを作って下さい。起動ポート: 3009。",
        )
        .expect("nuxt files");
        assert!(
            nuxt_files.iter().any(
                |(path, content)| path == Path::new("package.json") && content.contains("3009")
            )
        );
        assert!(
            nuxt_files
                .iter()
                .any(|(path, content)| path == Path::new("app.vue")
                    && content.contains("ストップウォッチ兼ラップ記録アプリ")
                    && content.contains("Score history")
                    && content.contains("Best"))
        );
    }

    #[test]
    fn deterministic_empty_framework_app_files_materialize_react_work_board() {
        let request = "React.jsで業務管理ボードを作って下さい。検索、ステータス変更、優先度表示を入れ、起動ポートは3011にして下さい。";
        let files = deterministic_empty_framework_app_files(request).expect("files");
        let app = files
            .iter()
            .find(|(path, _)| path == Path::new("src/App.tsx"))
            .map(|(_, content)| content)
            .expect("app");
        assert!(app.contains("Practice Console"));
        assert!(app.contains("Score history"));
        assert!(app.contains("recordLap"));
        assert!(app.contains("averageScore"));
        assert!(app.contains("Calculated total"));
        assert!(app.contains("targetMet"));
        assert!(app.contains("aria-live"));
        assert!(app.contains("localStorage"));
        assert!(
            files
                .iter()
                .any(|(path, content)| path == Path::new("package.json")
                    && content.contains(r#""audit": "npm audit --audit-level=critical""#))
        );
        assert!(
            files
                .iter()
                .any(|(path, _)| path == Path::new("scripts/dev.mjs"))
        );
        assert!(
            files
                .iter()
                .any(|(path, _)| path == Path::new("scripts/smoke-test.mjs"))
        );
    }

    #[test]
    fn deterministic_empty_framework_app_files_materialize_nuxt_work_board() {
        let request = "Nuxt.jsで管理画面を作って下さい。一覧、選択詳細、メモ追加、入力バリデーションを入れ、起動ポートは3011にして下さい。";
        let files = deterministic_empty_framework_app_files(request).expect("files");
        let app = files
            .iter()
            .find(|(path, _)| path == Path::new("app.vue"))
            .map(|(_, content)| content)
            .expect("app");
        assert!(app.contains("Practice Console"));
        assert!(app.contains("Score history"));
        assert!(app.contains("recordLap"));
        assert!(app.contains("averageScore"));
        assert!(app.contains("Calculated total"));
        assert!(app.contains("targetMet"));
        assert!(app.contains("aria-live"));
        assert!(app.contains("localStorage"));
    }

    #[test]
    fn deterministic_fallback_replaces_business_scaffold_placeholder() {
        let request = "React.jsで業務管理ボードを作って下さい。検索、ステータス変更、優先度表示を入れ、起動ポートは3011にして下さい。";
        let current = "export default function App() { return <h1>Get started</h1>; }";
        let output = deterministic_playable_ui_fallback(request, Path::new("src/App.tsx"), current)
            .expect("fallback");
        assert!(output.contains("Practice Console"));
        assert!(output.contains("Score history"));
        assert!(output.contains("recordLap"));
        assert!(!output.contains("NEON"));
    }

    #[test]
    fn deterministic_polish_handles_quality_improvement_wording() {
        let request = "1つ目のタスク管理アプリに、空状態、エラー表示、キーボード操作を追加して品質を上げて下さい。";
        let output = deterministic_playable_ui_polish_fallback(
            request,
            Path::new("src/app/page.tsx"),
            REACT_TASK_MANAGER_TEMPLATE,
        )
        .expect("polish");
        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("Search work items"));
        assert!(output.contains("Add memo"));
    }

    #[test]
    fn deterministic_polish_accepts_generic_improvement_without_app_word() {
        let current = react_business_app_template(
            "Next.jsで売上シミュレーターを作って下さい。単価、数量、割引率を入力し、合計と粗利をグラフ風に表示して下さい。",
        );
        assert!(request_is_playable_ui_improvement(
            "1つ目の売上シミュレーターを、入力値の異常値チェックと目標達成判定付きに改善して下さい。"
        ));
        let output = deterministic_playable_ui_polish_fallback(
            "1つ目の売上シミュレーターを、入力値の異常値チェックと目標達成判定付きに改善して下さい。",
            Path::new("src/app/page.tsx"),
            &current,
        )
        .expect("polish");
        assert!(output.contains("data-anvil-polish=\"v1\""));
    }

    #[test]
    fn deterministic_polish_adds_business_summary_and_validation() {
        let request = "1つ目の管理画面に、月別サマリーと入力ミスを防ぐバリデーションを追加して下さい。既存の画面構成は大きく変えないで下さい。";
        let output = deterministic_playable_ui_polish_fallback(
            request,
            Path::new("src/App.tsx"),
            REACT_TASK_MANAGER_TEMPLATE,
        )
        .expect("polish");
        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("Monthly Summary"));
        assert!(output.contains("monthlySummary"));
        assert!(output.contains("validateTaskTitle"));
        assert!(output.contains("Operations Board"));
        assert!(output.contains("Search work items"));
        assert!(output.contains("addMemo"));
    }

    #[test]
    fn deterministic_empty_framework_game_files_pin_stable_next_and_vite_dependencies() {
        let next_files = deterministic_empty_framework_game_files(
            "シューティングゲームを3011ポートで起動可能なNext.jsアプリとして開発してください",
        )
        .expect("next files");
        let next_package = next_files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("next package");
        assert!(next_package.contains(r#""next": "14.2.35""#));
        assert!(next_package.contains(r#""react": "18.2.0""#));
        assert!(next_package.contains(r#""audit": "npm audit --audit-level=critical""#));
        assert!(!next_package.contains(r#""latest""#));
        assert!(
            next_files
                .iter()
                .any(|(path, _)| path == Path::new("scripts/smoke-test.mjs"))
        );

        let react_files = deterministic_empty_framework_game_files(
            "ボール崩しゲームを3011ポートで起動可能なReact.jsアプリとして開発してください",
        )
        .expect("react files");
        let react_package = react_files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("react package");
        let react_dev_script = react_files
            .iter()
            .find(|(path, _)| path == Path::new("scripts/dev.mjs"))
            .map(|(_, content)| content)
            .expect("react dev script");
        assert!(react_package.contains(r#""vite": "5.4.11""#));
        assert!(react_package.contains(r#""@vitejs/plugin-react": "4.3.4""#));
        assert!(react_package.contains(r#""dev": "node scripts/dev.mjs""#));
        assert!(react_package.contains(r#""test": "node scripts/smoke-test.mjs""#));
        assert!(react_package.contains(r#""audit": "npm audit --audit-level=critical""#));
        assert!(react_dev_script.contains(r#"arg === "-p""#));
        assert!(react_dev_script.contains(r#"viteArgs.push("--port", "3011")"#));
        assert!(!react_package.contains(r#""latest""#));
    }

    #[test]
    fn deterministic_fallback_ignores_non_placeholder_canvas_content() {
        let request =
            "最高に面白くかっこいいボール崩しゲームをReact.jsアプリとして開発してください";
        let current = r#"
"use client";
import { useEffect, useRef, useState } from "react";
export default function App(){
  const canvasRef = useRef(null);
  const [score, setScore] = useState(0);
  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    const down = () => setScore((value) => value + 1);
    window.addEventListener("keydown", down);
    let raf = 0;
    const frame = () => { ctx?.fillRect(0, 0, 10, 10); raf = requestAnimationFrame(frame); };
    raf = requestAnimationFrame(frame);
    return () => { cancelAnimationFrame(raf); window.removeEventListener("keydown", down); };
  }, []);
  return <><canvas ref={canvasRef} /><button onClick={() => setScore(0)}>Restart {score}</button></>;
}
"#;
        assert!(
            deterministic_playable_ui_fallback(request, Path::new("src/App.tsx"), current)
                .is_none()
        );
    }

    #[test]
    fn deterministic_polish_improves_existing_react_game_without_replacing_logic() {
        let request = "シューティングゲームをよりカッコよくしてください";
        let current = react_canvas_game_template(GameKind::Invaders);
        let output =
            deterministic_playable_ui_polish_fallback(request, Path::new("src/App.tsx"), &current)
                .expect("polish output");

        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("radial-gradient(circle_at_18%_12%"));
        assert!(output.contains("requestAnimationFrame(frame)"));
        assert!(
            deterministic_playable_ui_polish_fallback(request, Path::new("src/App.tsx"), &output)
                .is_none()
        );
    }

    #[test]
    fn deterministic_polish_improves_existing_vue_game() {
        let request = "テトリスゲームをよりカッコよくしてください";
        let current = vue_canvas_game_template(GameKind::Blocks);
        let output =
            deterministic_playable_ui_polish_fallback(request, Path::new("app.vue"), &current)
                .expect("polish output");

        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("radial-gradient(circle at 18% 12%"));
        assert!(output.contains("requestAnimationFrame"));
    }

    #[test]
    fn package_json_port_fallback_updates_next_dev_script() {
        let request =
            "シューティングゲームを3011ポートで起動可能なnext.jsアプリとして開発してください";
        let package = r#"{
  "scripts": { "dev": "next dev", "build": "next build" },
  "dependencies": { "next": "16.2.4", "react": "19.2.4" }
}"#;
        let output = package_json_with_requested_port(request, package).expect("package update");
        assert!(output.contains(r#""dev": "next dev -p 3011""#));
        assert!(output.contains(r#""build": "next build""#));
    }

    #[test]
    fn package_json_port_fallback_accepts_port_before_digits() {
        let request = "React.jsでゲームを作って下さい。起動ポートは3007にして下さい。";
        let package = r#"{
  "scripts": { "dev": "vite" },
  "dependencies": { "react": "19.2.4" },
  "devDependencies": { "vite": "^7.0.0" }
}"#;
        let output = package_json_with_requested_port(request, package).expect("package update");
        assert!(output.contains(r#""dev": "node scripts/dev.mjs""#));
        let files = deterministic_empty_framework_game_files(request).expect("files");
        let dev_wrapper = files
            .iter()
            .find(|(path, _)| path == Path::new("scripts/dev.mjs"))
            .map(|(_, content)| content)
            .expect("dev wrapper");
        assert!(dev_wrapper.contains(r#"viteArgs.push("--port", "3007");"#));
    }

    #[test]
    fn package_json_port_fallback_updates_vite_and_nuxt() {
        let react_request =
            "ボール崩しゲームを3011ポートで起動可能なReact.jsアプリとして開発してください";
        let react_package = r#"{
  "scripts": { "dev": "vite" },
  "dependencies": { "react": "19.2.4" },
  "devDependencies": { "vite": "^7.0.0" }
}"#;
        let react_output =
            package_json_with_requested_port(react_request, react_package).expect("react package");
        assert!(react_output.contains(r#""dev": "node scripts/dev.mjs""#));

        let nuxt_request = "テトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください";
        let nuxt_package = r#"{
  "scripts": { "dev": "nuxt dev" },
  "dependencies": { "nuxt": "^3.13.0" }
}"#;
        let nuxt_output =
            package_json_with_requested_port(nuxt_request, nuxt_package).expect("nuxt package");
        assert!(nuxt_output.contains(r#""dev": "nuxt dev -p 3011""#));
    }
}

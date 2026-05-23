use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::ValueEnum;

use crate::cli::{CliArgs, ResumeRequest};
use crate::safety::host_validation::validate_localhost_url;

/// `EnvFilter` directive that silences DEBUG/TRACE noise from hyper/reqwest/rustls.
pub const NOISY_CRATES_FILTER: &str = "hyper_util=warn,reqwest=warn,hyper=warn,rustls=warn";

pub const DEFAULT_PHOTON_ROLLOUT_MIN_EVAL_TURNS: u32 = 100;

/// 3-level log verbosity. The `Info < Verbose < Trace` ordering is part of the
/// public contract (callers use comparisons like `log_level >= Verbose`) and
/// must not be reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum LogLevel {
    #[default]
    Info,
    Verbose,
    Trace,
}

impl LogLevel {
    /// Build the `tracing_subscriber::EnvFilter` directive for this level.
    pub fn env_filter(self) -> String {
        match self {
            LogLevel::Info => format!("info,{NOISY_CRATES_FILTER}"),
            LogLevel::Verbose => format!("debug,{NOISY_CRATES_FILTER}"),
            // Trace intentionally leaves noisy crates uncapped.
            LogLevel::Trace => "trace".to_string(),
        }
    }

    /// Map the legacy `debug=true/false` flag onto a `LogLevel`.
    pub fn from_legacy_debug(debug: bool) -> Self {
        if debug { Self::Trace } else { Self::Info }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            LogLevel::Info => "info",
            LogLevel::Verbose => "verbose",
            LogLevel::Trace => "trace",
        };
        write!(f, "{s}")
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "info" => Ok(LogLevel::Info),
            "verbose" => Ok(LogLevel::Verbose),
            "trace" => Ok(LogLevel::Trace),
            other => Err(format!("unknown log level: {other}")),
        }
    }
}

/// Controls deterministic recovery writes. Security and syntax guards remain
/// deterministic regardless of this value; this only gates product-quality
/// templates and fallback file materialization.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum DeterministicFallbackMode {
    #[value(alias = "disabled", alias = "false", alias = "0")]
    Off,
    #[value(alias = "hint")]
    HintOnly,
    #[default]
    #[value(
        alias = "support-only",
        alias = "support_only",
        alias = "support",
        alias = "minimal",
        alias = "minimal-patches",
        alias = "minimal_patches"
    )]
    MinimalPatch,
    #[value(
        alias = "full",
        alias = "enabled",
        alias = "true",
        alias = "1",
        alias = "full-templates",
        alias = "full_templates"
    )]
    FullTemplate,
}

impl DeterministicFallbackMode {
    pub fn fallback_level(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::HintOnly => "hint-only",
            Self::MinimalPatch => "minimal-patch",
            Self::FullTemplate => "full-template",
        }
    }

    pub fn allows_hint_only(self) -> bool {
        matches!(
            self,
            Self::HintOnly | Self::MinimalPatch | Self::FullTemplate
        )
    }

    pub fn allows_template_completion(self) -> bool {
        matches!(self, Self::FullTemplate)
    }

    pub fn allows_support_recovery(self) -> bool {
        matches!(self, Self::MinimalPatch | Self::FullTemplate)
    }
}

impl fmt::Display for DeterministicFallbackMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.fallback_level())
    }
}

impl FromStr for DeterministicFallbackMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "off" | "disabled" | "false" | "0" => Ok(Self::Off),
            "hint-only" | "hint" => Ok(Self::HintOnly),
            "support-only" | "support" | "minimal" | "minimal-patch" | "minimal-patches" => {
                Ok(Self::MinimalPatch)
            }
            "full" | "enabled" | "true" | "1" | "full-template" | "full-templates" => {
                Ok(Self::FullTemplate)
            }
            other => Err(format!("unknown deterministic fallback mode: {other}")),
        }
    }
}

/// Identifies where a `log_level` / legacy `debug` setting was read from, so
/// `resolve_log_level` can emit source-appropriate warning messages.
#[derive(Debug, Clone, Copy)]
enum LogLevelSource {
    ConfigFile,
    Env,
}

/// Resolve a log level from an optional primary value (`log_level=` /
/// `ANVIL_LOG_LEVEL`) plus an optional legacy debug value (`debug=` /
/// `ANVIL_DEBUG`). The primary value wins; invalid primaries fall back to
/// `Info` with a warning, and consulting the legacy key emits a deprecation
/// warning.
fn resolve_log_level(
    primary: Option<&str>,
    legacy_debug: Option<&str>,
    source: LogLevelSource,
    warnings: &mut Vec<String>,
) -> Option<LogLevel> {
    if let Some(raw) = primary {
        return Some(match raw.parse::<LogLevel>() {
            Ok(level) => level,
            Err(err) => {
                warnings.push(match source {
                    LogLevelSource::ConfigFile => {
                        format!("invalid log_level={raw} in config file, using info: {err}")
                    }
                    LogLevelSource::Env => {
                        format!("invalid ANVIL_LOG_LEVEL={raw}, using info: {err}")
                    }
                });
                LogLevel::Info
            }
        });
    }

    let debug = legacy_debug.and_then(parse_bool)?;
    warnings.push(
        match source {
            LogLevelSource::ConfigFile => {
                "config file key 'debug' is deprecated, use 'log_level=info|verbose|trace'"
            }
            LogLevelSource::Env => {
                "ANVIL_DEBUG is deprecated, use ANVIL_LOG_LEVEL=info|verbose|trace"
            }
        }
        .to_string(),
    );
    Some(LogLevel::from_legacy_debug(debug))
}

/// Translate CLI log-level flags into an optional `LogLevel`.
/// `--trace` and the deprecated `--debug` alias win over `--verbose`; when
/// none are present the caller defers to env/config/defaults.
fn cli_log_level_from_flags(trace: bool, debug: bool, verbose: bool) -> Option<LogLevel> {
    if trace || debug {
        Some(LogLevel::Trace)
    } else if verbose {
        Some(LogLevel::Verbose)
    } else {
        None
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub cwd: PathBuf,
    pub requested_model: Option<String>,
    pub requested_sidecar_model: Option<String>,
    pub ollama_host: String,
    pub context_budget: usize,
    pub max_iterations: usize,
    pub chat_timeout_secs: u64,
    pub chat_retries: usize,
    pub log_level: LogLevel,
    pub stream: bool,
    pub yes_mode: bool,
    pub fresh_session: bool,
    pub oneshot: bool,
    pub auto_plan: bool,
    pub offline: bool,
    pub deterministic_fallback: DeterministicFallbackMode,
    pub prompt: Option<String>,
    pub state_dir_override: Option<PathBuf>,
    pub resume: ResumeRequest,
    /// Whether the fixed footer status bar should be enabled. `true` by default;
    /// disabled by `--no-footer`, `ANVIL_NO_FOOTER` (non-empty), or
    /// `.anvil/config` `footer=false`. See issue #430 §5.
    pub footer: bool,
    pub photon_enabled: bool,
    pub photon_url: String,
    /// Issue #667 (S3-004 / S7-004 / DR2-007): toggle for the PAM
    /// (photon-action-memory) advisory pipeline. Default: `true` via
    /// `Config::load`'s `unwrap_or(true)` (the struct's derived `Default`
    /// is `false`; production callers always go through `Config::load`).
    /// Setting this to `false` is the immediate rollback (§10.1) — the
    /// adapter chokepoint (`Agent::record_pam_advisory_decision`) early-
    /// returns and `MemoryReport.pam_decision` stays `None`.
    pub pam_advisory_enabled: bool,
    pub photon_shadow_mode: bool,
    pub photon_canary: u16,
    pub photon_timeout_ms: u64,
    /// Minimum number of shadow-mode evaluate turns required before canary rollout.
    /// Default: 100. Env: ANVIL_PHOTON_ROLLOUT_MIN_EVAL_TURNS.
    pub photon_rollout_min_eval_turns: u32,
    /// Issue #583: whether `context_pack` warnings should be respected to filter
    /// premature-termination seeds before prompt injection. Default: `true` via
    /// `Config::load` (the struct's derived `Default` leaves this `false`; the
    /// production path goes through `Config::load` which fills in `true`).
    ///
    /// Issue #589: when enabled, admission_reason-handled IDs are subtracted
    /// from the block set. No new env/config flag is added; the existing
    /// `photon_respect_warnings` gate governs the entire two-stage pipeline.
    pub photon_respect_warnings: bool,
    /// Issue #592: when `true`, `/photon-rule` persists a `PhotonSeedDraft` and
    /// ships it to the photon sidecar as a rule-promotion seed (i.e. potentially
    /// adopted into the common photon corpus). When `false` (default), the
    /// command runs as a dry-run that only records the rule locally without
    /// hitting `/v1/evaluate`. Env: `ANVIL_PHOTON_COMMON_SEED`. Config file key:
    /// `photon_common_seed_enabled`.
    pub photon_common_seed_enabled: bool,
    /// Issue #604 (AP-07): post-loop photon auto-promote hook 機能フラグ。
    /// Default: `true`。env: `ANVIL_PHOTON_AUTO_PROMOTE`、config key:
    /// `photon_auto_promote`。Phase 1 rollout 中は本フラグだけでなく
    /// `photon_auto_promote_dry_run=true` で HTTP skip するため、本フラグ
    /// `true` 単独では実際の photon 投入は走らない。
    pub photon_auto_promote: bool,
    /// Issue #604 (AP-07): 緊急 disable kill-switch。`true` の場合
    /// `photon_auto_promote` の値に関わらず hook を強制 disable する
    /// (`AutoPromoteSkipReason::Disabled { sub_reason: None }`)。Default:
    /// `false`。env: `ANVIL_PHOTON_NO_AUTO_PROMOTE` (POSIX `NO_COLOR` 慣例:
    /// 非空値で disable)、config key: `photon_no_auto_promote`。
    pub photon_no_auto_promote: bool,
    /// Issue #604 (AP-07 / DR-AP-5): Phase 1 rollout 中の HTTP skip フラグ。
    /// Default: **`true`** (Phase 1 dry-run period)。`true` の場合
    /// eligibility 判定を通過しても photon `/v1/summary/upsert` POST を
    /// skip し、`agent.photon_auto_promote.skipped {reason: dry_run}` event
    /// だけ emit する (per-turn cap は立てる / S7-004 #3)。env:
    /// `ANVIL_PHOTON_AUTO_PROMOTE_DRY_RUN`、config key:
    /// `photon_auto_promote_dry_run`。
    pub photon_auto_promote_dry_run: bool,
    /// Issue #604 (AP-07 / DR1-015): scrub mode の **文字列保持**。
    /// config 層は session 層 `ScrubMode` enum を import せず、生文字列で
    /// 保持する (既存 photon 系 primitive 流儀)。Default: `"strict"`
    /// (= `DEFAULT_SCRUB_MODE`)。`"warn"` を許容、それ以外は agent 層 hook
    /// 内で `ScrubMode::from_env_str_or_default` を通したときに warn log を
    /// 出して `Strict` にフォールバックする (DR1-016)。env:
    /// `ANVIL_PHOTON_AUTO_PROMOTE_SCRUB_MODE`、config key:
    /// `photon_auto_promote_scrub_mode`。
    pub photon_auto_promote_scrub_mode: String,
    /// Issue #634: 特化 fallback (FastAPI scaffold / Python CSV / FizzBuzz /
    /// 固定 arithmetic patch / qwen3.5 固有 deterministic edit) の experimental
    /// gate。Default: `false`。env: `ANVIL_EXPERIMENTAL_SPECIALIZED_FALLBACK`、
    /// config key: `experimental_specialized_fallback`、CLI:
    /// `--experimental-specialized-fallback`。
    ///
    /// `DeterministicFallbackMode::FullTemplate` の意味は変えない。
    /// template 系特化 fallback は本 flag と `FullTemplate` の AND 条件で発火
    /// (`specialized_template_fallback_enabled()` 参照)。
    /// edit 系 (arithmetic patch) は本 flag のみで gate
    /// (`specialized_fallback_enabled()` 参照)。
    pub experimental_specialized_fallback: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartialConfig {
    pub model: Option<String>,
    pub sidecar_model: Option<String>,
    pub ollama_host: Option<String>,
    pub context_budget: Option<usize>,
    pub max_iterations: Option<usize>,
    pub chat_timeout_secs: Option<u64>,
    pub chat_retries: Option<usize>,
    pub log_level: Option<LogLevel>,
    pub stream: Option<bool>,
    pub yes_mode: Option<bool>,
    pub fresh_session: Option<bool>,
    pub auto_plan: Option<bool>,
    pub offline: Option<bool>,
    pub deterministic_fallback: Option<DeterministicFallbackMode>,
    pub state_dir_override: Option<PathBuf>,
    /// `Some(false)` when an explicit disable signal is present
    /// (`--no-footer` / non-empty `ANVIL_NO_FOOTER` / `.anvil/config` `footer=false`).
    /// `None` means "no opinion" so default (`true`) wins.
    pub footer: Option<bool>,
    pub photon_enabled: Option<bool>,
    pub photon_url: Option<String>,
    /// Issue #667: optional override for the PAM advisory pipeline gate
    /// (None = production default `true` via `Config::load`).
    pub pam_advisory_enabled: Option<bool>,
    pub photon_shadow_mode: Option<bool>,
    pub photon_canary: Option<u16>,
    pub photon_timeout_ms: Option<u64>,
    pub photon_rollout_min_eval_turns: Option<u32>,
    /// Issue #583: optional override for the warning filter (None = use default).
    pub photon_respect_warnings: Option<bool>,
    /// Issue #592: optional override for the common-seed (photon-rule) gate
    /// (None = use default `false`).
    pub photon_common_seed_enabled: Option<bool>,
    /// Issue #604: optional override for the auto-promote hook feature flag
    /// (None = use default `true`).
    pub photon_auto_promote: Option<bool>,
    /// Issue #604: optional override for the emergency kill-switch
    /// (None = use default `false`).
    pub photon_no_auto_promote: Option<bool>,
    /// Issue #604: optional override for the dry-run gate
    /// (None = use default `true` — Phase 1 rollout).
    pub photon_auto_promote_dry_run: Option<bool>,
    /// Issue #604: optional override for the scrub mode string
    /// (None = use default `"strict"`). Unknown strings are accepted at this
    /// layer and validated only when the agent layer constructs `ScrubMode`
    /// (DR1-015 / DR1-016).
    pub photon_auto_promote_scrub_mode: Option<String>,
    /// Issue #634: optional override for the experimental specialized fallback
    /// gate (None = use default `false`).
    pub experimental_specialized_fallback: Option<bool>,
}

impl Config {
    /// Issue #634: experimental flag そのもの。edit 系特化 fallback
    /// (固定 arithmetic patch 等) の gate に使う。call site で必要に応じて
    /// `MinimalPatch` 以上か等の追加判定と AND する。本メソッドは agent 層
    /// (`src/agent/loop_run/*`) のみが参照する想定 (DR3-002)。
    pub fn specialized_fallback_enabled(&self) -> bool {
        self.experimental_specialized_fallback
    }

    /// Issue #634: template 系特化 fallback (FastAPI / Python CLI / FizzBuzz
    /// scaffold) が発火可能か。`experimental_specialized_fallback=true` かつ
    /// `DeterministicFallbackMode::FullTemplate` の AND 条件で true。
    /// `FullTemplate` の意味は変えず (後方互換)、本 flag を ON にしない限り
    /// production 完了 path には反映されない。
    pub fn specialized_template_fallback_enabled(&self) -> bool {
        self.experimental_specialized_fallback
            && self.deterministic_fallback.allows_template_completion()
    }

    pub fn load(args: CliArgs) -> Result<(Self, Vec<String>), String> {
        let cwd = match args.cwd.clone() {
            Some(path) => path,
            None => env::current_dir().map_err(|err| format!("failed to resolve cwd: {err}"))?,
        };

        let mut warnings: Vec<String> = Vec::new();

        let file_config = load_config_file(&cwd.join(".anvil").join("config"), &mut warnings)?;
        let env_config = load_env_config(&mut warnings);

        let cli_log_level = cli_log_level_from_flags(args.trace, args.debug, args.verbose);

        let cli_config = PartialConfig {
            model: args.model.clone(),
            sidecar_model: args.sidecar_model.clone(),
            ollama_host: args.ollama_host.clone(),
            context_budget: args.context_budget,
            max_iterations: args.max_iterations,
            chat_timeout_secs: args.chat_timeout_secs,
            chat_retries: args.chat_retries,
            log_level: cli_log_level,
            stream: args.stream.then_some(true),
            yes_mode: args.yes.then_some(true),
            fresh_session: args.fresh_session.then_some(true),
            auto_plan: args.auto_plan.then_some(true),
            offline: args.offline.then_some(true),
            deterministic_fallback: args.deterministic_fallback,
            state_dir_override: args.state_dir.clone(),
            // CLI footer flag is "disable-only": `--no-footer` emits Some(false),
            // omission emits None so file/env/default can still apply.
            footer: args.no_footer.then_some(false),
            // photon settings are not configurable via CLI flags in A1
            photon_enabled: None,
            photon_url: None,
            // Issue #667: PAM advisory CLI flag not introduced — env + file only.
            pam_advisory_enabled: None,
            photon_shadow_mode: None,
            photon_canary: None,
            photon_timeout_ms: None,
            photon_rollout_min_eval_turns: None,
            photon_respect_warnings: None,
            photon_common_seed_enabled: None,
            // Issue #604: auto-promote hook env-only (no CLI flags).
            photon_auto_promote: None,
            photon_no_auto_promote: None,
            photon_auto_promote_dry_run: None,
            photon_auto_promote_scrub_mode: None,
            // Issue #634: CLI flag for the experimental specialized fallback gate.
            experimental_specialized_fallback: args.experimental_specialized_fallback,
        };
        let merged = merge_partial_configs(&[file_config, env_config, cli_config]);
        let ollama_host = validate_localhost_url(
            merged
                .ollama_host
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string()),
            "ollama host",
        )?;
        let photon_url_raw = merged
            .photon_url
            .unwrap_or_else(|| "http://127.0.0.1:3030".to_string());
        let photon_url = validate_localhost_url(photon_url_raw, "photon_url")?;

        let offline = merged.offline.unwrap_or(false);
        if offline && merged.photon_enabled.unwrap_or(false) {
            warnings.push("photon_enabled is forced false because offline=true".to_string());
        }
        let photon_enabled = !offline && merged.photon_enabled.unwrap_or(false);

        let config = Self {
            cwd,
            requested_model: merged.model,
            requested_sidecar_model: merged.sidecar_model,
            ollama_host,
            context_budget: merged.context_budget.unwrap_or(24_000),
            max_iterations: merged.max_iterations.unwrap_or(50),
            chat_timeout_secs: merged.chat_timeout_secs.unwrap_or(300),
            chat_retries: merged.chat_retries.unwrap_or(2),
            log_level: merged.log_level.unwrap_or_default(),
            stream: merged.stream.unwrap_or(false),
            yes_mode: merged.yes_mode.unwrap_or(false),
            fresh_session: merged.fresh_session.unwrap_or(false),
            oneshot: args.oneshot || args.prompt.is_some(),
            auto_plan: merged.auto_plan.unwrap_or(false),
            offline,
            deterministic_fallback: merged.deterministic_fallback.unwrap_or_default(),
            prompt: args.prompt,
            state_dir_override: merged.state_dir_override,
            resume: ResumeRequest::from_flag(args.resume),
            // Default true; any disable signal (file/env/CLI) lands as Some(false).
            footer: merged.footer.unwrap_or(true),
            photon_enabled,
            photon_url,
            // Issue #667 (S3-004 / DR3-005): production default is `true`
            // — `Config::default()` (derive) yields `false`, which fixtures
            // must override explicitly (see T22 in the in-crate test mod).
            pam_advisory_enabled: merged.pam_advisory_enabled.unwrap_or(true),
            photon_shadow_mode: merged.photon_shadow_mode.unwrap_or(true),
            photon_canary: merged.photon_canary.unwrap_or(0),
            photon_timeout_ms: merged.photon_timeout_ms.unwrap_or(200),
            photon_rollout_min_eval_turns: merged.photon_rollout_min_eval_turns.unwrap_or(100),
            // Default true (Issue #583); explicit Some(false) from env/file
            // disables the warning filter (escape hatch for canary rollback).
            photon_respect_warnings: merged.photon_respect_warnings.unwrap_or(true),
            // Default false (Issue #592); explicit Some(true) from env/file
            // enables shipping `/photon-rule` drafts to the sidecar evaluate.
            photon_common_seed_enabled: merged.photon_common_seed_enabled.unwrap_or(false),
            // Issue #604 (AP-07): post-loop auto-promote hook. Default true:
            // the feature is on, but Phase 1 rollout still relies on
            // `photon_auto_promote_dry_run=true` for HTTP skip.
            photon_auto_promote: merged.photon_auto_promote.unwrap_or(true),
            // Issue #604 (AP-07): emergency kill-switch. Default false; any
            // non-empty `ANVIL_PHOTON_NO_AUTO_PROMOTE` sets Some(true) via
            // `load_env_config` so the disable wins.
            photon_no_auto_promote: merged.photon_no_auto_promote.unwrap_or(false),
            // Issue #604 (AP-07 / DR-AP-5): Phase 1 rollout default = true
            // (dry-run / HTTP skip). Switching to false enables real photon
            // upserts (deferred to a later rollout phase / separate issue).
            photon_auto_promote_dry_run: merged.photon_auto_promote_dry_run.unwrap_or(true),
            // Issue #604 (AP-07 / DR1-015 / DR1-018): default `"strict"`
            // matches `DEFAULT_SCRUB_MODE` (Strict). Unknown strings fall
            // back to Strict with a `tracing::warn!` in the agent layer when
            // `ScrubMode::from_env_str_or_default` is invoked.
            photon_auto_promote_scrub_mode: merged
                .photon_auto_promote_scrub_mode
                .unwrap_or_else(|| "strict".to_string()),
            // Issue #634: default false. Production builds keep specialized
            // fallback paths gated unless the operator explicitly opts in.
            experimental_specialized_fallback: merged
                .experimental_specialized_fallback
                .unwrap_or(false),
        };
        Ok((config, warnings))
    }
}

pub fn merge_partial_configs(configs: &[PartialConfig]) -> PartialConfig {
    let mut merged = PartialConfig::default();
    for config in configs {
        if config.model.is_some() {
            merged.model = config.model.clone();
        }
        if config.sidecar_model.is_some() {
            merged.sidecar_model = config.sidecar_model.clone();
        }
        if config.ollama_host.is_some() {
            merged.ollama_host = config.ollama_host.clone();
        }
        if config.context_budget.is_some() {
            merged.context_budget = config.context_budget;
        }
        if config.max_iterations.is_some() {
            merged.max_iterations = config.max_iterations;
        }
        if config.chat_timeout_secs.is_some() {
            merged.chat_timeout_secs = config.chat_timeout_secs;
        }
        if config.chat_retries.is_some() {
            merged.chat_retries = config.chat_retries;
        }
        if config.log_level.is_some() {
            merged.log_level = config.log_level;
        }
        if config.stream.is_some() {
            merged.stream = config.stream;
        }
        if config.yes_mode.is_some() {
            merged.yes_mode = config.yes_mode;
        }
        if config.fresh_session.is_some() {
            merged.fresh_session = config.fresh_session;
        }
        if config.auto_plan.is_some() {
            merged.auto_plan = config.auto_plan;
        }
        if config.offline.is_some() {
            merged.offline = config.offline;
        }
        if config.deterministic_fallback.is_some() {
            merged.deterministic_fallback = config.deterministic_fallback;
        }
        if config.state_dir_override.is_some() {
            merged.state_dir_override = config.state_dir_override.clone();
        }
        if config.footer.is_some() {
            merged.footer = config.footer;
        }
        if config.photon_enabled.is_some() {
            merged.photon_enabled = config.photon_enabled;
        }
        if config.photon_url.is_some() {
            merged.photon_url = config.photon_url.clone();
        }
        // Issue #667 (DR2-007): merged adjacent to photon_shadow_mode
        // (alphabetical `pam_*` < `photon_*` + functional adjacency).
        if config.pam_advisory_enabled.is_some() {
            merged.pam_advisory_enabled = config.pam_advisory_enabled;
        }
        if config.photon_shadow_mode.is_some() {
            merged.photon_shadow_mode = config.photon_shadow_mode;
        }
        if config.photon_canary.is_some() {
            merged.photon_canary = config.photon_canary;
        }
        if config.photon_timeout_ms.is_some() {
            merged.photon_timeout_ms = config.photon_timeout_ms;
        }
        if config.photon_rollout_min_eval_turns.is_some() {
            merged.photon_rollout_min_eval_turns = config.photon_rollout_min_eval_turns;
        }
        if config.photon_respect_warnings.is_some() {
            merged.photon_respect_warnings = config.photon_respect_warnings;
        }
        if config.photon_common_seed_enabled.is_some() {
            merged.photon_common_seed_enabled = config.photon_common_seed_enabled;
        }
        if config.photon_auto_promote.is_some() {
            merged.photon_auto_promote = config.photon_auto_promote;
        }
        if config.photon_no_auto_promote.is_some() {
            merged.photon_no_auto_promote = config.photon_no_auto_promote;
        }
        if config.photon_auto_promote_dry_run.is_some() {
            merged.photon_auto_promote_dry_run = config.photon_auto_promote_dry_run;
        }
        if config.photon_auto_promote_scrub_mode.is_some() {
            merged.photon_auto_promote_scrub_mode = config.photon_auto_promote_scrub_mode.clone();
        }
        if config.experimental_specialized_fallback.is_some() {
            merged.experimental_specialized_fallback = config.experimental_specialized_fallback;
        }
    }
    merged
}

pub fn load_config_file(path: &Path, warnings: &mut Vec<String>) -> Result<PartialConfig, String> {
    if !path.exists() {
        return Ok(PartialConfig::default());
    }

    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let map = parse_key_value_config(&contents);

    let log_level = resolve_log_level(
        map.get("log_level").map(String::as_str),
        map.get("debug").map(String::as_str),
        LogLevelSource::ConfigFile,
        warnings,
    );

    Ok(PartialConfig {
        model: map.get("model").cloned(),
        sidecar_model: map.get("sidecar_model").cloned(),
        ollama_host: map
            .get("ollama_host")
            .cloned()
            .or_else(|| map.get("provider_url").cloned()),
        context_budget: map
            .get("context_budget")
            .and_then(|value| value.parse().ok()),
        max_iterations: map
            .get("max_iterations")
            .and_then(|value| value.parse().ok()),
        chat_timeout_secs: map
            .get("chat_timeout_secs")
            .and_then(|value| value.parse().ok()),
        chat_retries: map.get("chat_retries").and_then(|value| value.parse().ok()),
        log_level,
        stream: map.get("stream").and_then(|value| parse_bool(value)),
        yes_mode: map.get("yes_mode").and_then(|value| parse_bool(value)),
        fresh_session: map.get("fresh_session").and_then(|value| parse_bool(value)),
        auto_plan: map.get("auto_plan").and_then(|value| parse_bool(value)),
        offline: map.get("offline").and_then(|value| parse_bool(value)),
        deterministic_fallback: map
            .get("deterministic_fallback")
            .and_then(|value| value.parse::<DeterministicFallbackMode>().ok()),
        state_dir_override: map.get("state_dir").map(PathBuf::from),
        // Only emit Some(false) for explicit disable; any other value (true /
        // unrecognized / missing) leaves footer as None so default wins.
        footer: map
            .get("footer")
            .and_then(|value| parse_bool(value))
            .and_then(|enabled| (!enabled).then_some(false)),
        photon_enabled: map.get("photon_enabled").and_then(|v| parse_bool(v)),
        // photon_url empty string is skipped by parse_key_value_config, so None means unset
        photon_url: map.get("photon_url").cloned(),
        // Issue #667: `.anvil/config` key `pam_advisory_enabled` (DR2-007
        // adjacent to `photon_shadow_mode`).
        pam_advisory_enabled: map.get("pam_advisory_enabled").and_then(|v| parse_bool(v)),
        photon_shadow_mode: map.get("photon_shadow_mode").and_then(|v| parse_bool(v)),
        photon_canary: map
            .get("photon_canary")
            .and_then(|v| parse_photon_canary(v, warnings)),
        photon_timeout_ms: map
            .get("photon_timeout_ms")
            .and_then(|v| parse_photon_timeout_ms(v, warnings)),
        photon_rollout_min_eval_turns: map
            .get("photon_rollout_min_eval_turns")
            .and_then(|v| parse_photon_rollout_min_eval_turns(v, warnings)),
        photon_respect_warnings: map
            .get("photon_respect_warnings")
            .and_then(|v| parse_bool(v)),
        photon_common_seed_enabled: map
            .get("photon_common_seed_enabled")
            .and_then(|v| parse_bool(v)),
        // Issue #604 auto-promote config keys.
        photon_auto_promote: map.get("photon_auto_promote").and_then(|v| parse_bool(v)),
        photon_no_auto_promote: map
            .get("photon_no_auto_promote")
            .and_then(|v| parse_bool(v)),
        photon_auto_promote_dry_run: map
            .get("photon_auto_promote_dry_run")
            .and_then(|v| parse_bool(v)),
        photon_auto_promote_scrub_mode: map.get("photon_auto_promote_scrub_mode").cloned(),
        // Issue #634: experimental specialized fallback opt-in.
        experimental_specialized_fallback: map
            .get("experimental_specialized_fallback")
            .and_then(|v| parse_bool(v)),
    })
}

pub fn load_env_config(warnings: &mut Vec<String>) -> PartialConfig {
    let anvil_log_level = env::var("ANVIL_LOG_LEVEL").ok();
    let anvil_debug = env::var("ANVIL_DEBUG").ok();
    let log_level = resolve_log_level(
        anvil_log_level.as_deref(),
        anvil_debug.as_deref(),
        LogLevelSource::Env,
        warnings,
    );

    PartialConfig {
        model: env::var("ANVIL_MODEL").ok(),
        sidecar_model: env::var("ANVIL_SIDECAR_MODEL").ok(),
        ollama_host: env::var("ANVIL_OLLAMA_HOST")
            .ok()
            .or_else(|| env::var("ANVIL_PROVIDER_URL").ok()),
        context_budget: env::var("ANVIL_CONTEXT_BUDGET")
            .ok()
            .and_then(|value| value.parse().ok()),
        max_iterations: env::var("ANVIL_MAX_ITERATIONS")
            .ok()
            .and_then(|value| value.parse().ok()),
        chat_timeout_secs: env::var("ANVIL_CHAT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok()),
        chat_retries: env::var("ANVIL_CHAT_RETRIES")
            .ok()
            .and_then(|value| value.parse().ok()),
        log_level,
        stream: env::var("ANVIL_STREAM")
            .ok()
            .and_then(|value| parse_bool(&value)),
        yes_mode: env::var("ANVIL_YES")
            .ok()
            .and_then(|value| parse_bool(&value)),
        fresh_session: env::var("ANVIL_FRESH_SESSION")
            .ok()
            .and_then(|value| parse_bool(&value)),
        auto_plan: env::var("ANVIL_AUTO_PLAN")
            .ok()
            .and_then(|value| parse_bool(&value)),
        offline: env::var("ANVIL_OFFLINE")
            .ok()
            .and_then(|value| parse_bool(&value)),
        deterministic_fallback: env::var("ANVIL_DETERMINISTIC_FALLBACK")
            .ok()
            .and_then(|value| value.parse::<DeterministicFallbackMode>().ok()),
        state_dir_override: env::var("ANVIL_STATE_DIR").ok().map(PathBuf::from),
        // POSIX `NO_COLOR` convention: any non-empty value disables; matches
        // `ANVIL_NO_SPINNER` / `ANVIL_NO_INTERRUPT` precedent (see spinner.rs).
        footer: env::var("ANVIL_NO_FOOTER")
            .ok()
            .and_then(|value| (!value.is_empty()).then_some(false)),
        photon_enabled: env::var("ANVIL_PHOTON_ENABLED")
            .ok()
            .and_then(|v| parse_bool(&v)),
        photon_url: env::var("ANVIL_PHOTON_URL").ok(),
        // Issue #667: env override `ANVIL_PAM_ADVISORY_ENABLED` (DR2-007).
        pam_advisory_enabled: env::var("ANVIL_PAM_ADVISORY_ENABLED")
            .ok()
            .and_then(|v| parse_bool(&v)),
        photon_shadow_mode: env::var("ANVIL_PHOTON_SHADOW_MODE")
            .ok()
            .and_then(|v| parse_bool(&v)),
        photon_canary: env::var("ANVIL_PHOTON_CANARY")
            .ok()
            .and_then(|v| parse_photon_canary(&v, warnings)),
        photon_timeout_ms: env::var("ANVIL_PHOTON_TIMEOUT_MS")
            .ok()
            .and_then(|v| parse_photon_timeout_ms(&v, warnings)),
        photon_rollout_min_eval_turns: env::var("ANVIL_PHOTON_ROLLOUT_MIN_EVAL_TURNS")
            .ok()
            .and_then(|v| parse_photon_rollout_min_eval_turns(&v, warnings)),
        photon_respect_warnings: env::var("ANVIL_PHOTON_RESPECT_WARNINGS")
            .ok()
            .and_then(|v| parse_bool(&v)),
        photon_common_seed_enabled: env::var("ANVIL_PHOTON_COMMON_SEED")
            .ok()
            .and_then(|v| parse_bool(&v)),
        // Issue #604 (AP-07) auto-promote env knobs.
        photon_auto_promote: env::var("ANVIL_PHOTON_AUTO_PROMOTE")
            .ok()
            .and_then(|v| parse_bool(&v)),
        // POSIX `NO_COLOR` convention: any non-empty value disables; matches
        // `ANVIL_NO_FOOTER` / `ANVIL_NO_SPINNER` / `ANVIL_NO_INTERRUPT`.
        photon_no_auto_promote: env::var("ANVIL_PHOTON_NO_AUTO_PROMOTE")
            .ok()
            .and_then(|v| (!v.is_empty()).then_some(true)),
        photon_auto_promote_dry_run: env::var("ANVIL_PHOTON_AUTO_PROMOTE_DRY_RUN")
            .ok()
            .and_then(|v| parse_bool(&v)),
        photon_auto_promote_scrub_mode: env::var("ANVIL_PHOTON_AUTO_PROMOTE_SCRUB_MODE")
            .ok()
            .filter(|v| !v.trim().is_empty()),
        // Issue #634: experimental specialized fallback opt-in.
        experimental_specialized_fallback: env::var("ANVIL_EXPERIMENTAL_SPECIALIZED_FALLBACK")
            .ok()
            .and_then(|value| parse_bool(&value)),
    }
}

fn parse_photon_canary(s: &str, warnings: &mut Vec<String>) -> Option<u16> {
    match s.trim().parse::<u16>() {
        Ok(n) if n <= 1000 => Some(n),
        _ => {
            warnings.push(format!(
                "invalid ANVIL_PHOTON_CANARY={s}, must be 0-1000, using default (0)"
            ));
            None
        }
    }
}

fn parse_photon_timeout_ms(s: &str, warnings: &mut Vec<String>) -> Option<u64> {
    match s.trim().parse::<u64>() {
        Ok(n) if (1..=60_000).contains(&n) => Some(n),
        _ => {
            warnings.push(format!(
                "invalid ANVIL_PHOTON_TIMEOUT_MS={s}, must be 1-60000, using default (200)"
            ));
            None
        }
    }
}

fn parse_photon_rollout_min_eval_turns(s: &str, warnings: &mut Vec<String>) -> Option<u32> {
    match s.trim().parse::<u32>() {
        Ok(n) if (1..=10_000).contains(&n) => Some(n),
        _ => {
            warnings.push(format!(
                "invalid photon_rollout_min_eval_turns={s}, must be 1-10000, using default ({DEFAULT_PHOTON_ROLLOUT_MIN_EVAL_TURNS})"
            ));
            None
        }
    }
}

pub fn parse_key_value_config(contents: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let without_inline_comment = match trimmed.split_once('#') {
            Some((prefix, _)) => prefix.trim(),
            None => trimmed,
        };
        if let Some((raw_key, raw_value)) = without_inline_comment.split_once('=') {
            let key = raw_key.trim().to_lowercase().replace('-', "_");
            let value = raw_value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Resolve `photon_rollout_min_eval_turns` without running full `Config::load`.
/// Priority: env > .anvil/config > default (100).
pub fn resolve_photon_rollout_min_eval_turns(
    workspace_root: &Path,
    warnings: &mut Vec<String>,
) -> u32 {
    let file_val = load_config_file(&workspace_root.join(".anvil").join("config"), warnings)
        .unwrap_or_default()
        .photon_rollout_min_eval_turns;
    let env_val = env::var("ANVIL_PHOTON_ROLLOUT_MIN_EVAL_TURNS")
        .ok()
        .and_then(|v| parse_photon_rollout_min_eval_turns(&v, warnings));
    env_val
        .or(file_val)
        .unwrap_or(DEFAULT_PHOTON_ROLLOUT_MIN_EVAL_TURNS)
}

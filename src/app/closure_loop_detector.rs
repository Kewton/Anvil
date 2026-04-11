//! Closure-mode reasoning loop detector (Issue #349).
//!
//! After `agent.fix_slice` fails (no successful worker mutation observed),
//! the parent agent can enter a natural-language "closure" loop: it keeps
//! producing zero-tool-call reasoning responses that restate the same cut
//! points (resolveAutoAnswer / truncated file / late-stage closure ...)
//! until the external runner timeout fires. The existing `LoopDetector`
//! only fires on repeated tool calls, so it never engages for this shape.
//!
//! This detector fingerprints reasoning-only LLM responses seen while the
//! guard is armed and escalates when a near-duplicate reappears.
//!
//! - Guard arming (the caller must verify these before calling):
//!     1. `worker_observed == false`
//!     2. `fixslice_worker_failure_count > 0`
//!     3. The LLM response produced no tool calls
//!     4. The raw reasoning content is non-trivial (≥ `MIN_SIGNIFICANT_TOKENS`)
//! - Escalation ladder: `Continue → Warn → StrongWarn → Break`
//!
//! A response is fingerprinted as the sorted set of "significant" tokens
//! (lowercase ASCII-alphanumeric runs of length ≥ `TOKEN_MIN_LEN`). Two
//! responses match when their Jaccard similarity ≥ `JACCARD_MATCH_THRESHOLD`.
//! LLM closure loops overwhelmingly recycle the same domain vocabulary
//! turn after turn — Jaccard survives the connector-word paraphrasing
//! that exact hashing would miss.

use std::collections::BTreeSet;
use std::collections::VecDeque;

use super::loop_detector::LoopAction;

/// Minimum number of significant tokens in a response before it is eligible
/// for fingerprinting. Short acknowledgements ("ok", "done") must not trip
/// the detector.
pub const MIN_SIGNIFICANT_TOKENS: usize = 20;

/// Minimum character length of a token to count as "significant". Filters
/// stopwords and CJK punctuation noise without needing a word list.
const TOKEN_MIN_LEN: usize = 5;

/// Jaccard similarity threshold (`|A ∩ B| / |A ∪ B|`) at which two
/// responses are considered near-duplicates. 0.7 is strict enough to
/// avoid flagging two unrelated discussions that happen to share common
/// domain words, but loose enough to absorb the connector-word
/// rewordings observed in cycle 8/9 closure loops.
const JACCARD_MATCH_THRESHOLD: f64 = 0.7;

/// Recent token sets retained for lookup. Four slots cover the Warn /
/// StrongWarn / Break ladder plus one headroom entry.
const HISTORY_CAPACITY: usize = 8;

/// A fingerprinted, reasoning-only LLM response.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    tokens: BTreeSet<String>,
}

impl Fingerprint {
    /// Extract significant tokens from `raw_content`.
    ///
    /// Returns `None` when the content does not contain enough significant
    /// tokens to be worth fingerprinting.
    pub fn from_raw(raw_content: &str) -> Option<Self> {
        let tokens: BTreeSet<String> = raw_content
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= TOKEN_MIN_LEN)
            .map(str::to_ascii_lowercase)
            .collect();
        if tokens.len() < MIN_SIGNIFICANT_TOKENS {
            return None;
        }
        Some(Self { tokens })
    }

    /// Jaccard similarity against `other` in `[0.0, 1.0]`.
    pub fn jaccard(&self, other: &Self) -> f64 {
        if self.tokens.is_empty() && other.tokens.is_empty() {
            return 1.0;
        }
        let intersect = self.tokens.intersection(&other.tokens).count() as f64;
        let union = self.tokens.union(&other.tokens).count() as f64;
        if union == 0.0 { 0.0 } else { intersect / union }
    }

    #[cfg(test)]
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }
}

/// Detector for closure-mode reasoning loops after `fix_slice` failure.
#[derive(Debug)]
pub struct ClosureLoopDetector {
    history: VecDeque<Fingerprint>,
    escalation_count: usize,
}

impl Default for ClosureLoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ClosureLoopDetector {
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            escalation_count: 0,
        }
    }

    /// Reset to a clean state.
    ///
    /// Called when the guard is disarmed (worker success observed, a new
    /// plan registered, or the response carried tool calls again).
    pub fn reset(&mut self) {
        self.history.clear();
        self.escalation_count = 0;
    }

    /// Observe a reasoning-only LLM response while the guard is armed and
    /// return the escalation action.
    ///
    /// Returns `LoopAction::Continue` if the response is too short to
    /// fingerprint or if no recent entry is near-duplicate.
    pub fn record_and_check(&mut self, raw_content: &str) -> LoopAction {
        let Some(fp) = Fingerprint::from_raw(raw_content) else {
            return LoopAction::Continue;
        };

        let near_duplicate = self
            .history
            .iter()
            .any(|prev| prev.jaccard(&fp) >= JACCARD_MATCH_THRESHOLD);

        if self.history.len() >= HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(fp);

        if !near_duplicate {
            return LoopAction::Continue;
        }

        self.escalation_count += 1;
        match self.escalation_count {
            1 => LoopAction::Warn(
                "[Closure Loop Guard] The parent agent is repeating the same closure \
                 reasoning after a fix_slice worker failure without advancing any tool. \
                 Decide now: either call agent.fix_slice once more on the troubled target \
                 or emit ANVIL_FINAL and stop."
                    .to_string(),
            ),
            2 => LoopAction::StrongWarn(
                "[Closure Loop Guard - WARNING] The parent agent has repeated the same \
                 closure reasoning multiple times without any tool-advancing output. You \
                 MUST either issue a concrete tool call now (agent.fix_slice on a specific \
                 target_path) or terminate with ANVIL_FINAL. Do NOT restate the same \
                 analysis again."
                    .to_string(),
            ),
            _ => LoopAction::Break(
                "[Closure Loop Guard - TERMINATED] Agentic loop terminated: closure-mode \
                 reasoning repeated after fix_slice worker failure with no tool-advancing \
                 progress."
                    .to_string(),
            ),
        }
    }

    #[cfg(test)]
    pub fn escalation_count(&self) -> usize {
        self.escalation_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOSURE_A: &str = "The resolveAutoAnswer helper appears truncated. \
        Because the worker produced no valid proposal, the fix_slice invocation \
        failed. The late-stage closure branch should therefore abort instead \
        of looping. resolveAutoAnswer, truncated, closure, invalid, proposal, \
        worker, failure, fix_slice, target_path, rewrite, iterations, session.";

    /// Same domain vocabulary, different connector words — the exact cycle
    /// 8/9 closure-loop shape we need to catch.
    const CLOSURE_A_REWORDED: &str = "Worker did not produce any valid proposal, \
        so the fix_slice invocation has failed. The resolveAutoAnswer helper \
        still appears truncated, therefore the late-stage closure branch \
        should abort instead of looping further, because continuing reasoning \
        is unhelpful. Keywords: closure, invalid, proposal, worker, failure, \
        fix_slice, target_path, rewrite, iterations, session, \
        resolveAutoAnswer, truncated.";

    const CLOSURE_B: &str = "Different angle now: the ANVIL_PLAN required \
        additional verification reads against configuration modules. Running \
        cargo check would reveal compilation blockers. Consider auditing \
        provider ollama sidecar summarize helpers, exploring retrieval \
        cache, evaluating telemetry snapshot format, inspecting hooks \
        engine wiring, scanning tooling shell policy.";

    #[test]
    fn short_content_is_not_fingerprinted() {
        assert!(Fingerprint::from_raw("ok").is_none());
        assert!(Fingerprint::from_raw("Plan done. Looks fine.").is_none());
    }

    #[test]
    fn long_content_yields_stable_fingerprint() {
        let fp1 = Fingerprint::from_raw(CLOSURE_A).expect("fingerprint");
        let fp2 = Fingerprint::from_raw(CLOSURE_A).expect("fingerprint");
        assert!((fp1.jaccard(&fp2) - 1.0).abs() < f64::EPSILON);
        assert!(fp1.token_count() >= MIN_SIGNIFICANT_TOKENS);
    }

    #[test]
    fn reworded_paraphrase_is_near_duplicate() {
        let fp1 = Fingerprint::from_raw(CLOSURE_A).expect("fingerprint");
        let fp2 = Fingerprint::from_raw(CLOSURE_A_REWORDED).expect("fingerprint");
        let sim = fp1.jaccard(&fp2);
        assert!(
            sim >= JACCARD_MATCH_THRESHOLD,
            "expected Jaccard ≥ {}, got {}",
            JACCARD_MATCH_THRESHOLD,
            sim
        );
    }

    #[test]
    fn unrelated_reasoning_is_below_threshold() {
        let fp1 = Fingerprint::from_raw(CLOSURE_A).expect("fingerprint");
        let fp2 = Fingerprint::from_raw(CLOSURE_B).expect("fingerprint");
        assert!(
            fp1.jaccard(&fp2) < JACCARD_MATCH_THRESHOLD,
            "unrelated content should be below threshold; got {}",
            fp1.jaccard(&fp2)
        );
    }

    #[test]
    fn first_observation_is_continue() {
        let mut det = ClosureLoopDetector::new();
        assert_eq!(det.record_and_check(CLOSURE_A), LoopAction::Continue);
    }

    #[test]
    fn second_near_duplicate_warns() {
        let mut det = ClosureLoopDetector::new();
        det.record_and_check(CLOSURE_A);
        let action = det.record_and_check(CLOSURE_A_REWORDED);
        assert!(
            matches!(action, LoopAction::Warn(_)),
            "expected Warn, got {:?}",
            action
        );
    }

    #[test]
    fn escalation_ladder_reaches_break() {
        let mut det = ClosureLoopDetector::new();
        assert_eq!(det.record_and_check(CLOSURE_A), LoopAction::Continue);
        assert!(matches!(
            det.record_and_check(CLOSURE_A),
            LoopAction::Warn(_)
        ));
        assert!(matches!(
            det.record_and_check(CLOSURE_A),
            LoopAction::StrongWarn(_)
        ));
        assert!(matches!(
            det.record_and_check(CLOSURE_A),
            LoopAction::Break(_)
        ));
    }

    #[test]
    fn novel_reasoning_between_repeats_still_escalates() {
        let mut det = ClosureLoopDetector::new();
        det.record_and_check(CLOSURE_A);
        // Novel cut point interleaves — should NOT clear escalation.
        assert_eq!(det.record_and_check(CLOSURE_B), LoopAction::Continue);
        let action = det.record_and_check(CLOSURE_A_REWORDED);
        assert!(
            matches!(action, LoopAction::Warn(_)),
            "expected Warn when A repeats despite B in between, got {:?}",
            action
        );
    }

    #[test]
    fn short_content_does_not_advance_state() {
        let mut det = ClosureLoopDetector::new();
        det.record_and_check(CLOSURE_A);
        assert_eq!(det.record_and_check("ok"), LoopAction::Continue);
        assert_eq!(det.escalation_count(), 0);
    }

    #[test]
    fn reset_clears_history_and_escalation() {
        let mut det = ClosureLoopDetector::new();
        det.record_and_check(CLOSURE_A);
        det.record_and_check(CLOSURE_A);
        assert_eq!(det.escalation_count(), 1);
        det.reset();
        assert_eq!(det.escalation_count(), 0);
        assert_eq!(det.record_and_check(CLOSURE_A), LoopAction::Continue);
    }
}

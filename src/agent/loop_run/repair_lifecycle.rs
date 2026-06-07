//! Pure repair lifecycle policy helpers.
//!
//! This module intentionally consumes already-normalized controller events. It
//! does not parse verifier text, inspect prompts, or interpret LLM prose. The
//! repair job owns mutable state; this module owns small read-only transition
//! predicates so retry/replan policy does not keep accumulating inside
//! `RepairJob::next_action`.

use super::repair_job::{RejectedAttempt, RepairAttemptKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RejectionEscalationKind {
    ActiveAttempt,
    CorrectionClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RejectionEscalation<'a> {
    pub(super) attempt: &'a RejectedAttempt,
    pub(super) kind: RejectionEscalationKind,
}

pub(super) fn first_rejection_escalation<'a>(
    rejected_attempts: &'a [RejectedAttempt],
    active_attempt_key: Option<&RepairAttemptKey>,
    active_correction_key: Option<&RepairAttemptKey>,
    repeated_attempt_threshold: usize,
    repeated_correction_threshold: usize,
) -> Option<RejectionEscalation<'a>> {
    repeated_active_attempt(
        rejected_attempts,
        active_attempt_key,
        repeated_attempt_threshold,
    )
    .map(|attempt| RejectionEscalation {
        attempt,
        kind: RejectionEscalationKind::ActiveAttempt,
    })
    .or_else(|| {
        repeated_correction_class(
            rejected_attempts,
            active_correction_key,
            repeated_correction_threshold,
        )
        .map(|attempt| RejectionEscalation {
            attempt,
            kind: RejectionEscalationKind::CorrectionClass,
        })
    })
}

fn repeated_active_attempt<'a>(
    rejected_attempts: &'a [RejectedAttempt],
    active_key: Option<&RepairAttemptKey>,
    threshold: usize,
) -> Option<&'a RejectedAttempt> {
    let active_key = active_key?;
    if active_attempt_count(rejected_attempts, active_key) < threshold {
        return None;
    }
    rejected_attempts
        .iter()
        .rev()
        .filter(|attempt| attempt_matches_active_key(&attempt.key, active_key))
        .next()
}

fn attempt_matches_active_key(
    attempt_key: &RepairAttemptKey,
    active_key: &RepairAttemptKey,
) -> bool {
    attempt_key.role == active_key.role
        && attempt_key.path == active_key.path
        && attempt_key.obligation_id == active_key.obligation_id
        && attempt_key.failure_domain == active_key.failure_domain
        && attempt_key.correction_kind == active_key.correction_kind
        && active_key
            .allowed_change_kind
            .is_none_or(|kind| attempt_key.allowed_change_kind == Some(kind))
}

fn repeated_correction_class<'a>(
    rejected_attempts: &'a [RejectedAttempt],
    active_key: Option<&RepairAttemptKey>,
    threshold: usize,
) -> Option<&'a RejectedAttempt> {
    let active_key = active_key?;
    let active_domain = active_key.failure_domain?;
    let active_kind = active_key.correction_kind?;
    rejected_attempts.iter().rev().find(|attempt| {
        attempt.key.failure_domain == Some(active_domain)
            && attempt.key.correction_kind == Some(active_kind)
            && correction_class_count(rejected_attempts, active_domain, active_kind) >= threshold
    })
}

fn active_attempt_count(
    rejected_attempts: &[RejectedAttempt],
    active_key: &RepairAttemptKey,
) -> usize {
    rejected_attempts
        .iter()
        .filter(|candidate| attempt_matches_active_key(&candidate.key, active_key))
        .count()
}

fn correction_class_count(
    rejected_attempts: &[RejectedAttempt],
    domain: super::repair_packet::DeliverableFailureDomain,
    kind: super::repair_packet::CorrectionKind,
) -> usize {
    rejected_attempts
        .iter()
        .filter(|candidate| {
            candidate.key.failure_domain == Some(domain)
                && candidate.key.correction_kind == Some(kind)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::super::repair_brief::AllowedChangeKind;
    use super::super::repair_job::RejectedAttemptReason;
    use super::super::repair_packet::{CorrectionKind, DeliverableFailureDomain};
    use super::super::task_contract::ArtifactRole;
    use super::*;

    fn key(path: &str) -> RepairAttemptKey {
        RepairAttemptKey {
            role: ArtifactRole::Implementation,
            path: path.to_string(),
            allowed_change_kind: Some(AllowedChangeKind::FixImplementationBehavior),
            obligation_id: None,
            failure_domain: None,
            correction_kind: Some(CorrectionKind::Patch),
        }
    }

    fn correction_key(domain: DeliverableFailureDomain, kind: CorrectionKind) -> RepairAttemptKey {
        RepairAttemptKey {
            role: ArtifactRole::Setup,
            path: "Cargo.toml".to_string(),
            allowed_change_kind: None,
            obligation_id: Some("setup:Cargo.toml".to_string()),
            failure_domain: Some(domain),
            correction_kind: Some(kind),
        }
    }

    fn rejected(key: RepairAttemptKey, reason: RejectedAttemptReason) -> RejectedAttempt {
        RejectedAttempt { key, reason }
    }

    #[test]
    fn escalation_detects_repeated_active_attempt_only_for_active_key() {
        let old_key = key("tests/old.rs");
        let active_key = key("src/lib.rs");
        let attempts = vec![
            rejected(old_key.clone(), RejectedAttemptReason::MalformedPatch),
            rejected(old_key, RejectedAttemptReason::MalformedPatch),
            rejected(active_key.clone(), RejectedAttemptReason::NoopPatch),
            rejected(active_key.clone(), RejectedAttemptReason::NoopPatch),
        ];

        let escalation =
            first_rejection_escalation(&attempts, Some(&active_key), Some(&active_key), 2, 2)
                .expect("active repeated rejection");

        assert_eq!(escalation.kind, RejectionEscalationKind::ActiveAttempt);
        assert_eq!(escalation.attempt.key.path, "src/lib.rs");
    }

    #[test]
    fn escalation_detects_repeated_active_target_even_when_reasons_differ() {
        let active_key = key("src/lib.rs");
        let attempts = vec![
            rejected(active_key.clone(), RejectedAttemptReason::MalformedPatch),
            rejected(active_key.clone(), RejectedAttemptReason::WrongTarget),
        ];

        let escalation =
            first_rejection_escalation(&attempts, Some(&active_key), Some(&active_key), 2, 2)
                .expect("active target repeated rejection");

        assert_eq!(escalation.kind, RejectionEscalationKind::ActiveAttempt);
        assert_eq!(
            escalation.attempt.reason,
            RejectedAttemptReason::WrongTarget
        );
    }

    #[test]
    fn escalation_ignores_rejections_for_inactive_target() {
        let old_key = key("tests/old.rs");
        let active_key = key("src/lib.rs");
        let attempts = vec![
            rejected(old_key.clone(), RejectedAttemptReason::MalformedPatch),
            rejected(old_key, RejectedAttemptReason::MalformedPatch),
        ];

        assert!(
            first_rejection_escalation(&attempts, Some(&active_key), Some(&active_key), 2, 2)
                .is_none()
        );
    }

    #[test]
    fn escalation_detects_repeated_correction_class_across_paths() {
        let active_key = correction_key(
            DeliverableFailureDomain::InvalidManifest,
            CorrectionKind::ManifestCorrection,
        );
        let alternate_key = RepairAttemptKey {
            path: "package.json".to_string(),
            ..active_key.clone()
        };
        let attempts = vec![
            rejected(alternate_key, RejectedAttemptReason::MalformedPatch),
            rejected(active_key.clone(), RejectedAttemptReason::WrongTarget),
        ];

        let escalation =
            first_rejection_escalation(&attempts, Some(&active_key), Some(&active_key), 2, 2)
                .expect("correction class repeated");

        assert_eq!(escalation.kind, RejectionEscalationKind::CorrectionClass);
    }
}

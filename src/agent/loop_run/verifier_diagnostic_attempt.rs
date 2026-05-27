pub(super) const VERIFIER_DIAGNOSTIC_SIDECAR_TIMEOUT_SECS: u64 = 45;
pub(super) const VERIFIER_DIAGNOSTIC_MAIN_FALLBACK_TIMEOUT_SECS: u64 = 90;
pub(super) const VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierDiagnosticAttemptSpec {
    pub(super) model: String,
    pub(super) timeout_secs: u64,
    pub(super) role: &'static str,
}

pub(super) fn verifier_diagnostic_attempt_spec(
    main_model: &str,
    sidecar_model: Option<&str>,
    attempts_done: usize,
) -> Option<VerifierDiagnosticAttemptSpec> {
    if attempts_done >= VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT {
        return None;
    }
    let sidecar = sidecar_model.filter(|model| *model != main_model);
    match (attempts_done, sidecar) {
        (0, Some(model)) => Some(VerifierDiagnosticAttemptSpec {
            model: model.to_string(),
            timeout_secs: VERIFIER_DIAGNOSTIC_SIDECAR_TIMEOUT_SECS,
            role: "sidecar",
        }),
        (0, None) => Some(VerifierDiagnosticAttemptSpec {
            model: main_model.to_string(),
            timeout_secs: VERIFIER_DIAGNOSTIC_MAIN_FALLBACK_TIMEOUT_SECS,
            role: "main",
        }),
        (1, Some(_)) => Some(VerifierDiagnosticAttemptSpec {
            model: main_model.to_string(),
            timeout_secs: VERIFIER_DIAGNOSTIC_MAIN_FALLBACK_TIMEOUT_SECS,
            role: "main_fallback",
        }),
        (1, None) | (2, _) => Some(VerifierDiagnosticAttemptSpec {
            model: main_model.to_string(),
            timeout_secs: VERIFIER_DIAGNOSTIC_MAIN_FALLBACK_TIMEOUT_SECS,
            role: "main_retry",
        }),
        _ => None,
    }
}

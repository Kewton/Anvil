use super::task_contract::{
    SETUP_MARKER_NEEDLES_ASCII, lower_contains_setup_token_unnegated, request_asks_for_setup,
};

/// Positive baseline: "install dependencies" must match (no negation).
#[test]
fn request_asks_for_setup_positive_install_dependencies_matches() {
    let req = "Please install dependencies before running tests.";
    let lower = req.to_ascii_lowercase();
    assert!(request_asks_for_setup(req, &lower));
}

/// Positive baseline: "setup the project" must match (no negation,
/// token-bounded).
#[test]
fn request_asks_for_setup_positive_setup_dependencies_matches() {
    let req = "Setup the project dependencies for fresh checkout.";
    let lower = req.to_ascii_lowercase();
    assert!(request_asks_for_setup(req, &lower));
}

/// CB-002 token boundary + negation: "uninstall dependencies" must
/// NOT classify as asking for Setup.
#[test]
fn request_asks_for_setup_token_boundary_uninstall_not_match() {
    let req = "uninstall dependencies and reinstall a clean build";
    let lower = req.to_ascii_lowercase();
    assert!(
        !lower_contains_setup_token_unnegated(&lower, "install"),
        "leading 'un' must break the 'install' token boundary"
    );
    assert!(
        !lower_contains_setup_token_unnegated(&lower, "dependencies"),
        "preceding 'uninstall' must suppress the 'dependencies' marker via un-prefix detection"
    );
    assert!(
        !request_asks_for_setup(req, &lower),
        "full 'uninstall dependencies ...' request must NOT classify as asking for Setup"
    );
}

/// CB-002 negation prefix: "do not install dependencies" must NOT
/// classify as asking for Setup.
#[test]
fn request_asks_for_setup_negation_do_not_install_not_match() {
    let req = "Please do not install dependencies for this branch.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "install"));
}

/// CB-002 negation prefix: "don't install dependencies" must NOT
/// classify as asking for Setup.
#[test]
fn request_asks_for_setup_negation_dont_install_not_match() {
    let req = "Don't install dependencies; the runner already has them.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "install"));
}

/// CB-002 negation prefix: "without dependencies" must NOT match the
/// `dependencies` marker.
#[test]
fn request_asks_for_setup_negation_without_dependencies_not_match() {
    let req = "Build the binary without dependencies on system libs.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(
        &lower,
        "dependencies"
    ));
}

/// CB-002 negation prefix: "disable setup" must NOT classify as
/// asking for Setup.
#[test]
fn request_asks_for_setup_negation_disable_setup_not_match() {
    let req = "Disable setup hooks during release packaging.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "setup"));
}

/// CB-002 negation prefix: "no setup" / "no dependencies" rejected.
#[test]
fn request_asks_for_setup_negation_no_setup_not_match() {
    let req = "No setup steps are required for this command.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "setup"));
}

/// CB2-002 phrase-span: "do not install dependencies" must NOT
/// classify as asking for Setup.
#[test]
fn request_asks_for_setup_phrase_negation_do_not_install_dependencies() {
    let req = "Please do not install dependencies for this branch.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "install"));
    assert!(
        !lower_contains_setup_token_unnegated(&lower, "dependencies"),
        "phrase-span scan must suppress 'dependencies' carried in a 'do not install' phrase (CB2-002)"
    );
    assert!(
        !request_asks_for_setup(req, &lower),
        "full 'do not install dependencies' phrase must NOT classify as asking for Setup"
    );
}

/// CB2-002 phrase-span: "don't install dependencies".
#[test]
fn request_asks_for_setup_phrase_negation_dont_install_dependencies() {
    let req = "Don't install dependencies; the runner already has them.";
    let lower = req.to_ascii_lowercase();
    assert!(!lower_contains_setup_token_unnegated(&lower, "install"));
    assert!(
        !lower_contains_setup_token_unnegated(&lower, "dependencies"),
        "phrase-span scan must suppress 'dependencies' carried in a \"don't install\" phrase (CB2-002)"
    );
    assert!(
        !request_asks_for_setup(req, &lower),
        "full \"don't install dependencies\" phrase must NOT classify as asking for Setup"
    );
}

/// CB2-002 suffix-compound: "dependency-free X" classifies as a
/// negation via the `-free` suffix morpheme.
#[test]
fn request_asks_for_setup_dependency_free_compound_suffix() {
    let req = "Build a dependency-free binary.";
    let lower = req.to_ascii_lowercase();
    assert!(
        !lower_contains_setup_token_unnegated(&lower, "dependency"),
        "suffix-compound `-free` must suppress the `dependency` marker (CB2-002)"
    );
    assert!(
        !request_asks_for_setup(req, &lower),
        "dependency-free phrase must NOT classify as asking for Setup"
    );
}

/// CB2-002 baseline: positive `install dependencies` (no negation)
/// must still match.
#[test]
fn request_asks_for_setup_positive_install_dependencies_baseline() {
    let req = "Please install dependencies for the feature work.";
    let lower = req.to_ascii_lowercase();
    assert!(lower_contains_setup_token_unnegated(&lower, "install"));
    assert!(lower_contains_setup_token_unnegated(&lower, "dependencies"));
    assert!(request_asks_for_setup(req, &lower));
}

#[test]
fn request_asks_for_setup_false_for_readme_setup_section() {
    let req = "Write README.md with setup, usage, and troubleshooting sections for a backup CLI.";
    let lower = req.to_ascii_lowercase();

    assert!(!request_asks_for_setup(req, &lower));
}

/// Compositional regression: the public `request_asks_for_setup` API
/// must return `false` for the canonical negated phrasings even when
/// other unrelated text is present.
#[test]
fn request_asks_for_setup_composite_negated_phrasings_return_false() {
    let negated_phrasings = [
        "do not install anything",
        "don't install the package",
        "without setup hooks",
        "disable setup",
        "no setup needed",
        "skip setup",
        "avoid install of optional crates",
    ];
    for phrasing in negated_phrasings {
        let lower = phrasing.to_ascii_lowercase();
        for needle in SETUP_MARKER_NEEDLES_ASCII {
            assert!(
                !lower_contains_setup_token_unnegated(&lower, needle),
                "negated phrasing {phrasing:?} unexpectedly matched needle {needle:?}"
            );
        }
    }
}

/// Regression: positive phrasings must still pass through the
/// token-boundary path so iteration-1 acceptance is preserved.
#[test]
fn request_asks_for_setup_composite_positive_phrasings_return_true() {
    let positive_phrasings = [
        "install dependencies",
        "setup the requirements",
        "please install the package",
        "install requirements.txt",
    ];
    for phrasing in positive_phrasings {
        let lower = phrasing.to_ascii_lowercase();
        assert!(
            request_asks_for_setup(phrasing, &lower),
            "positive phrasing {phrasing:?} regressed to false"
        );
    }
}

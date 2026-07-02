import sys
import unittest
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_scoring import score_plan_file
from eval_lib.suites import load_suite


class PlanScoringTest(unittest.TestCase):
    def setUp(self):
        suite = load_suite(ROOT / "eval/suites/mvp-smoke.yaml")
        self.scenario = next(s for s in suite["scenarios"] if s["id"] == "nextjs-space-invaders-large")

    def test_good_plan_scores_higher_than_bad(self):
        good = score_plan_file(ROOT / "eval/fixtures/plans/good-step-plan.yaml", self.scenario)
        bad = score_plan_file(ROOT / "eval/fixtures/plans/bad-overlong-step-plan.yaml", self.scenario)
        self.assertGreater(good["score"], bad["score"])

    def test_path_escape_is_penalized(self):
        score = score_plan_file(ROOT / "eval/fixtures/plans/bad-path-escape-step-plan.yaml", self.scenario)
        penalties = score["details"]["penalties"]
        self.assertTrue(any(p["kind"] == "path_escape" for p in penalties))
        self.assertLess(score["score"], 60)

    def test_ultra_plan_scores(self):
        score = score_plan_file(ROOT / "eval/fixtures/plans/good-ultra-plan.yaml", self.scenario)
        self.assertEqual(score["kind"], "ultra")
        self.assertGreaterEqual(score["score"], 60)

    def test_plan_quality_penalizes_lint_categories(self):
        text = """goal: bad
steps:
  - id: s1
    kind: implement
    instruction: Create app
    expected_paths:
      - src/app/page.tsx
    verify:
      - npm test && npm run build
  - id: s2
    kind: implement
    instruction: Create duplicate
    expected_paths:
      - src/app/page.tsx
"""
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, self.scenario)
        penalties = {penalty["kind"] for penalty in score["details"]["penalties"]}
        self.assertIn("verify_command_policy_error", penalties)
        self.assertIn("duplicate_expected_path_ownership", penalties)

    def test_executable_score_penalizes_read_before_create(self):
        text = """goal: docs
steps:
  - id: inspect
    kind: inspect
    instruction: Check whether docs/CLI_USAGE.md already exists before writing it.
  - id: implement
    kind: implement
    instruction: Create CLI usage docs.
    expected_paths:
      - docs/CLI_USAGE.md
"""
        scenario = {
            "size": "small",
            "expected_artifacts": ["docs/CLI_USAGE.md"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": []},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        penalties = {penalty["kind"] for penalty in score["executable_details"]["penalties"]}
        self.assertIn("read_before_create_risk", penalties)
        self.assertLess(score["executable_score"], 70)

    def test_executable_score_penalizes_verify_step_without_command(self):
        text = """goal: rust
steps:
  - id: implement
    kind: implement
    instruction: Create src/lib.rs for the range set library.
    expected_paths:
      - src/lib.rs
  - id: verify
    kind: verify
    instruction: Verify the Rust tests pass.
"""
        scenario = {
            "size": "medium",
            "expected_artifacts": ["src/lib.rs"],
            "plan_constraints": {"min_steps": 2, "max_steps": 5, "required_verify_keywords": ["cargo test"]},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        penalties = {penalty["kind"] for penalty in score["executable_details"]["penalties"]}
        self.assertIn("verify_step_without_command", penalties)
        self.assertLess(score["executable_score"], 70)

    def test_executable_score_penalizes_existing_workspace_assumption(self):
        text = """goal: docs
steps:
  - id: inspect
    kind: inspect
    instruction: Inspect the workspace to understand the existing command line tool and its options.
  - id: implement
    kind: implement
    instruction: Create docs/CLI_USAGE.md with an options table and one example.
    expected_paths:
      - docs/CLI_USAGE.md
  - id: verify
    kind: verify
    instruction: Verify docs/CLI_USAGE.md exists.
    verify:
      - test -s docs/CLI_USAGE.md
"""
        scenario = {
            "size": "small",
            "expected_artifacts": ["docs/CLI_USAGE.md"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": ["docs/CLI_USAGE.md"]},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        penalties = {penalty["kind"] for penalty in score["executable_details"]["penalties"]}
        self.assertIn("workspace_assumption_before_creation", penalties)
        self.assertLess(score["executable_score"], 90)

    def test_executable_score_allows_seeded_workspace_inspection(self):
        text = """goal: docs
steps:
  - id: inspect
    kind: inspect
    instruction: Inspect the workspace to understand the existing command line tool and its options.
  - id: implement
    kind: implement
    instruction: Create docs/CLI_USAGE.md with an options table and one example.
    expected_paths:
      - docs/CLI_USAGE.md
"""
        scenario = {
            "size": "small",
            "expected_artifacts": ["docs/CLI_USAGE.md"],
            "seed_files": ["src/main.rs"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": []},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        penalties = {penalty["kind"] for penalty in score["executable_details"]["penalties"]}
        self.assertNotIn("workspace_assumption_before_creation", penalties)

    def test_executable_score_rewards_direct_creation_and_verify(self):
        text = """goal: parser
steps:
  - id: implement
    kind: implement
    instruction: Create duration_parser.js with a node self-test.
    expected_paths:
      - duration_parser.js
    verify:
      - node duration_parser.js
"""
        scenario = {
            "size": "small",
            "expected_artifacts": ["duration_parser.js"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": ["node"]},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        self.assertGreaterEqual(score["executable_score"], 80, score)

    def test_constraint_verify_and_artifact_scores_reward_complete_nextjs_plan(self):
        text = """goal: nextjs app on port 3011
steps:
  - id: setup
    kind: setup
    instruction: Create package.json with next, react, react-dom, scripts.build = next build, and dev script next dev -p 3011.
    expected_paths:
      - package.json
  - id: implement
    kind: implement
    instruction: Create src/app/page.tsx, src/app/layout.tsx, and src/app/global.d.ts for the App Router game.
    expected_paths:
      - src/app/page.tsx
      - src/app/layout.tsx
      - src/app/global.d.ts
  - id: verify
    kind: verify
    instruction: Verify the Next.js production build.
    verify:
      - npm run build
"""
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, self.scenario)
        self.assertGreaterEqual(score["constraint_coverage_score"], 90, score)
        self.assertGreaterEqual(score["verify_strength_score"], 75, score)
        self.assertGreaterEqual(score["artifact_ownership_score"], 90, score)

    def test_new_scores_penalize_weak_verify_and_extra_artifact(self):
        text = """goal: js helper
steps:
  - id: inspect
    kind: inspect
    instruction: Inspect the workspace before making changes.
    verify:
      - cat date-helper.js
  - id: implement
    kind: implement
    instruction: Create the helper.
    expected_paths:
      - date-helper.js
      - smoke-check.js
  - id: verify
    kind: verify
    instruction: Check files exist.
    verify:
      - test -f date-helper.js
"""
        scenario = {
            "size": "small",
            "profile": "generic",
            "prompt": "Fix a JavaScript date helper and add a deterministic node smoke check.",
            "expected_artifacts": ["date-helper.js"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": ["node"]},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        self.assertLess(score["constraint_coverage_score"], 80, score)
        self.assertLess(score["verify_strength_score"], 40, score)
        self.assertLess(score["artifact_ownership_score"], 90, score)
        penalties = {penalty["kind"] for penalty in score["artifact_ownership_details"]["penalties"]}
        self.assertIn("extra_artifact_ownership", penalties)

    def test_execution_shape_readiness_penalizes_wrapper_and_terminal_report(self):
        risky = """goal: js helper
steps:
  - id: inspect
    kind: inspect
    instruction: Inspect the current workspace and determine whether date-helper.js already exists.
  - id: implement
    kind: implement
    instruction: Create date-helper.js.
    expected_paths:
      - date-helper.js
  - id: report
    kind: report
    instruction: Report completion.
"""
        direct = """goal: js helper
steps:
  - id: implement
    kind: implement
    instruction: Create date-helper.js and smoke-check.js, then verify with node smoke-check.js.
    expected_paths:
      - date-helper.js
      - smoke-check.js
    verify:
      - node smoke-check.js
"""
        scenario = {
            "size": "small",
            "profile": "generic",
            "prompt": "Fix a JavaScript date helper and add a deterministic node smoke check.",
            "expected_artifacts": ["date-helper.js"],
            "plan_constraints": {"min_steps": 1, "max_steps": 4, "required_verify_keywords": ["node"]},
        }
        with tempfile.TemporaryDirectory() as td:
            risky_path = Path(td) / "risky.yaml"
            direct_path = Path(td) / "direct.yaml"
            risky_path.write_text(risky, encoding="utf-8")
            direct_path.write_text(direct, encoding="utf-8")
            risky_score = score_plan_file(risky_path, scenario)
            direct_score = score_plan_file(direct_path, scenario)
        self.assertLess(
            risky_score["execution_shape_readiness_score"],
            direct_score["execution_shape_readiness_score"],
            (risky_score, direct_score),
        )
        penalties = {penalty["kind"] for penalty in risky_score["execution_shape_details"]["penalties"]}
        self.assertIn("wrapper_steps_without_artifacts", penalties)
        self.assertIn("terminal_report_step", penalties)

    def test_verify_strength_penalizes_semantically_invalid_rust_verify(self):
        text = """goal: rust cli
steps:
  - id: setup
    kind: setup
    instruction: Create Cargo.toml for the CLI crate.
    expected_paths:
      - Cargo.toml
    verify:
      - cargo verify-project --manifest-path Cargo.toml
  - id: implement
    kind: implement
    instruction: Create src/main.rs with CLI behavior and a unit test.
    expected_paths:
      - src/main.rs
    verify:
      - rustc src/main.rs --no-link
      - cargo test
"""
        scenario = {
            "size": "medium",
            "expected_artifacts": ["Cargo.toml", "src/main.rs"],
            "postcheck": {"commands": ["cargo test"]},
            "plan_constraints": {"min_steps": 3, "max_steps": 8, "required_verify_keywords": ["cargo test"]},
        }
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, scenario)
        verify_penalties = {penalty["kind"] for penalty in score["verify_strength_details"]["penalties"]}
        shape_penalties = {penalty["kind"] for penalty in score["execution_shape_details"]["penalties"]}
        self.assertIn("invalid_raw_compile_verify", verify_penalties)
        self.assertIn("invalid_raw_compile_verify", shape_penalties)
        self.assertLess(score["verify_strength_score"], 45, score)
        self.assertLess(score["execution_shape_readiness_score"], 80, score)

    def test_nextjs_environment_contract_rewards_explicit_dependency_coherence(self):
        vague = """goal: nextjs app on port 3011
steps:
  - id: setup-package-and-tsconfig
    kind: setup
    instruction: Create package.json with next, react, react-dom, and typescript/developer dependencies. Create a modern tsconfig.json optimized for Next.js build compatibility.
    expected_paths:
      - package.json
      - tsconfig.json
    verify:
      - test -f package.json
  - id: implement-app
    kind: implement
    instruction: Create src/app/page.tsx, src/app/layout.tsx, and src/app/global.d.ts for the game.
    expected_paths:
      - src/app/page.tsx
      - src/app/layout.tsx
      - src/app/global.d.ts
    verify:
      - test -f src/app/page.tsx
  - id: verify-build
    kind: verify
    instruction: Verify the Next.js production build.
    verify:
      - grep -q 3011 package.json
      - grep -q "next build" package.json
      - npm run build
"""
        explicit = """goal: nextjs app on port 3011
steps:
  - id: setup-package-and-tsconfig
    kind: setup
    instruction: Create package.json with compatible aligned Next.js, React, React DOM, @types/react, @types/react-dom, and TypeScript 5.x dependencies. Use matching React runtime and type package major versions. Add scripts.build = next build and scripts.dev = next dev -p 3011. Create tsconfig.json with moduleResolution=bundler and target=ES2017 or newer.
    expected_paths:
      - package.json
      - tsconfig.json
    verify:
      - test -f package.json
  - id: implement-app
    kind: implement
    instruction: Create src/app/page.tsx, src/app/layout.tsx, and src/app/global.d.ts for the App Router game.
    expected_paths:
      - src/app/page.tsx
      - src/app/layout.tsx
      - src/app/global.d.ts
    verify:
      - test -f src/app/page.tsx
  - id: verify-build
    kind: verify
    instruction: Verify the Next.js production build.
    verify:
      - npm run build
"""
        with tempfile.TemporaryDirectory() as td:
            vague_path = Path(td) / "vague.yaml"
            explicit_path = Path(td) / "explicit.yaml"
            vague_path.write_text(vague, encoding="utf-8")
            explicit_path.write_text(explicit, encoding="utf-8")
            vague_score = score_plan_file(vague_path, self.scenario)
            explicit_score = score_plan_file(explicit_path, self.scenario)
        vague_penalties = {penalty["kind"] for penalty in vague_score["execution_shape_details"]["penalties"]}
        explicit_penalties = {
            penalty["kind"] for penalty in explicit_score["execution_shape_details"]["penalties"]
        }
        self.assertIn("dependency_coherence_contract_not_explicit", vague_penalties)
        self.assertIn("type_dependency_contract_not_explicit", vague_penalties)
        self.assertNotIn("dependency_coherence_contract_not_explicit", explicit_penalties)
        self.assertGreater(
            explicit_score["execution_shape_readiness_score"],
            vague_score["execution_shape_readiness_score"],
            (vague_score, explicit_score),
        )

    def test_nextjs_environment_contract_detects_known_package_version_risks(self):
        text = """goal: nextjs app on port 3011
steps:
  - id: setup
    kind: setup
    instruction: Create package.json with react ^19.0.0, react-dom ^19.0.0, @types/react 19.2.17, @types/react-dom ^18.3.0, and TypeScript 6.0.3. Create tsconfig.json with target ES5 and ignoreDeprecations 6.0.
    expected_paths:
      - package.json
      - tsconfig.json
    verify:
      - npm run build
"""
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, self.scenario)
        penalties = {penalty["kind"] for penalty in score["execution_shape_details"]["penalties"]}
        self.assertIn("type_dependency_major_mismatch", penalties)
        self.assertIn("config_deprecated_target_risk", penalties)
        self.assertLess(score["execution_shape_readiness_score"], 60, score)


if __name__ == "__main__":
    unittest.main()

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))


def load_eval_run():
    module_path = ROOT / "scripts/eval-run.py"
    spec = importlib.util.spec_from_file_location("eval_run_contract_snapshots", module_path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CompletionContractSnapshotTest(unittest.TestCase):
    def test_nextjs_contract_keeps_deferred_build_requirement(self):
        eval_run = load_eval_run()
        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "id": "nextjs-space-invaders-large",
                    "profile": "nextjs",
                    "prompt": "Create a Next.js app",
                    "expected_artifacts": [
                        "package.json",
                        "src/app/page.tsx",
                        "src/app/layout.tsx",
                        "src/app/global.d.ts",
                    ],
                    "postcheck": {
                        "commands": ["npm install --ignore-scripts", "npm run build"]
                    },
                },
            }
        )
        self.assertEqual(contract["profile"], "nextjs")
        self.assertEqual(contract["verify_commands"], [])
        self.assertEqual(
            contract["deferred_verify_requirements"][0]["command"], "npm run build"
        )
        self.assertEqual(
            contract["deferred_verify_requirements"][0]["status"],
            "blocked_by_dependency_setup",
        )

    def test_python_unittest_contract_keeps_verify_command(self):
        eval_run = load_eval_run()
        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "profile": "generic",
                    "prompt": "Create a Python linter",
                    "expected_artifacts": ["markdown_lint.py", "test_markdown_lint.py"],
                    "postcheck": {
                        "commands": ["python3 -m unittest test_markdown_lint.py"]
                    },
                },
            }
        )
        self.assertEqual(
            contract["verify_commands"], ["python3 -m unittest test_markdown_lint.py"]
        )
        self.assertNotIn("deferred_verify_requirements", contract)
        self.assertEqual(contract["required_capabilities"], [])

    def test_functional_contract_projects_runtime_capabilities_and_evidence(self):
        eval_run = load_eval_run()
        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "profile": "generic",
                    "prompt": "Fix a JavaScript date helper and add a deterministic node smoke check.",
                    "functional_contract": {
                        "category": "library-with-tests",
                        "required_capabilities": [
                            "implementation",
                            "deterministic_test",
                        ],
                    },
                    "expected_artifacts": ["date-helper.js"],
                    "postcheck": {"commands": ["node date-helper.js"]},
                },
            }
        )
        self.assertEqual(
            contract["required_capabilities"],
            ["implementation", "deterministic_test"],
        )
        self.assertIn("implementation_artifact", contract["required_evidence"])
        self.assertIn("test_artifact", contract["required_evidence"])
        self.assertIn("bound_verify_command", contract["required_evidence"])

    def test_docs_only_contract_allows_artifact_completion(self):
        eval_run = load_eval_run()
        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "profile": "generic",
                    "prompt": "Update README",
                    "expected_artifacts": ["README.md"],
                    "postcheck": {"commands": []},
                },
            }
        )
        self.assertEqual(contract["required_paths"], ["README.md"])
        self.assertEqual(contract["verify_commands"], [])
        self.assertNotIn("profile", contract)


if __name__ == "__main__":
    unittest.main()

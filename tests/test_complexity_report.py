from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "complexity_report.py"
BASELINE = REPO_ROOT / "workspace" / "v0.4.25" / "complexity-baseline.json"


class TestComplexityReport(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        self.src = self.root / "src"
        self.src.mkdir()

    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_reports_function_complexity_as_json(self) -> None:
        (self.src / "lib.rs").write_text(
            """
            fn simple() {
                if true && false {
                    loop { break; }
                }
            }

            fn with_match(value: Option<i32>) -> Result<i32, String> {
                let x = value.ok_or("missing")?;
                match x {
                    0 => Ok(0),
                    1 => Ok(1),
                    _ => Err("bad".to_string()),
                }
            }
            """,
            encoding="utf-8",
        )

        result = self._run("--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout)

        self.assertEqual(payload["files_analyzed"], 1)
        functions = {item["name"]: item for item in payload["top_functions"]}
        self.assertIn("simple", functions)
        self.assertIn("with_match", functions)
        self.assertGreaterEqual(functions["simple"]["rough_cc"], 4)
        self.assertGreaterEqual(functions["with_match"]["rough_cc"], 6)

    def test_ignores_comments_and_string_literals(self) -> None:
        (self.src / "lib.rs").write_text(
            '''
            fn example() {
                // if match for while loop && || =>
                let text = "if match for while loop && || => ?";
                let raw = r#"
                    if match for while loop && || => ?
                    { these braces should not change function extent }
                "#;
                let multiline = "if match for
                    while loop && || => ?
                    { these braces should not change function extent }";
                if text.is_empty() {
                    return;
                }
            }
            ''',
            encoding="utf-8",
        )

        result = self._run("--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout)
        function = payload["top_functions"][0]

        self.assertEqual(function["name"], "example")
        self.assertLess(function["rough_cc"], 5)

    def test_rejects_path_escape(self) -> None:
        outside = self.root.parent / "outside.rs"
        outside.write_text("fn outside() { if true {} }", encoding="utf-8")
        self.addCleanup(lambda: outside.unlink(missing_ok=True))

        result = self._run("--json", str(outside))
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout)

        self.assertEqual(payload["files_analyzed"], 0)
        self.assertEqual(payload["functions_analyzed"], 0)

    def test_checked_in_baseline_fixture_is_parseable(self) -> None:
        payload = json.loads(BASELINE.read_text(encoding="utf-8"))

        self.assertEqual(payload["baseline_schema_version"], 1)
        self.assertEqual(payload["script_schema_version"], 1)
        self.assertEqual(payload["threshold"], 15)
        self.assertEqual(payload["high_threshold"], 50)
        self.assertTrue(payload["command"].startswith("python3 scripts/complexity_report.py"))
        self.assertGreater(len(payload["file_metrics"]), 0)
        self.assertGreater(len(payload["top_functions"]), 0)
        self.assertEqual(payload["top_functions"][0]["name"], "run_actor_loop")


if __name__ == "__main__":
    unittest.main()

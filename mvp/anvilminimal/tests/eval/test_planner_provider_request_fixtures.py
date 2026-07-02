import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class PlannerProviderRequestFixturesTest(unittest.TestCase):
    def test_openai_request_fixture_preserves_system_and_user_contract(self):
        fixture = json.loads((ROOT / "eval/fixtures/planner_requests/openai-step-plan-request.json").read_text())
        self.assertEqual(fixture["provider"], "openai")
        self.assertEqual(fixture["tools"], [])
        self.assertEqual([item["role"] for item in fixture["input"]], ["system", "user"])
        system_terms = fixture["input"][0]["content"][0]["text_contains"]
        user_terms = fixture["input"][1]["content"][0]["text_contains"]
        self.assertIn("Return only one JSON object", system_terms)
        self.assertIn("Required final artifacts", user_terms)

    def test_gemini_request_fixture_uses_text_json_contract_without_tools(self):
        fixture = json.loads((ROOT / "eval/fixtures/planner_requests/gemini-step-plan-request.json").read_text())
        self.assertEqual(fixture["provider"], "gemini")
        self.assertEqual(fixture["tools"], [])
        terms = fixture["input_text_contains"]
        self.assertIn("system:", terms)
        self.assertIn("Return only one JSON object", terms)
        self.assertIn("Required final artifacts", terms)

    def test_ollama_request_fixture_preserves_message_roles_without_tools(self):
        fixture = json.loads((ROOT / "eval/fixtures/planner_requests/ollama-step-plan-request.json").read_text())
        self.assertEqual(fixture["provider"], "ollama")
        self.assertEqual(fixture["tools"], [])
        self.assertEqual([item["role"] for item in fixture["messages"]], ["system", "user"])
        self.assertIn("Return only one JSON object", fixture["messages"][0]["content_contains"])
        self.assertIn("Required final artifacts", fixture["messages"][1]["content_contains"])

    def test_request_fixtures_do_not_contain_credentials(self):
        text = "\n".join(path.read_text(encoding="utf-8") for path in (ROOT / "eval/fixtures/planner_requests").glob("*.json"))
        self.assertNotIn("OPENAI_API_KEY", text)
        self.assertNotIn("GEMINI_API_KEY", text)
        self.assertNotIn("sk-", text)
        self.assertNotIn("AIza", text)
        self.assertNotIn("/Users/", text)


if __name__ == "__main__":
    unittest.main()

import socket
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.postcheck import run_postcheck


class PostcheckDevServerTest(unittest.TestCase):
    def test_dev_server_readiness_and_shutdown(self):
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td) / "work"
            out = Path(td) / "out"
            workdir.mkdir()
            (workdir / "package.json").write_text("{}", encoding="utf-8")
            try:
                port = free_port()
            except PermissionError as err:
                self.skipTest(f"socket bind is not permitted in this sandbox: {err}")
            scenario = {
                "expected_artifacts": ["package.json"],
                "postcheck": {
                    "commands": [],
                    "dev_server": {
                        "command": f"{sys.executable} -m http.server {port}",
                        "port": port,
                        "readiness": {
                            "url": f"http://127.0.0.1:{port}/",
                            "expect_status": 200,
                            "timeout_sec": 10,
                        },
                        "shutdown": "signal",
                    },
                },
            }
            result = run_postcheck(scenario, workdir, out)
            self.assertTrue(result["ok"])
            self.assertTrue((out / "dev-server.stdout.log").exists())


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


if __name__ == "__main__":
    unittest.main()

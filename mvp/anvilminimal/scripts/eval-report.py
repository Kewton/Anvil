#!/usr/bin/env python3
from __future__ import annotations

import argparse
import sys
from pathlib import Path

from eval_lib.report import generate_report


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate markdown report for an eval run root.")
    parser.add_argument("--run-root", required=True)
    args = parser.parse_args()
    root = Path(args.run_root)
    text = generate_report(root)
    (root / "report.md").write_text(text, encoding="utf-8")
    print(root / "report.md")
    return 0


if __name__ == "__main__":
    sys.exit(main())


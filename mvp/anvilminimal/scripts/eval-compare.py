#!/usr/bin/env python3
from __future__ import annotations

import argparse
import sys
from pathlib import Path

from eval_lib.report import compare_summaries


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare two summary.eval.tsv files.")
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--experiment", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--fail-on-regression", action="store_true")
    args = parser.parse_args()
    text = compare_summaries(Path(args.baseline), Path(args.experiment))
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())


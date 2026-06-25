#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from eval_lib.postcheck import run_postcheck
from eval_lib.simple_yaml import load_yaml


def main() -> int:
    parser = argparse.ArgumentParser(description="Run deterministic postcheck for one eval scenario.")
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--workdir", required=True)
    parser.add_argument("--out", default="postcheck")
    args = parser.parse_args()
    scenario = load_yaml(args.scenario)
    result = run_postcheck(scenario, Path(args.workdir), Path(args.out))
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())


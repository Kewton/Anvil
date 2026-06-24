#!/usr/bin/env python3
import csv
import sys
from pathlib import Path

HEADER = ["run", "model", "case", "pam_variant", "rc", "elapsed_sec", "workdir", "session_copied", "extras_json"]

def read_summary(path: Path):
    with path.open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        if reader.fieldnames != HEADER:
            raise SystemExit(f"unsupported header in {path}: {reader.fieldnames}")
        return list(reader)

def by_case(rows):
    out = {}
    for row in rows:
        out.setdefault(row["case"], []).append(row)
    return out

def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: compare.py <baseline-summary.tsv> <experiment-summary.tsv>")
    base = by_case(read_summary(Path(sys.argv[1])))
    exp = by_case(read_summary(Path(sys.argv[2])))
    print("case\tbaseline_runs\texperiment_runs\tbaseline_rc0\texperiment_rc0")
    for case in sorted(set(base) | set(exp)):
        b = base.get(case, [])
        e = exp.get(case, [])
        print(
            f"{case}\t{len(b)}\t{len(e)}\t"
            f"{sum(1 for r in b if r['rc'] == '0')}\t"
            f"{sum(1 for r in e if r['rc'] == '0')}"
        )

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path

from eval_lib.artifacts import write_json
from eval_lib.runtime_trace import compare_trace_reports, read_json, write_trace_artifacts


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate normalized runtime semantics trace artifacts.")
    parser.add_argument("--run-root", type=Path, help="Eval run root containing summary.eval.tsv and runs/.")
    parser.add_argument(
        "--subject",
        choices=["mvp-anvilminimal", "source-anvildev"],
        help="Trace subject.",
    )
    parser.add_argument(
        "--binary-kind",
        choices=["anvilminimal", "anvildev"],
        help="Binary dialect used by the run.",
    )
    parser.add_argument("--binary-path", default="")
    parser.add_argument("--commit-sha", default="")
    parser.add_argument("--label", default="")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--compare-source-report", type=Path)
    parser.add_argument("--compare-mvp-report", type=Path)
    parser.add_argument("--diff-output", type=Path)
    args = parser.parse_args()

    if args.compare_source_report or args.compare_mvp_report:
        if not args.compare_source_report or not args.compare_mvp_report:
            raise SystemExit("--compare-source-report and --compare-mvp-report must be used together")
        source = read_json(args.compare_source_report, {})
        mvp = read_json(args.compare_mvp_report, {})
        diff = compare_trace_reports(source, mvp)
        output = args.diff_output or Path("runtime-semantics-trace-diff.json")
        write_json(output, diff)
        print(f"[write] {output}")
        return 0

    if not args.run_root or not args.subject or not args.binary_kind:
        raise SystemExit("--run-root, --subject, and --binary-kind are required unless comparing reports")
    report = write_trace_artifacts(
        args.run_root,
        subject=args.subject,
        binary_kind=args.binary_kind,
        binary_path=args.binary_path,
        label=args.label,
        commit_sha=args.commit_sha,
        output_dir=args.output_dir,
    )
    print(json.dumps({
        "report_path": report["report_path"],
        "manifest_path": report["manifest_path"],
        "normalized_event_sequence_path": report["normalized_event_sequence_path"],
    }, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

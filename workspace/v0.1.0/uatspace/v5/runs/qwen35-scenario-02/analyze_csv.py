#!/usr/bin/env python3
import argparse
import csv
from collections import defaultdict
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser(description="Summarize Amount totals from a CSV file.")
    parser.add_argument("csv_path", type=Path, help="CSV file with Category and Amount columns")
    parser.add_argument("--category", default="Category", help="category column name")
    parser.add_argument("--amount", default="Amount", help="amount column name")
    return parser.parse_args()


def main():
    args = parse_args()
    totals = defaultdict(float)
    with args.csv_path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        missing = {args.category, args.amount} - set(reader.fieldnames or [])
        if missing:
            raise SystemExit(f"missing required column(s): {', '.join(sorted(missing))}")
        for row in reader:
            category = (row.get(args.category) or "Uncategorized").strip() or "Uncategorized"
            raw_amount = (row.get(args.amount) or "0").replace(",", "").strip()
            totals[category] += float(raw_amount)

    grand_total = sum(totals.values())
    print("Category,Total")
    for category, total in sorted(totals.items()):
        print(f"{category},{total:.2f}")
    print(f"Grand Total,{grand_total:.2f}")


if __name__ == "__main__":
    main()

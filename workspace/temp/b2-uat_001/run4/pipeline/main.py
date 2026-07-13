import csv
import json
from collections import defaultdict

def run_pipeline():
    input_path = 'data/sales.csv'
    results_path = 'output/results.json'
    report_path = 'output/report.md'
    
    valid_regions = {'North', 'South', 'East', 'West'}
    
    input_rows = 0
    used_rows = 0
    excluded_reasons = defaultdict(int)
    aggregations = defaultdict(float)
    grand_total = 0.0

    try:
        with open(input_path, mode='r', encoding='utf-8') as f:
            reader = csv.DictReader(f)
            for row in reader:
                input_rows += 1
                
                # Validation: Date
                date_str = row.get('date', '').strip()
                if not date_str or len(date_str) < 7: # Basic YYYY-MM check
                    excluded_reasons['invalid_date'] += 1
                    continue
                
                # Extract Month (YYYY-MM)
                try:
                    month = date_str[:7]
                except Exception:
                    excluded_reasons['invalid_date'] += 1
                    continue

                # Validation: Region
                region = row.get('region', '').strip()
                if region not in valid_regions:
                    excluded_reasons['invalid_region'] += 1
                    continue

                # Validation: Amount
                amount_str = row.get('amount', '').strip()
                try:
                    amount = float(amount_str)
                    if amount < 0:
                        raise ValueError("Negative amount")
                except (ValueError, TypeError):
                    excluded_reasons['negative_amount'] += 1
                    continue

                # Valid Row processing
                used_rows += 1
                aggregations[(month, region)] += amount
                grand_total += amount

    except FileNotFoundError:
        print(f"Error: {input_path} not found.")
        return

    # Prepare aggregation list (sorted for determinism)
    agg_list = []
    for (month, region), total in sorted(aggregations.items()):
        agg_list.append({
            'month': month,
            'region': region,
            'total': total
        })

    # Prepare excluded rows list
    excluded_list = []
    for reason in sorted(excluded_reasons.keys()):
        excluded_list.append({
            'reason': reason,
            'count': excluded_reasons[reason]
        })

    results = {
        'input_rows': input_rows,
        'used_rows': used_rows,
        'excluded_rows': excluded_list,
        'aggregations': agg_list,
        'totals': {
            'grand_total': grand_total
        }
    }

    # Write results.json
    with open(results_path, 'w', encoding='utf-8') as f:
        json.dump(results, f, indent=2)

    # Write report.md
    with open(report_path, 'w', encoding='utf-8') as f:
        f.write("# Sales Summary Report\n\n")
        f.write(f"- Total Input Rows: {input_rows}\n")
        f.write(f"- Valid Rows Used: {used_rows}\n")
        f.write(f"- Grand Total Sales: {grand_total:.2f}\n\n")
        f.write("## Exclusions\n")
        if not excluded_list:
            f.write("No rows were excluded.\n")
        else:
            for item in excluded_list:
                f.write(f"- {item['reason']}: {item['count']}\n")
        f.write("\n## Monthly Aggregations\n")
        f.write("| Month | Region | Total |\n")
        f.write("|-------|--------|-------|\n")
        for item in agg_list:
            f.write(f"| {item['month']} | {item['region']} | {item['total']:.2f} |\n")

if __name__ == "__main__":
    run_pipeline()

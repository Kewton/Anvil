import csv
import json
from collections import defaultdict
from statistics import fsum

def main():
    input_file = 'data/sales.csv'
    results_file = 'output/results.json'
    report_file = 'output/report.md'

    # Deterministic state: no randomness used, stable iteration order via sorted keys
    input_rows = 0
    used_rows = 0
    excluded_rows = {
        'missing_value': 0,
        'invalid_format': 0
    }
    
    # aggregation: region -> month -> total_sales
    aggregation = defaultdict(lambda: defaultdict(float))
    total_sum = 0.0

    try:
        with open(input_file, 'r', encoding='utf-8') as f:
            reader = csv.DictReader(f)
            for row in reader:
                input_rows += 1
                
                # Validation
                try:
                    # Check for missing values in critical columns
                    if not row.get('date') or not row.get('region') or not row.get('amount'):
                        excluded_rows['missing_value'] += 1
                        continue
                    
                    # Format validation (basic)
                    amount = float(row['amount'])
                    # Date format assumed YYYY-MM-DD, extract YYYY-MM
                    date_val = row['date']
                    if len(date_val) < 7:
                        raise ValueError("Invalid date length")
                    month = date_val[:7]
                    region = row['region']
                    
                    # Aggregate
                    aggregation[region][month] += amount
                    total_sum += amount
                    used_rows += 1
                    
                except (ValueError, TypeError):
                    excluded_rows['invalid_format'] += 1
                    continue
    except FileNotFoundError:
        print(f"Error: {input_file} not found")
        return

    # Process aggregation into a stable sorted structure
    final_agg = {}
    sorted_regions = sorted(aggregation.keys())
    for region in sorted_regions:
        region_data = {}
        sorted_months = sorted(aggregation[region].keys())
        for month in sorted_months:
            region_data[month] = aggregation[region][month]
        final_agg[region] = region_data

    # Results JSON
    results = {
        "summary": {
            "input_rows": input_rows,
            "used_rows": used_rows,
            "excluded_rows": excluded_rows,
            "total_sales": total_sum
        },
        "aggregation": final_agg
    }
    
    with open(results_file, 'w', encoding='utf-8') as f:
        json.dump(results, f, indent=2, sort_keys=True)

    # Report MD
    with open(report_file, 'w', encoding='utf-8') as f:
        f.write("# Sales Summary Report\n\n")
        f.write(f"- Total Input Rows: {input_rows}\n")
        f.write(f"- Valid Rows Used: {used_rows}\n")
        f.write(f"- Excluded Rows:\n")
        for reason, count in sorted(excluded_rows.items()):
            f.write(f"  - {reason}: {count}\n")
        f.write(f"\n- Total Sales: {total_sum:.2f}\n\n")
        f.write("## Monthly Sales by Region\n\n")
        f.write("| Region | Month | Sales |\n")
        f.write("|---|---|---|\n")
        for region in sorted_regions:
            for month in sorted(final_agg[region].keys()):
                f.write(f"| {region} | {month} | {final_agg[region][month]:.2f} |\n")

if __name__ == "__main__":
    main()

import json
import subprocess
import os

def run_pipeline():
    print("Running pipeline/main.py...")
    result = subprocess.run(["python3", "pipeline/main.py"], capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Pipeline failed with error:\n{result.stderr}")
        return False
    return True

def verify():
    # 1. Execute pipeline
    if not run_pipeline():
        print("Verification failed: pipeline/main.py did not exit successfully.")
        return False

    # 2. Load output/results.json
    results_path = "output/results.json"
    if not os.path.exists(results_path):
        print(f"Verification failed: {results_path} not found.")
        return False

    try:
        with open(results_path, "r") as f:
            data = json.load(f)
    except Exception as e:
        print(f"Verification failed: {results_path} is not valid JSON. Error: {e}")
        return False

    # 3. Assert JSON schema/validity (Basic check)
    required_keys = ["input_rows", "used_rows", "excluded_rows", "aggregations", "totals"]
    for key in required_keys:
        if key not in data:
            print(f"Verification failed: Key {key} missing from results.json")
            return False

    # 4. Assert input_rows == used_rows + sum(excluded_rows counts)
    input_rows = data["input_rows"]
    used_rows = data["used_rows"]
    excluded_count = sum(item["count"] for item in data["excluded_rows"])
    
    if input_rows != used_rows + excluded_count:
        print(f"Verification failed: Input rows ({input_rows}) != used ({used_rows}) + excluded ({excluded_count})")
        return False

    # 5. Assert aggregations list is non-empty and contains month, region, total keys
    aggs = data["aggregations"]
    if not aggs:
        print("Verification failed: aggregations list is empty.")
        return False
    
    for entry in aggs:
        if not all(k in entry for k in ("month", "region", "total")):
            print(f"Verification failed: aggregation entry {entry} missing required keys.")
            return False

    # 6. Assert output/report.md exists and is non-empty
    report_path = "output/report.md"
    if not os.path.exists(report_path):
        print(f"Verification failed: {report_path} not found.")
        return False
    
    if os.path.getsize(report_path) == 0:
        print(f"Verification failed: {report_path} is empty.")
        return False

    print("Verification successful!")
    return True

if __name__ == "__main__":
    if verify():
        exit(0)
    else:
        exit(1)

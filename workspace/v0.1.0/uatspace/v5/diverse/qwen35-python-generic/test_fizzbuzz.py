#!/usr/bin/env python3
"""FizzBuzz CLI のテスト"""

import subprocess
import sys


def run_cli(limit: int) -> str:
    result = subprocess.run(
        [sys.executable, "main.py", "--limit", str(limit)],
        capture_output=True,
        text=True,
        check=True
    )
    return result.stdout


def test_fizzbuzz():
    output = run_cli(5)
    expected = ["1", "2", "Fizz", "4", "Buzz"]
    lines = output.strip().split("\n")
    assert lines == expected
    print("✓ limit=5 のテスト成功")

    output = run_cli(15)
    lines = output.strip().split("\n")
    expected_15 = ["1", "2", "Fizz", "4", "Buzz", "Fizz", "7", "8", "Fizz", "Buzz", "11", "Fizz", "13", "FizzBuzz", "15"]
    assert lines == expected_15
    print("✓ limit=15 のテスト成功")

    try:
        subprocess.run([sys.executable, "main.py", "--limit", "-1"], capture_output=True, text=True, check=False)
        assert False
    except subprocess.CalledProcessError:
        print("✓ negative limit のエラー処理テスト成功")


def test_help():
    result = subprocess.run([sys.executable, "main.py", "--help"], capture_output=True, text=True)
    assert result.returncode == 0
    assert "--limit" in result.stdout
    print("✓ --help のテスト成功")


if __name__ == "__main__":
    print("FizzBuzz CLI テストを実行中...\n")
    test_fizzbuzz()
    test_help()
    print("\n✓ すべてのテストが成功しました！")

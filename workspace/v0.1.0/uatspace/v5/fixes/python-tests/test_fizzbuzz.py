#!/usr/bin/env python3
import subprocess
import sys


def test_fizzbuzz_limit_15():
    result = subprocess.run(
        [sys.executable, "fizzbuzz.py", "--limit", "15"],
        check=True,
        text=True,
        capture_output=True,
    )
    assert result.stdout.strip().splitlines() == [
        "1", "2", "Fizz", "4", "Buzz", "Fizz", "7", "8", "Fizz", "Buzz",
        "11", "Fizz", "13", "14", "FizzBuzz",
    ]


if __name__ == "__main__":
    test_fizzbuzz_limit_15()
    print("python smoke ok")

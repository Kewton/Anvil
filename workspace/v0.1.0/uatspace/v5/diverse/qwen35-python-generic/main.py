#!/usr/bin/env python3
"""FizzBuzz CLI ツール"""

import argparse
import sys


def fizzbuzz(n: int) -> list[str]:
    """1 から n までの FizzBuzz を生成する"""
    result = []
    for i in range(1, n + 1):
        if i % 15 == 0:
            result.append("FizzBuzz")
        elif i % 5 == 0:
            result.append("Fizz")
        elif i % 3 == 0:
            result.append("Buzz")
        else:
            result.append(str(i))
    return result


def main():
    parser = argparse.ArgumentParser(description="FizzBuzz CLI ツール")
    parser.add_argument("--limit", type=int, required=True, help="出力する最大の数字")
    args = parser.parse_args()

    if args.limit < 1:
        print("エラー：limit は 1 以上である必要があります", file=sys.stderr)
        sys.exit(1)

    output = fizzbuzz(args.limit)
    for line in output:
        print(line)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""FizzBuzz CLI - outputs FizzBuzz sequence up to a given limit."""

import argparse


def fizzbuzz(limit: int) -> list[str]:
    """Return a list of FizzBuzz strings from 1 to limit."""
    result = []
    for i in range(1, limit + 1):
        if i % 15 == 0:
            result.append("FizzBuzz")
        elif i % 3 == 0:
            result.append("Fizz")
        elif i % 5 == 0:
            result.append("Buzz")
        else:
            result.append(str(i))
    return result


def main():
    parser = argparse.ArgumentParser(description="Output FizzBuzz sequence.")
    parser.add_argument(
        "--limit",
        type=int,
        default=100,
        help="Upper limit for FizzBuzz sequence (default: 100)",
    )
    args = parser.parse_args()

    for line in fizzbuzz(args.limit):
        print(line)


if __name__ == "__main__":
    main()

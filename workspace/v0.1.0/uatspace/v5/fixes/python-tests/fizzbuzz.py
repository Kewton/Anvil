#!/usr/bin/env python3
"""FizzBuzz CLI - prints FizzBuzz sequence up to a given limit."""

import argparse


def fizzbuzz(n: int) -> str:
    """Return FizzBuzz value for n."""
    if n % 15 == 0:
        return "FizzBuzz"
    if n % 3 == 0:
        return "Fizz"
    if n % 5 == 0:
        return "Buzz"
    return str(n)


def main():
    parser = argparse.ArgumentParser(description="Print FizzBuzz sequence.")
    parser.add_argument(
        "--limit", type=int, default=100,
        help="Upper limit of the sequence (default: 100)"
    )
    args = parser.parse_args()
    for i in range(1, args.limit + 1):
        print(fizzbuzz(i))


if __name__ == "__main__":
    main()

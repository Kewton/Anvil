# Anvil Tester Skill v1 fixture (Issue #459 / Phase 2 E2E).
#
# A bare .py file with no pytest config / no tests/ dir, so the Tester Skill
# takes the Python branch via `TesterCandidate::detect`. The fixture mirrors
# the smallest "library + main" layout users open new repos with.


def greet(name: str) -> str:
    """Return a friendly greeting for the given name."""
    return f"hello, {name}!"


if __name__ == "__main__":
    print(greet("world"))

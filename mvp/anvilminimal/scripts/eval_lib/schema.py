from __future__ import annotations

from typing import Any


EVAL_SCHEMA_VERSION = "eval-summary-v3-failure-gate"
NOT_AVAILABLE = "not_available"


def not_available_if_empty(value: Any) -> Any:
    if value in {"", None}:
        return NOT_AVAILABLE
    return value

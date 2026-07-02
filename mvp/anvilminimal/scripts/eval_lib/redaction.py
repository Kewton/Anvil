from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Any


SECRET_PATTERNS = [
    re.compile(r"sk-[A-Za-z0-9_\-]{8,}"),
    re.compile(r"AIza[0-9A-Za-z_\-]{8,}"),
    re.compile(r"Bearer\s+[A-Za-z0-9._\-]+", re.IGNORECASE),
    re.compile(r"request[_ -]?id[:=]\s*[A-Za-z0-9._\-]+", re.IGNORECASE),
]


def redact_text(value: str) -> str:
    out = value
    for key in ("OPENAI_API_KEY", "GEMINI_API_KEY"):
        secret = os.environ.get(key)
        if secret:
            out = out.replace(secret, f"<{key}>")
    home = str(Path.home())
    if home and home != "/":
        out = out.replace(home, "<HOME>")
    for pattern in SECRET_PATTERNS:
        out = pattern.sub(_redacted_match, out)
    return out


def redact_json(value: Any) -> Any:
    if isinstance(value, str):
        return redact_text(value)
    if isinstance(value, list):
        return [redact_json(item) for item in value]
    if isinstance(value, dict):
        return {key: redact_json(item) for key, item in value.items()}
    return value


def _redacted_match(match: re.Match[str]) -> str:
    text = match.group(0)
    if text.lower().startswith("bearer"):
        return "Bearer <REDACTED>"
    if "request" in text.lower():
        return "request_id=<REDACTED>"
    if text.startswith("sk-"):
        return "sk-<REDACTED>"
    if text.startswith("AIza"):
        return "AIza<REDACTED>"
    return "<REDACTED>"

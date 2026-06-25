from __future__ import annotations

import ast
import json
from pathlib import Path
from typing import Any


def load_yaml(path: str | Path) -> Any:
    text = Path(path).read_text(encoding="utf-8")
    return loads_yaml(text)


def loads_yaml(text: str) -> Any:
    try:
        import yaml  # type: ignore

        data = yaml.safe_load(text)
        return {} if data is None else data
    except ModuleNotFoundError:
        pass
    stripped = text.strip()
    if not stripped:
        return {}
    if stripped[0] in "[{":
        return json.loads(stripped)
    return _TinyYaml(stripped.splitlines()).parse()


def parse_scalar(value: Any) -> Any:
    if not isinstance(value, str):
        return value
    value = value.strip()
    if value == "":
        return ""
    if value in {"[]", "{}"}:
        return [] if value == "[]" else {}
    lower = value.lower()
    if lower in {"true", "false"}:
        return lower == "true"
    if lower in {"null", "none", "~"}:
        return None
    if value.startswith("[") and value.endswith("]"):
        return [_parse_flow_scalar(part.strip()) for part in _split_flow(value[1:-1])]
    if value.startswith("{") and value.endswith("}"):
        out: dict[str, Any] = {}
        for part in _split_flow(value[1:-1]):
            key, sep, rest = part.partition(":")
            if not sep:
                continue
            out[key.strip().strip('"').strip("'")] = _parse_flow_scalar(rest.strip())
        return out
    if value.startswith("[") or value.startswith("{"):
        try:
            return ast.literal_eval(value)
        except Exception:
            try:
                return json.loads(value)
            except Exception:
                return value
    if (value.startswith('"') and value.endswith('"')) or (
        value.startswith("'") and value.endswith("'")
    ):
        try:
            return ast.literal_eval(value)
        except Exception:
            return value[1:-1]
    try:
        return int(value)
    except ValueError:
        pass
    try:
        return float(value)
    except ValueError:
        return value


def _split_flow(text: str) -> list[str]:
    parts: list[str] = []
    current: list[str] = []
    depth = 0
    quote: str | None = None
    for ch in text:
        if quote:
            current.append(ch)
            if ch == quote:
                quote = None
            continue
        if ch in {"'", '"'}:
            quote = ch
            current.append(ch)
            continue
        if ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    if current:
        parts.append("".join(current).strip())
    return parts


def _parse_flow_scalar(value: str) -> Any:
    if value.startswith("[") or value.startswith("{"):
        return parse_scalar(value)
    return parse_scalar(value)


class _TinyYaml:
    """A small fallback parser for the fixture subset used by eval.

    It intentionally supports only dictionaries, lists, simple scalars, inline
    arrays/maps, and one-line key/value list items. PyYAML is used whenever it is
    installed; this fallback keeps the scripts usable in lean environments.
    """

    def __init__(self, lines: list[str]) -> None:
        self.lines = [line.rstrip("\n") for line in lines if line.strip() and not line.lstrip().startswith("#")]
        self.index = 0

    def parse(self) -> Any:
        value, _ = self._parse_block(0)
        return value

    def _parse_block(self, indent: int) -> tuple[Any, int]:
        if self.index >= len(self.lines):
            return {}, self.index
        current = self.lines[self.index]
        if self._indent(current) < indent:
            return {}, self.index
        if current.lstrip().startswith("- "):
            return self._parse_list(indent)
        return self._parse_dict(indent)

    def _parse_dict(self, indent: int) -> tuple[dict[str, Any], int]:
        out: dict[str, Any] = {}
        while self.index < len(self.lines):
            line = self.lines[self.index]
            cur = self._indent(line)
            if cur < indent:
                break
            if cur > indent:
                break
            text = line.strip()
            if text.startswith("- "):
                break
            key, sep, rest = text.partition(":")
            if not sep:
                self.index += 1
                continue
            key = key.strip()
            rest = rest.strip()
            self.index += 1
            if rest in {"|", ">"}:
                out[key] = self._parse_multiline(cur + 2)
            elif rest:
                out[key] = parse_scalar(rest)
            else:
                child, _ = self._parse_block(cur + 2)
                out[key] = child
        return out, self.index

    def _parse_list(self, indent: int) -> tuple[list[Any], int]:
        out: list[Any] = []
        while self.index < len(self.lines):
            line = self.lines[self.index]
            cur = self._indent(line)
            if cur < indent or cur > indent:
                break
            text = line.strip()
            if not text.startswith("- "):
                break
            item = text[2:].strip()
            self.index += 1
            if not item:
                child, _ = self._parse_block(cur + 2)
                out.append(child)
            elif ":" in item and not item.startswith(("'", '"')):
                key, _, rest = item.partition(":")
                obj: dict[str, Any] = {key.strip(): parse_scalar(rest.strip()) if rest.strip() else {}}
                while self.index < len(self.lines):
                    nxt = self.lines[self.index]
                    nxt_indent = self._indent(nxt)
                    if nxt_indent <= cur:
                        break
                    if nxt_indent == cur + 2 and not nxt.strip().startswith("- ") and ":" in nxt:
                        k, _, r = nxt.strip().partition(":")
                        self.index += 1
                        if r.strip():
                            obj[k.strip()] = parse_scalar(r.strip())
                        else:
                            child, _ = self._parse_block(nxt_indent + 2)
                            obj[k.strip()] = child
                    else:
                        break
                out.append(obj)
            else:
                out.append(parse_scalar(item))
        return out, self.index

    def _parse_multiline(self, indent: int) -> str:
        parts: list[str] = []
        while self.index < len(self.lines):
            line = self.lines[self.index]
            if self._indent(line) < indent:
                break
            parts.append(line[indent:])
            self.index += 1
        return "\n".join(parts).strip()

    @staticmethod
    def _indent(line: str) -> int:
        return len(line) - len(line.lstrip(" "))

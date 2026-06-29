from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml


PLAN_CAPABILITY_ORACLE_VERSION = "plan-capability-v1"


@dataclass(frozen=True)
class CapabilityRule:
    name: str
    prompt: tuple[str, ...]
    plan: tuple[str, ...]
    source: tuple[str, ...]
    verify: tuple[str, ...]
    acceptance_capabilities: tuple[str, ...] = ()
    profiles: tuple[str, ...] = ()


CAPABILITY_RULES: tuple[CapabilityRule, ...] = (
    CapabilityRule(
        name="render_loop_or_canvas",
        prompt=(r"\bcanvas\b", r"\bgame\s+loop\b", r"60\s*fps", r"描画"),
        plan=(r"\bcanvas\b", r"html5\s+canvas", r"game\s+loop", r"requestanimationframe", r"60\s*fps", r"描画"),
        source=(r"\bcanvas\b", r"getcontext\s*\(", r"requestanimationframe", r"setinterval\s*\("),
        verify=(r"\bcanvas\b", r"getcontext", r"requestanimationframe", r"animation\s*frame", r"render\s+loop"),
        acceptance_capabilities=("stateful_interaction",),
        profiles=("nextjs-game",),
    ),
    CapabilityRule(
        name="keyboard_or_player_control",
        prompt=(r"\bkeyboard\b", r"\bcontrols?\b", r"\bkey\s*(?:down|up|press|board)?\b", r"\barrow", r"\bwasd\b", r"キーボード", r"操作"),
        plan=(r"\bkeyboard\b", r"\bkey\s*(?:down|up|press|board)?\b", r"\barrow(?:left|right|up|down)\b", r"\bwasd\b", r"\bplayer\s+control", r"キーボード", r"操作"),
        source=(r"onkeydown", r"onkeyup", r"addeventlistener\s*\(\s*['\"]keydown", r"addeventlistener\s*\(\s*['\"]keyup", r"arrowleft", r"arrowright", r"arrowup", r"arrowdown", r"\bwasd\b", r"setplayer"),
        verify=(r"keydown", r"keyup", r"keyboard", r"arrowleft", r"arrowright", r"\bwasd\b", r"player\s+control"),
        acceptance_capabilities=("player_control", "user_input_or_action"),
    ),
    CapabilityRule(
        name="player_entity",
        prompt=(r"\bplayer\b", r"\bship\b", r"\bcannon\b", r"自機", r"プレイヤー"),
        plan=(r"\bplayer\b", r"\bship\b", r"\bcannon\b", r"自機", r"プレイヤー"),
        source=(r"\bplayer\b", r"\bship\b", r"\bcannon\b", r"setplayer", r"自機"),
        verify=(r"\bplayer\b", r"\bship\b", r"\bcannon\b", r"player\s+entity"),
        acceptance_capabilities=("player_control",),
    ),
    CapabilityRule(
        name="adversary_entity",
        prompt=(r"\benem(?:y|ies)\b", r"\binvaders?\b", r"\balien", r"\bwave\b", r"\bchallenge\b", r"敵"),
        plan=(r"\benem(?:y|ies)\b", r"\binvader", r"\balien", r"\bwave\b", r"\bspawn", r"\bchallenge\b", r"敵"),
        source=(r"\benem(?:y|ies)\b", r"\binvader", r"\balien", r"\bwave\b", r"\bspawn", r"setenem", r"敵"),
        verify=(r"\benem(?:y|ies)\b", r"\binvader", r"\balien", r"\bwave\b", r"\bspawn", r"\bchallenge\b"),
        acceptance_capabilities=("adversary_or_challenge",),
    ),
    CapabilityRule(
        name="projectile_or_shooting",
        prompt=(r"\bbullet", r"\blaser", r"\bprojectile", r"\bshoot", r"\bfire\b", r"\bspace\s+invaders?\b", r"弾", r"発射"),
        plan=(r"\bbullet", r"\blaser", r"\bprojectile", r"\bshoot", r"\bfire\b", r"弾", r"発射"),
        source=(r"\bbullet", r"\blaser", r"\bprojectile", r"\bshoot", r"\bfire\b", r"setbullet", r"弾"),
        verify=(r"\bbullet", r"\blaser", r"\bprojectile", r"\bshoot", r"\bfire\b"),
        acceptance_capabilities=("adversary_or_challenge",),
    ),
    CapabilityRule(
        name="collision_or_failure_rule",
        prompt=(r"\bcollision", r"\bcollide", r"\bhit\b", r"\bdamage", r"\bgame\s*over", r"\blives?\b", r"当たり", r"衝突"),
        plan=(r"\bcollision", r"\bcollide", r"\bhit\b", r"\bdamage", r"\bgame\s*over", r"\blives?\b", r"当たり", r"衝突"),
        source=(r"\bcollision", r"\bcollide", r"\bhit\b", r"\bdamage", r"\bgame\s*over", r"\blives?\b", r"当たり", r"衝突", r"\.x\s*[<>]=?\s*[^&|;\n]+\.x", r"\.y\s*[<>]=?\s*[^&|;\n]+\.y"),
        verify=(r"\bcollision", r"\bcollide", r"\bhit\b", r"\bdamage", r"\bgame\s*over", r"\blives?\b", r"intersect", r"overlap"),
        acceptance_capabilities=("failure_or_collision_rule",),
    ),
    CapabilityRule(
        name="score_or_progression",
        prompt=(r"\bscore\b", r"\bpoints\b", r"\bscoreboard\b", r"\blevel\b", r"\bstage\b", r"\bcombo\b", r"\bupgrade\b", r"\bshop\b", r"\blives?\b", r"スコア"),
        plan=(r"\bscore\b", r"\bpoints\b", r"\bscoreboard\b", r"\blevel\b", r"\bstage\b", r"\bcombo\b", r"\bupgrade\b", r"\bshop\b", r"\blives?\b", r"スコア"),
        source=(r"\bscore\b", r"\bpoints?\b", r"\bscoreboard\b", r"\blevel\b", r"\bstage\b", r"\bcombo\b", r"\bupgrade\b", r"\bshop\b", r"\blives?\b", r"setscore", r"スコア"),
        verify=(r"\bscore\b", r"\bpoints\b", r"\bscoreboard\b", r"\blevel\b", r"\bstage\b", r"\bcombo\b", r"\blives?\b", r"progress"),
        acceptance_capabilities=("progression_or_score", "start_or_restart_flow"),
    ),
    CapabilityRule(
        name="audio_feedback",
        prompt=(r"\baudio\b", r"\bsound\b", r"\bsfx\b", r"web\s+audio", r"音"),
        plan=(r"\baudio\b", r"\bsound\b", r"\bsfx\b", r"web\s+audio", r"音"),
        source=(r"\baudio\b", r"\bsound\b", r"\bsfx\b", r"audiocontext", r"new\s+audio\s*\(", r"音"),
        verify=(r"\baudio\b", r"\bsound\b", r"\bsfx\b", r"audiocontext"),
    ),
    CapabilityRule(
        name="visual_effects",
        prompt=(r"\bparticle", r"\bexplosion", r"\btrail", r"\beffect", r"パーティクル", r"演出"),
        plan=(r"\bparticle", r"\bexplosion", r"\btrail", r"\beffect", r"パーティクル", r"演出"),
        source=(r"\bparticle", r"\bexplosion", r"\btrail", r"\beffect", r"パーティクル", r"演出"),
        verify=(r"\bparticle", r"\bexplosion", r"\btrail", r"\beffect"),
    ),
    CapabilityRule(
        name="cli_entrypoint",
        prompt=(r"\bcli\b", r"command\s+line", r"\bterminal\b", r"コマンド"),
        plan=(r"\bcli\b", r"command\s+line", r"\bentry\s+point\b", r"\bmain\b", r"\bargparse\b", r"\bclap\b"),
        source=(r"if\s+__name__\s*==", r"\bfn\s+main\b", r"\bmain\s*\(", r"process\.argv", r"argparse", r"\bclap\b"),
        verify=(r"\bcargo\s+test\b", r"\bnode\b", r"python\d?\s+-m\s+unittest", r"\bpytest\b", r"\bassert\b"),
        acceptance_capabilities=("entrypoint",),
    ),
    CapabilityRule(
        name="library_functionality",
        prompt=(r"\blibrary\b", r"\bfunction\b", r"\bmodule\b", r"ライブラリ"),
        plan=(r"\blibrary\b", r"\bfunction\b", r"\bmodule\b", r"export\s+function", r"\bclass\b"),
        source=(r"\bdef\s+", r"\bfn\s+", r"\bfunction\s+", r"export\s+function", r"\bclass\s+"),
        verify=(r"\btest\b", r"\bassert\b", r"\bunittest\b", r"\bpytest\b", r"\bcargo\s+test\b"),
        acceptance_capabilities=("implementation",),
    ),
    CapabilityRule(
        name="deterministic_test",
        prompt=(r"\bunit\s+test", r"\bself-test\b", r"\bdeterministic\b", r"\bunittest\b", r"\bpytest\b", r"\bcargo\s+test\b"),
        plan=(r"\bunit\s+test", r"\bself-test\b", r"\bdeterministic\b", r"\bunittest\b", r"\bpytest\b", r"\bcargo\s+test\b", r"\bassert\b"),
        source=(r"\bunittest\b", r"\bpytest\b", r"#\[test\]", r"\bassert\b", r"\bdescribe\s*\(", r"\bit\s*\("),
        verify=(r"\bunittest\b", r"\bpytest\b", r"\bcargo\s+test\b", r"\bnpm\s+test\b", r"\bassert\b"),
        acceptance_capabilities=("deterministic_test", "deterministic_check"),
    ),
    CapabilityRule(
        name="docs_requested_content",
        prompt=(r"\breadme\b", r"\bdocs\b", r"\bdocumentation\b", r"\busage\b", r"\bexample\b", r"ドキュメント", r"説明"),
        plan=(r"\breadme\b", r"\bdocs\b", r"\bdocumentation\b", r"\busage\b", r"\bexample\b", r"ドキュメント", r"説明"),
        source=(r"\busage\b", r"\bexample\b", r"\bcommand\b", r"\breadme\b", r"使い方", r"例"),
        verify=(r"\breadme\b", r"\bdocs/", r"\bgrep\b", r"\btest\s+-f\b"),
        acceptance_capabilities=("requested_content",),
    ),
    CapabilityRule(
        name="data_transform_contract",
        prompt=(r"\btransform\b", r"\bconvert\b", r"\bparse\b", r"\bschema\b", r"変換"),
        plan=(r"\btransform\b", r"\bconvert\b", r"\bparse\b", r"\bschema\b", r"\binput\b", r"\boutput\b"),
        source=(r"\binput\b", r"\boutput\b", r"\bparse\b", r"\btransform\b", r"\bconvert\b", r"\bschema\b"),
        verify=(r"\binput\b", r"\boutput\b", r"\bassert\b", r"\bunittest\b", r"\bpytest\b", r"\bnode\b"),
        acceptance_capabilities=("input_output_contract",),
    ),
)


SOURCE_PATTERN_BY_CAPABILITY = {rule.name: rule.source for rule in CAPABILITY_RULES}
PLAN_PATTERN_BY_CAPABILITY = {rule.name: rule.plan for rule in CAPABILITY_RULES}
VERIFY_PATTERN_BY_CAPABILITY = {rule.name: rule.verify for rule in CAPABILITY_RULES}


def score_plan_capability_contract(
    *,
    scenario: dict[str, Any],
    plan_paths: list[Path] | None = None,
    plan_data: dict[str, Any] | None = None,
) -> dict[str, Any]:
    plan_text, plan_items, parse_errors = collect_plan_contract(plan_paths or [], plan_data)
    if not plan_text:
        return not_applicable("no_plan_contract", scenario=scenario, parse_errors=parse_errors)

    prompt_caps, prompt_sources = extract_prompt_capabilities(scenario)
    plan_caps, plan_sources = extract_capabilities(plan_text, "plan")
    prompt_set = set(prompt_caps)
    plan_set = set(plan_caps)
    if not prompt_caps and not plan_caps:
        return not_applicable("no_capability_contract", scenario=scenario, parse_errors=parse_errors)

    missing = sorted(prompt_set.difference(plan_set))
    coverage = score_ratio(len(prompt_set.intersection(plan_set)), len(prompt_set))
    evidence = score_evidence_completeness(plan_items, plan_caps)
    obligation_alignment = score_obligation_alignment(plan_items, plan_caps)
    vague_penalty = vague_promise_penalty(plan_text, plan_caps)
    contract_score = round(
        max(0.0, min(100.0, 0.45 * coverage + 0.25 * evidence + 0.30 * obligation_alignment - vague_penalty)),
        1,
    )
    gap_kind = prompt_plan_gap_kind(prompt_caps, plan_caps, missing, plan_text)
    return {
        "plan_capability_contract_score": contract_score,
        "plan_capability_oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
        "prompt_plan_capability_coverage_score": coverage,
        "prompt_plan_missing_capability_count": len(missing),
        "plan_required_capability_count": len(plan_caps),
        "prompt_plan_gap_kind": gap_kind,
        "plan_required_capabilities": plan_caps,
        "prompt_required_capabilities": prompt_caps,
        "prompt_plan_missing_capabilities": missing,
        "capability_contract_details": {
            "applicable": True,
            "oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
            "prompt_sources": prompt_sources,
            "plan_sources": plan_sources,
            "evidence_completeness_score": evidence,
            "obligation_alignment_score": obligation_alignment,
            "vague_promise_penalty": vague_penalty,
            "parse_errors": parse_errors,
            "plan_items": [
                {
                    "id": item.get("id", ""),
                    "kind": item.get("kind", ""),
                    "capabilities": item.get("capabilities", []),
                    "expected_paths": item.get("expected_paths", []),
                    "artifact_roles": item.get("artifact_roles", []),
                    "verify": item.get("verify", []),
                }
                for item in plan_items
            ],
        },
    }


def not_applicable(reason: str, *, scenario: dict[str, Any], parse_errors: list[str]) -> dict[str, Any]:
    prompt_caps, prompt_sources = extract_prompt_capabilities(scenario)
    return {
        "plan_capability_contract_score": "",
        "plan_capability_oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
        "prompt_plan_capability_coverage_score": "",
        "prompt_plan_missing_capability_count": "",
        "plan_required_capability_count": "",
        "prompt_plan_gap_kind": reason,
        "plan_required_capabilities": [],
        "prompt_required_capabilities": prompt_caps,
        "prompt_plan_missing_capabilities": [],
        "capability_contract_details": {
            "applicable": False,
            "reason": reason,
            "oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
            "prompt_sources": prompt_sources,
            "parse_errors": parse_errors,
        },
    }


def collect_plan_contract(plan_paths: list[Path], plan_data: dict[str, Any] | None = None) -> tuple[str, list[dict[str, Any]], list[str]]:
    chunks: list[str] = []
    items: list[dict[str, Any]] = []
    parse_errors: list[str] = []
    data_items: list[Any] = []
    if plan_data is not None:
        data_items.append(plan_data)
    for path in plan_paths:
        try:
            data_items.append(load_yaml(path))
        except Exception as err:
            parse_errors.append(f"{path}: {err}")
            try:
                chunks.append(path.read_text(encoding="utf-8", errors="replace"))
            except OSError as read_err:
                parse_errors.append(f"{path}: {read_err}")
    for data in data_items:
        text, extracted = plan_contract_text_and_items(data)
        chunks.append(text)
        items.extend(extracted)
    for item in items:
        caps, _ = extract_capabilities(item.get("text", ""), "plan")
        item["capabilities"] = caps
        item["artifact_roles"] = sorted({artifact_role_for_path(path) for path in item.get("expected_paths", [])})
    return "\n".join(chunk for chunk in chunks if chunk).lower(), items, parse_errors


def plan_contract_text_and_items(data: Any) -> tuple[str, list[dict[str, Any]]]:
    if not isinstance(data, dict):
        return json.dumps(data, ensure_ascii=False), []
    if isinstance(data.get("steps"), list):
        parts = [str(data.get("goal", ""))]
        items = []
        for step in data.get("steps") or []:
            if not isinstance(step, dict):
                continue
            text = "\n".join(
                [
                    str(step.get("id", "")),
                    str(step.get("kind", "")),
                    str(step.get("instruction", "")),
                    str(step.get("expected_result", "")),
                    json.dumps(step.get("expected_paths", []), ensure_ascii=False),
                    json.dumps(step.get("verify", []), ensure_ascii=False),
                ]
            )
            parts.append(text)
            items.append(
                {
                    "id": str(step.get("id", "")),
                    "kind": str(step.get("kind", "")),
                    "text": text.lower(),
                    "expected_paths": [str(path) for path in step.get("expected_paths", []) or []],
                    "verify": [str(command) for command in step.get("verify", []) or []],
                }
            )
        return "\n".join(parts), items
    if isinstance(data.get("phases"), list):
        parts = [str(data.get("goal", "")), str(data.get("profile", ""))]
        items = []
        for phase in data.get("phases") or []:
            if not isinstance(phase, dict):
                continue
            text = "\n".join(
                [
                    str(phase.get("id", "")),
                    str(phase.get("prompt", "")),
                    str(phase.get("intent", "")),
                    json.dumps(phase.get("verify", []), ensure_ascii=False),
                ]
            )
            parts.append(text)
            items.append(
                {
                    "id": str(phase.get("id", "")),
                    "kind": "phase",
                    "text": text.lower(),
                    "expected_paths": [],
                    "verify": [str(command) for command in phase.get("verify", []) or []],
                }
            )
        return "\n".join(parts), items
    return json.dumps(data, ensure_ascii=False), []


def extract_prompt_capabilities(scenario: dict[str, Any]) -> tuple[list[str], dict[str, list[str]]]:
    text = "\n".join(
        [
            str(scenario.get("prompt", "")),
            str(scenario.get("profile", "")),
            str(scenario.get("category", "")),
        ]
    ).lower()
    required = set()
    sources: dict[str, list[str]] = {}
    functional = scenario.get("functional_contract") or {}
    for acceptance_capability in functional.get("required_capabilities", []) or []:
        for rule in CAPABILITY_RULES:
            if str(acceptance_capability) in rule.acceptance_capabilities:
                required.add(rule.name)
                sources.setdefault(rule.name, []).append(f"functional_contract:{acceptance_capability}")
    category = str(functional.get("category") or "").lower()
    profile = str(scenario.get("profile", "")).lower()
    if category == "interactive-game" or ("game" in text or "ゲーム" in text):
        for name in ["render_loop_or_canvas", "keyboard_or_player_control", "player_entity", "score_or_progression"]:
            required.add(name)
            sources.setdefault(name, []).append("domain:interactive-game")
    if profile == "nextjs" and ("game" in text or "ゲーム" in text):
        sources.setdefault("render_loop_or_canvas", []).append("profile:nextjs-game")
    caps, pattern_sources = extract_capabilities(text, "prompt")
    for cap in caps:
        required.add(cap)
        sources.setdefault(cap, []).extend(pattern_sources.get(cap, []))
    return sorted(required), {key: sorted(set(value)) for key, value in sources.items()}


def extract_capabilities(text: str, source_kind: str) -> tuple[list[str], dict[str, list[str]]]:
    lowered = text.lower()
    found: list[str] = []
    sources: dict[str, list[str]] = {}
    for rule in CAPABILITY_RULES:
        patterns = getattr(rule, source_kind)
        snippets = matching_snippets(lowered, patterns)
        if snippets:
            found.append(rule.name)
            sources[rule.name] = snippets
    return sorted(dict.fromkeys(found)), sources


def required_capabilities_from_plan_text(plan_text: str) -> list[str]:
    return extract_capabilities(plan_text, "plan")[0]


def source_capability_detected(name: str, source_text: str) -> bool:
    patterns = SOURCE_PATTERN_BY_CAPABILITY.get(name, ())
    return any(re.search(pattern, source_text, re.IGNORECASE) for pattern in patterns)


def verify_capability_detected(name: str, verify_text: str) -> bool:
    patterns = VERIFY_PATTERN_BY_CAPABILITY.get(name, ())
    return any(re.search(pattern, verify_text, re.IGNORECASE) for pattern in patterns)


def matching_snippets(text: str, patterns: tuple[str, ...]) -> list[str]:
    out: list[str] = []
    for pattern in patterns:
        match = re.search(pattern, text, re.IGNORECASE)
        if not match:
            continue
        start = max(0, match.start() - 32)
        end = min(len(text), match.end() + 32)
        out.append(text[start:end].replace("\n", " ")[:120])
    return out


def score_evidence_completeness(plan_items: list[dict[str, Any]], plan_caps: list[str]) -> float:
    if not plan_caps:
        return 0.0
    capability_scores = []
    for cap in plan_caps:
        matching = [item for item in plan_items if cap in item.get("capabilities", [])]
        if not matching:
            capability_scores.append(0.0)
            continue
        best = 0.0
        for item in matching:
            score = 40.0
            roles = set(item.get("artifact_roles", []))
            if roles.intersection({"implementation", "verification", "acceptance_evidence"}):
                score += 30.0
            if item.get("verify"):
                score += 30.0
            best = max(best, score)
        capability_scores.append(best)
    return round(sum(capability_scores) / len(capability_scores), 1)


def score_obligation_alignment(plan_items: list[dict[str, Any]], plan_caps: list[str]) -> float:
    if not plan_caps:
        return 0.0
    capability_scores: list[float] = []
    for cap in plan_caps:
        matching = [item for item in plan_items if cap in item.get("capabilities", [])]
        if not matching:
            capability_scores.append(0.0)
            continue
        best = 0.0
        for item in matching:
            roles = set(item.get("artifact_roles", []))
            score = 0.0
            if roles.intersection({"implementation", "verification"}):
                score += 70.0
            elif roles.intersection({"setup", "scaffold", "style"}):
                score += 20.0
            if item.get("verify"):
                score += 30.0
            best = max(best, min(100.0, score))
        capability_scores.append(best)
    return round(sum(capability_scores) / len(capability_scores), 1)


def artifact_role_for_path(path: str) -> str:
    lower = str(path).strip().lower()
    name = lower.rsplit("/", 1)[-1]
    if name in {
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "cargo.toml",
        "cargo.lock",
        "pyproject.toml",
        "requirements.txt",
        "tsconfig.json",
    } or name.startswith(("next.config", "postcss.config", "tailwind.config", "vite.config")):
        return "setup"
    if lower.endswith((".css", ".scss", ".sass", ".less")):
        return "style"
    if lower.endswith(".d.ts") or lower.endswith("layout.tsx") or lower.endswith("layout.jsx"):
        return "scaffold"
    if (
        "/test" in lower
        or lower.startswith("test")
        or ".test." in lower
        or ".spec." in lower
        or "smoke" in name
        or name.endswith("_test.py")
        or name.endswith("_test.rs")
    ):
        return "verification"
    if lower.endswith(".md"):
        return "acceptance_evidence"
    if lower.endswith((".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".py", ".rs", ".go", ".java")):
        return "implementation"
    return "scaffold"


def vague_promise_penalty(plan_text: str, plan_caps: list[str]) -> float:
    if not plan_caps:
        return 0.0
    vague_hits = sum(1 for token in ["polished", "awesome", "rich", "great", "面白", "かっこいい"] if token in plan_text)
    concrete_hits = sum(1 for cap in plan_caps if cap in plan_text)
    if concrete_hits >= len(plan_caps) // 2:
        return 0.0
    return min(15.0, vague_hits * 5.0)


def prompt_plan_gap_kind(prompt_caps: list[str], plan_caps: list[str], missing: list[str], plan_text: str) -> str:
    if not prompt_caps:
        return "prompt_capability_not_applicable"
    if not plan_caps:
        return "plan_too_generic_for_prompt"
    if missing:
        return "prompt_capability_missing_from_plan"
    if len(plan_text.strip()) < 120:
        return "plan_too_generic_for_prompt"
    return ""


def score_ratio(hit_count: int, total_count: int) -> float:
    if total_count <= 0:
        return 100.0
    return round(100.0 * hit_count / total_count, 1)

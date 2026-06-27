from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml
from .source_semantic_oracle import collect_source_corpus


PLAN_OUTPUT_ORACLE_VERSION = "plan-output-v1"


CAPABILITY_RULES = [
    {
        "name": "render_loop_or_canvas",
        "plan": [
            r"\bcanvas\b",
            r"html5\s+canvas",
            r"game\s+loop",
            r"requestanimationframe",
            r"60\s*fps",
            r"描画",
        ],
        "source": [
            r"\bcanvas\b",
            r"getcontext\s*\(",
            r"requestanimationframe",
            r"setinterval\s*\(",
        ],
    },
    {
        "name": "keyboard_or_player_control",
        "plan": [
            r"\bkeyboard\b",
            r"\bkey\s*(?:down|up|press|board)?\b",
            r"\barrow(?:left|right|up|down)\b",
            r"\bwasd\b",
            r"\bplayer\s+control",
            r"キーボード",
            r"操作",
        ],
        "source": [
            r"onkeydown",
            r"onkeyup",
            r"addeventlistener\s*\(\s*['\"]keydown",
            r"addeventlistener\s*\(\s*['\"]keyup",
            r"arrowleft",
            r"arrowright",
            r"arrowup",
            r"arrowdown",
            r"\bwasd\b",
            r"setplayer",
        ],
    },
    {
        "name": "player_entity",
        "plan": [
            r"\bplayer\b",
            r"\bship\b",
            r"\bcannon\b",
            r"自機",
            r"プレイヤー",
        ],
        "source": [
            r"\bplayer\b",
            r"\bship\b",
            r"\bcannon\b",
            r"setplayer",
            r"自機",
        ],
    },
    {
        "name": "adversary_entity",
        "plan": [
            r"\benem(?:y|ies)\b",
            r"\binvader",
            r"\balien",
            r"\bwave\b",
            r"\bspawn",
            r"敵",
        ],
        "source": [
            r"\benem(?:y|ies)\b",
            r"\binvader",
            r"\balien",
            r"\bwave\b",
            r"\bspawn",
            r"setenem",
            r"敵",
        ],
    },
    {
        "name": "projectile_or_shooting",
        "plan": [
            r"\bbullet",
            r"\blaser",
            r"\bprojectile",
            r"\bshoot",
            r"\bfire\b",
            r"弾",
            r"発射",
        ],
        "source": [
            r"\bbullet",
            r"\blaser",
            r"\bprojectile",
            r"\bshoot",
            r"\bfire\b",
            r"setbullet",
            r"弾",
        ],
    },
    {
        "name": "collision_or_failure_rule",
        "plan": [
            r"\bcollision",
            r"\bcollide",
            r"\bhit\b",
            r"\bdamage",
            r"\bgame\s*over",
            r"\blives?\b",
            r"当たり",
            r"衝突",
        ],
        "source": [
            r"\bcollision",
            r"\bcollide",
            r"\bhit\b",
            r"\bdamage",
            r"\bgame\s*over",
            r"\blives?\b",
            r"当たり",
            r"衝突",
            r"\.x\s*[<>]=?\s*[^&|;\n]+\.x",
            r"\.y\s*[<>]=?\s*[^&|;\n]+\.y",
        ],
    },
    {
        "name": "score_or_progression",
        "plan": [
            r"\bscore",
            r"\bpoints\b",
            r"\blevel",
            r"\bstage",
            r"\bcombo",
            r"\bupgrade",
            r"\bshop\b",
            r"\blives?\b",
            r"スコア",
        ],
        "source": [
            r"\bscore",
            r"\bpoints?",
            r"\blevel",
            r"\bstage",
            r"\bcombo",
            r"\bupgrade",
            r"\bshop\b",
            r"\blives?\b",
            r"setscore",
            r"スコア",
        ],
    },
    {
        "name": "audio_feedback",
        "plan": [
            r"\baudio\b",
            r"\bsound",
            r"\bsfx\b",
            r"web\s+audio",
            r"音",
        ],
        "source": [
            r"\baudio\b",
            r"\bsound",
            r"\bsfx\b",
            r"audiocontext",
            r"new\s+audio\s*\(",
            r"音",
        ],
    },
    {
        "name": "visual_effects",
        "plan": [
            r"\bparticle",
            r"\bexplosion",
            r"\btrail",
            r"\beffect",
            r"パーティクル",
            r"演出",
        ],
        "source": [
            r"\bparticle",
            r"\bexplosion",
            r"\btrail",
            r"\beffect",
            r"パーティクル",
            r"演出",
        ],
    },
]


def evaluate_plan_output_adherence(
    *,
    plan_paths: list[Path],
    workdir: Path,
    scenario: dict[str, Any],
) -> dict[str, Any]:
    plan_text = collect_plan_text(plan_paths)
    if not plan_text:
        return not_applicable("no_plan_contract")

    required = required_capabilities_from_plan(plan_text)
    if not required:
        return not_applicable("no_plan_output_capabilities")

    corpus = collect_source_corpus(workdir, scenario)
    source_text = "\n".join(source for _, source in corpus).lower()
    capability_results = {
        name: source_capability_detected(name, source_text)
        for name in required
    }
    missing = [name for name, ok in capability_results.items() if not ok]
    score = round(100.0 * (len(required) - len(missing)) / max(1, len(required)), 1)
    return {
        "plan_output_adherence_success": not missing,
        "plan_output_adherence_score": score,
        "plan_output_failure_kind": "plan_output_missing_required_capabilities" if missing else "",
        "plan_output_oracle_version": PLAN_OUTPUT_ORACLE_VERSION,
        "plan_output_details": {
            "applicable": True,
            "plan_paths": [str(path) for path in plan_paths],
            "files_scanned": [path for path, _ in corpus],
            "required_capabilities": required,
            "capabilities": capability_results,
            "missing_capabilities": missing,
        },
    }


def not_applicable(reason: str) -> dict[str, Any]:
    return {
        "plan_output_adherence_success": "",
        "plan_output_adherence_score": "",
        "plan_output_failure_kind": "",
        "plan_output_oracle_version": PLAN_OUTPUT_ORACLE_VERSION,
        "plan_output_details": {"applicable": False, "reason": reason},
    }


def collect_plan_text(plan_paths: list[Path]) -> str:
    chunks: list[str] = []
    for path in plan_paths:
        try:
            data = load_yaml(path)
            chunks.append(plan_contract_text(data))
        except Exception:
            try:
                chunks.append(path.read_text(encoding="utf-8", errors="replace"))
            except OSError:
                continue
    return "\n".join(chunks).lower()


def plan_contract_text(data: Any) -> str:
    if isinstance(data, dict):
        if isinstance(data.get("steps"), list):
            parts = [str(data.get("goal", ""))]
            for step in data.get("steps") or []:
                if isinstance(step, dict):
                    parts.extend(
                        [
                            str(step.get("id", "")),
                            str(step.get("kind", "")),
                            str(step.get("instruction", "")),
                            str(step.get("expected_result", "")),
                            json.dumps(step.get("expected_paths", []), ensure_ascii=False),
                            json.dumps(step.get("verify", []), ensure_ascii=False),
                        ]
                    )
            return "\n".join(parts)
        if isinstance(data.get("phases"), list):
            parts = [str(data.get("goal", "")), str(data.get("profile", ""))]
            for phase in data.get("phases") or []:
                if isinstance(phase, dict):
                    parts.extend(
                        [
                            str(phase.get("id", "")),
                            str(phase.get("prompt", "")),
                            str(phase.get("intent", "")),
                            json.dumps(phase.get("verify", []), ensure_ascii=False),
                        ]
                    )
            return "\n".join(parts)
    return json.dumps(data, ensure_ascii=False)


def required_capabilities_from_plan(plan_text: str) -> list[str]:
    required: list[str] = []
    for rule in CAPABILITY_RULES:
        if any(re.search(pattern, plan_text, re.IGNORECASE) for pattern in rule["plan"]):
            required.append(rule["name"])
    return required


def source_capability_detected(name: str, source_text: str) -> bool:
    rule = next((rule for rule in CAPABILITY_RULES if rule["name"] == name), None)
    if not rule:
        return False
    return any(re.search(pattern, source_text, re.IGNORECASE) for pattern in rule["source"])

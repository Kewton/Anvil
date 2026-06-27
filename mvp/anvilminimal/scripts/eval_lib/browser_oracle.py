from __future__ import annotations

from pathlib import Path
from typing import Any

from .acceptance_contract import contract_from_scenario


def evaluate_browser_oracle(
    scenario: dict[str, Any],
    workdir: Path,
    *,
    enabled: bool = False,
) -> dict[str, Any]:
    """Browser acceptance hook.

    The default eval path is deterministic and dependency-light. Browser checks
    are exposed as an explicit adapter point so acceptance-required suites can
    enable them without making smoke eval depend on Playwright availability.
    """

    contract = contract_from_scenario(scenario)
    required = "browser_interaction" in (contract.oracle_contract.get("deterministic_oracles") or [])
    if not enabled:
        return {
            "browser_success": "",
            "browser_failure_kind": "",
            "browser_details": {
                "applicable": required,
                "status": "not_enabled",
                "workdir": str(workdir),
            },
        }
    return {
        "browser_success": "",
        "browser_failure_kind": "browser_adapter_not_implemented",
        "browser_details": {
            "applicable": required,
            "status": "adapter_not_implemented",
            "workdir": str(workdir),
        },
    }

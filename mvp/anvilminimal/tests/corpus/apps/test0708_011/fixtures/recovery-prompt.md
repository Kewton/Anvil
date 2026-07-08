Recover this failed run by producing and executing a focused ultra plan.

Original goal:
あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。

Profile: nextjs

Failure scope:
- phase: game-engine-core
- step: unknown
- kind: phase_execute_error

Failure evidence:
- challenge_or_adversary_evidence: wire a reachable challenge, obstacle, enemy, timer, or comparable adversary into state evolution
- failure_or_collision_evidence: implement a reachable failure, collision, timeout, or loss condition that changes visible state
- interactive_ui_source_evidence: keep the interactive implementation route-bound from the page entrypoint, not stranded in an unimported component
- non_static_screen_evidence: make the rendered screen change from state, input, timer, or progression instead of staying static
- restart_or_recoverable_state_evidence: add data-anvil-action="restart" to every restart/retry/new-game affordance (game-over, victory, and in-play when present) and ensure it resets observable state; the initial primary action alone is not restart evidence
- score_or_progression_evidence: make score, level, progress, or win/loss state change from meaningful gameplay or interaction
- stateful_update_evidence: update visible state over time or directly from user input, and expose the updated snapshot in data-anvil-state
- user_input_handler_evidence: wire keyboard, pointer, click, touch, or form handlers to route-bound state changes
- exhaustion classification: recoverable tool error repeated: edit_anchor_not_found

Missing paths:
- none

Missing capabilities:
- challenge_or_adversary_evidence
- failure_or_collision_evidence
- interactive_ui_source_evidence
- non_static_screen_evidence
- restart_or_recoverable_state_evidence
- score_or_progression_evidence
- stateful_update_evidence
- user_input_handler_evidence

Verification commands:
- none

Changed paths:
- none

Repair targets:
- implementation

Required recovery action:
- Inspect the current workspace state first.
- Preserve already useful artifacts.
- Create or repair the missing implementation artifacts.
- Use deterministic verification.
- Do not treat scaffold-only or build-only output as complete.

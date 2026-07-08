Recover this failed run by producing and executing a focused ultra plan.

Original goal:
あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。

Profile: nextjs

Failure scope:
- phase: build-verification
- step: unknown
- kind: final_acceptance_repair_failed

Failure evidence:
- render-loop checklist: ref attached -> effect runs -> rAF loop starts -> draw calls
- input-wiring checklist: keyboard or pointer input must visibly change game state (player position, projectiles, score/health, or state transitions); wire input handlers into the render/update loop.
- loop_progress_exhausted: no concrete blocker recorded

Missing paths:
- none

Missing capabilities:
- restart_or_recoverable_state_evidence

Verification commands:
- none

Changed paths:
- none

Repair targets:
- input_state_render_wiring

Required recovery action:
- Inspect the current workspace state first.
- Preserve already useful artifacts.
- Create or repair the missing implementation artifacts.
- Use deterministic verification.
- Do not treat scaffold-only or build-only output as complete.

# E2E/UAT Run Manifest

        - run_id: `photon-on-expanded-seeded-20260512-130654`
        - commit: `39b3a3f89651ffb3f1664d60f6a10699d7c83f6c`
        - models: `qwen3.6:27b-coding-nvfp4, qwen3.5:122b`
        - sidecar_model: `qwen3-coder:30b`
        - reps: `3`
        - max_iterations: `50`
        - photon_on: `False`

        ## Scenarios

        - S0-01: README answer-only, no edits
- S1-01: Architecture review, no edits
- S1-02: Run local script and summarize output
- S2-02: Creative greenfield Next.js game
- S2-03: Existing SvelteKit route edit
- S2-06: Unknown UI framework safe fail
- S3-01: Python bug fix with self-test
- S3-03: Multi-file Python dependency fix
- S3-04: Fix existing failing test
- S4-02: Log analysis and action plan
- S5-01: ANVIL.md preferred verifier
- S6-04: Secret-looking value redaction

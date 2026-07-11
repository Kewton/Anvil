# Integration Notes

## Repair Pressure Integration (2026-07-11)

The following pre-existing gaps were observed while consolidating repair
pressure. They are recorded here because this integration preserves behavior
and does not change thresholds, events, payloads, or terminal reasons.

### No-Progress Does Not Raise Write Pressure

The no-progress streak drives `no_progress_feedback` and the
`model_stagnation:no_progress_recorded` terminal path, but it does not activate
`write_required`. Repeated no-progress turns can therefore terminate without a
write-required intervention. The state-machine transition table and unit test
preserve this gap explicitly. Closing it requires a separate measured task with
a non-regression gate.

### Dependency Setup Can Stall Network-Restricted Tests

`plan_run_nextjs_game_setup_only_fails_inferred_obligation` can enter the real
Node dependency-setup path. Under a network-restricted test environment,
`npm install` can wait for the existing 600-second setup timeout. The test
passes when dependency setup can complete. This integration does not change
the setup decision or timeout.

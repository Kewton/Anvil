# Bootstrap and Config

## Goal

Define the first startup path and config responsibilities for the Rust implementation.

## Bootstrap Sequence

Recommended startup order:

1. parse CLI arguments
2. initialize minimal logging
3. load config file and environment overrides
4. derive effective runtime config
5. initialize filesystem paths and session directories
6. initialize provider backend
7. initialize app state and session layer
8. initialize TUI shell
9. enter interactive or one-shot mode

## Config Sources

Priority order:

1. CLI arguments
2. environment variables
3. config file
4. defaults

## Config Categories

### Runtime

- active provider
- active model
- sidecar model
- context window
- token limits
- temperature

### Mode

- interactive vs one-shot
- approval behavior
- reasoning visibility mode
- resume behavior
- debug or verbose logging

### Paths

- config directory
- state directory
- session directory
- history file

### UX

- TUI mode toggles
- status verbosity
- optional color and terminal behavior switches

## Derived Config

The config layer may derive:

- effective context window from model choice
- sidecar fallback behavior
- default provider backend
- approval defaults
- reasoning visibility defaults
- session file locations

Derived config should become a typed `EffectiveConfig` rather than being recomputed throughout the app.

`EffectiveConfig` should carry stable runtime settings.
Provider capability discovery that depends on the live backend should remain a separate typed runtime object, not be mixed into static config.

Representative split:

- `EffectiveConfig`: chosen model, sidecar, paths, approval mode, UX settings
- `ProviderCapabilities`: streaming support, tool-call support, context limits as observed from the live backend

## Failure Rules

- invalid config should fail early with a compact error
- unsafe config should be normalized or rejected explicitly
- filesystem setup failures should surface before the interactive loop starts

## First-Version Recommendation

The first Rust version should support:

- config file loading
- env overrides
- CLI overrides
- effective config rendering for debug/status
- explicit separation between config-derived values and runtime-discovered provider capabilities

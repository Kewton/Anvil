# Local Agent Architecture

The runtime separates configuration, model selection, turn execution, tools,
session storage, and quality recovery.

## Current Decision

Mode policy should remain structured runtime state. It should not be appended
as user-visible conversation history.

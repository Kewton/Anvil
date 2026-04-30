# Payment CLI

This small project calculates invoice totals from CSV input. It keeps parsing,
validation, and output formatting separate so the command can be tested without
spawning a shell process.

## Risks

- CSV column names are currently assumed to be stable.
- Error messages should be clearer for non-technical users.
- Large files are processed in memory.

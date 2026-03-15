# Later Expansion Plan

## Completed Slice

- Custom slash command extensions
  - load project-local commands from `.anvil/slash-commands.json`
  - surface them in `/help`
  - dispatch them into normal live provider turns

- Richer planning and execution flows
  - `/plan-add <item>`
  - `/plan-focus <n>`
  - `/plan-clear`
  - keep plan state in the visible operator console snapshot

- Additional local model backends
  - OpenAI-compatible backend support
  - normalized into the same provider contract as Ollama
  - covered by config/bootstrap and provider integration tests

## Next Slices

- Improved large-repo retrieval
  - repository indexing
  - retrieval scoring
  - compaction and summary snapshots for long sessions

- Advanced UX without losing clarity
  - richer tool-progress display
  - session timeline inspection
  - more expressive but still legible status views

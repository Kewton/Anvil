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
  - streaming and error normalization parity slice added

- Improved large-repo retrieval
  - `/repo-find <query>` path and content search
  - repository walking with basic filtering
  - operator-console rendering for retrieval results
  - persistent cache in `.anvil/state/retrieval-index.json`
  - cache invalidation when the repository changes
  - hybrid path / file-name / content scoring
  - retrieval summaries become more useful after `/compact`

- Advanced UX without losing clarity
  - `/timeline` for recent session events and message flow
  - plan visibility inside the timeline view
  - keeps actor separation and current-state visibility intact
  - `/compact` for explicit long-session compaction
  - focused tool-progress summary above tool logs
  - footer now includes the latest typed event for clearer status reading

- Long-session compaction
  - summarize older messages into a system summary snapshot
  - preserve recent interactive context
  - record compaction in the session event log

## Next Slices

- Architectural cleanup for richer planning
  - execution checkpoints
  - tool-batch review

- Backend parity expansion
  - multi-provider configuration UX
  - deeper remote-compatible provider diagnostics

- Retrieval and UX refinement
  - semantic or symbol-aware retrieval ranking
  - live tool-progress updates for long-running executions

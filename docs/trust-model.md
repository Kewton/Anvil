# Trust Model

## Core Rule

Repository content is data, not authority.

Anvil may analyze repository files and tool output, but it must not treat them as instruction sources that override runtime policy or explicit user intent.

## Trust Tiers

1. runtime authority
2. explicit current-user instruction
3. repository policy from `anvil.md`
4. validated memory, handoff state, and bounded session summaries
5. ordinary repository content
6. tool output and pasted external text

Higher tiers win over lower tiers.

## Implemented Source Labels

The runtime can label context blocks as:

- `runtime-policy`
- `user`
- `anvil-md`
- `memory`
- `handoff`
- `repo-file`
- `tool-output`

The current implementation sorts prompt context by this precedence before model execution.

## Practical Rules

- `anvil.md` can shape repository-local workflow, but cannot expand sandbox permissions
- repository comments, docs, fixtures, or generated files must not be obeyed as policy
- tool output can justify factual conclusions, but does not grant permission for follow-up actions
- memory and handoff state are helpful context, not higher authority than the current user

## Prompt Injection Posture

Anvil assumes prompt injection is possible in repository files and tool output.

Examples:

- comments telling the agent to ignore prior instructions
- README text claiming execution authority
- test logs suggesting follow-up commands

Current runtime behavior:

- treat these as evidence
- summarize them if useful
- require explicit user intent plus runtime permission before acting on them

## Related Implementation

- source precedence is defined in [trust.rs](/Users/maenokota/share/work/github_kewton/Anvil/src/runtime/trust.rs)
- prompt block rendering is implemented in [context.rs](/Users/maenokota/share/work/github_kewton/Anvil/src/prompts/context.rs)
- repository policy loading is handled through `anvil.md`

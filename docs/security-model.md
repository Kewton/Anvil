# Security Model

Anvil is a local-first coding agent for Ollama. The model runs locally, but the
agent can still perform powerful local actions through tools. This document
defines the security boundary for users and contributors.

## Trust Boundary

Trusted:

- the local user account running Anvil
- the selected workspace contents
- the local Ollama server on localhost

Not fully trusted:

- model output
- shell output interpreted by the model
- repository files from untrusted sources
- instructions embedded in issues, logs, docs, or source files

## Tool Capabilities

Anvil may:

- read workspace files
- write or edit workspace files
- run shell commands through Bash
- store sessions and logs outside the workspace state root

The Bash tool is the highest-risk capability. Approval prompts and command
classification reduce accidental execution, but they do not turn model output
into trusted code.

## Hardening Rules

Security-sensitive code should stay deterministic where possible:

- path confinement
- localhost URL validation
- dangerous command blocking
- approval prompts
- secret masking in logs
- parser recovery for malformed tool calls

Heuristic or model-assisted judgment is appropriate for ambiguous product
decisions, but not as the only guard for security boundaries.

## Verifier Selection

Verifier selection should be evidence-based. A candidate can come from a safe
`ANVIL.md` command, a manifest, a package script, an existing test surface, or a
compile-only fallback. The chosen verifier should expose its source and evidence
in logs or feedback so failures can be evaluated without guessing which pattern
matched.

Do not treat incidental strings as executable proof. For example, a top-level
`"test"` key in `package.json` is not equivalent to `scripts.test`.

Protocol success should also stay evidence-based. Deterministic recovery writes
may unblock a turn, but they are not proof that the requested implementation was
completed; the protocol must still require an appropriate artifact and verifier
path.

## Safer Operation

Recommended defaults for untrusted repositories:

```bash
anvil --fresh-session
```

Avoid `--yes` unless you trust the repository and prompt. Review proposed Bash
commands before approving them. Keep secrets out of prompts and fixture files.

## Photon サイドカーの trust boundary

Photon サイドカーは localhost 専用の外部プロセスとして扱われる。

**Trust boundary**:

context_pack レスポンスから生成されたプロンプトセクションには
`[Photon External Memory — untrusted, read-only context]` ラベルが付与される。
Photon が返すデータはモデル出力と同様に untrusted として扱われ、prompt injection 検査パイプラインを通過した上でのみプロンプトに注入される。

**Secret masking**:

context_pack リクエストには `mask_secrets`（トークン・KV・URL 形式のシークレット除去）と `mask_payload_inplace`（ペイロード全体への最終防衛線マスク）が適用される。これはシークレット情報の除去であり匿名化ではない。非シークレットの機微情報（タスク内容、ファイルパス、working memory 等）は送信される可能性がある。

**Localhost constraint**:

`validate_localhost_url` により、`ANVIL_PHOTON_URL` は localhost / 127.0.0.1 / ::1 かつ http/https スキームかつ credentials なしの URL のみ許可される。外部ホストへの送信はできない。

**shadow_mode / canary の役割**:

`ANVIL_PHOTON_SHADOW_MODE` と `ANVIL_PHOTON_CANARY` はプロンプト注入の gate であり、privacy gate ではない。mapper 経路（データ収集用）は shadow_mode=true でも送信を継続する。全送信を停止するには `ANVIL_PHOTON_ENABLED=false` または `--offline` を使用する。

**pre-turn 経路のペイロード**:

pre-turn hook（プロンプト注入用）が送信するペイロードは `session_id` と `turn_index` のみ（minimal）。リッチなコンテキスト（task / working_memory 等）は mapper 経路のみが送信する。

## Known Limits

- A local model can still suggest unsafe changes or commands.
- Logs may contain file contents or command output.
- `--yes` increases automation and reduces interactive review.
- Anvil does not sandbox arbitrary commands beyond its own approvals and path
  checks.

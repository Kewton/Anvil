# Anvil

A local terminal coding agent powered by [Ollama](https://ollama.com) and OpenAI-compatible backends.
Anvil runs entirely on your machine — no cloud dependency required.

---

## Installation

Install with a single command. Choose the one that matches your platform:

**macOS (Apple Silicon / ARM64)**
```bash
curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-darwin-arm64.gz | gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil
```

**macOS (Intel / x86_64)**
```bash
curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-darwin-amd64.gz | gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil
```

**Linux (x86_64)**
```bash
curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-linux-amd64.gz | gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil
```

**Linux (ARM64)**
```bash
curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-linux-arm64.gz | gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil
```

> If `/usr/local/bin` requires root access, use `sudo`:
> ```bash
> curl -sL <URL> | gunzip -c | sudo tee /usr/local/bin/anvil > /dev/null && sudo chmod +x /usr/local/bin/anvil
> ```

Verify:
```bash
anvil --version
```

All binaries are published at [AnvilBinary Releases](https://github.com/Kewton/AnvilBinary/releases).

---

## Quick Start

### 1. Start a local LLM backend

**Ollama (recommended, free)**
```bash
# Install from https://ollama.com
ollama serve
ollama pull qwen2.5-coder:32b
```

**OpenAI-compatible API (LM Studio, vLLM, etc.)**
```bash
# Start your server, then point Anvil at it
anvil --provider openai --provider-url http://localhost:1234 --model your-model
```

### 2. Run Anvil in your project

```bash
cd /path/to/your/project
anvil --model qwen2.5-coder:32b
```

You'll see an interactive prompt:

```
    ___              _ __
   /   |  ____ _   _(_) /_
  / /| | / __ \ | / / / __/
 / ___ |/ / / / |/ / / /_
/_/  |_/_/ /_/|___/_/\__/

  local coding agent for serious terminal work

  Model   : qwen2.5-coder:32b
  Context : 200k
  Mode    : local / confirm

  [U] you >
```

---

## Features

### Tools

Anvil exposes the following tools to the LLM:

| Tool | Permission | Description |
|------|-----------|-------------|
| `file.read` | Auto | Read files and list directories |
| `file.search` | Auto | Search by filename or content |
| `file.write` | Confirm | Create or overwrite files |
| `file.edit` | Confirm | Apply targeted in-place edits |
| `file.rewrite` | Confirm | Replace a line-range block |
| `shell.exec` | Confirm | Execute shell commands (output streamed live) |

### Approval Flow

In default mode, Anvil asks for confirmation before writing files or running commands:

```
  Allow shell.exec: cargo test? [y/n]
```

| Input | Behavior |
|-------|----------|
| `y` / `yes` | Execute the tool |
| `n` / anything else | Deny (LLM is notified as "denied by user") |

Use `--no-approval` to skip all prompts.

### Slash Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/status` | Show current state |
| `/plan` | Display active plan |
| `/plan-add <item>` | Add item to plan |
| `/plan-focus <n>` | Set active step |
| `/plan-clear` | Clear plan |
| `/checkpoint <memo>` | Save a checkpoint |
| `/repo-find <query>` | Search repository |
| `/timeline` | Show session timeline |
| `/compact` | Compress old history |
| `/model` | Show current model |
| `/provider` | Show provider info |
| `/reset` | Return to ready state |
| `/exit` | End session |

### Session Persistence

Anvil automatically saves sessions per project directory (`.anvil/sessions/`).
Restart in the same directory to resume where you left off.

```bash
anvil --model qwen2.5-coder:32b          # resumes last session
anvil --model qwen2.5-coder:32b --fresh-session   # start fresh
```

### Safety

The following commands are always blocked regardless of approval mode:
- `rm -rf /` / `rm -rf ~` (recursive root/home deletion)
- `mkfs` (disk format)
- `dd if=` (raw disk write)
- `:(){` (fork bomb)

File paths are sandboxed — absolute paths, `..` traversal, and symlink escapes are rejected.

---

## Configuration

### Config File

Create `.anvil/config` in your project root:

```ini
provider = ollama
model = qwen2.5-coder:32b
provider_url = http://127.0.0.1:11434
context_window = 200000
stream = true
```

### Environment Variables

```bash
ANVIL_PROVIDER=ollama             # Provider: ollama | openai | lmstudio
ANVIL_MODEL=qwen2.5-coder:32b     # Model name
ANVIL_PROVIDER_URL=http://...     # Provider base URL
ANVIL_CONTEXT_WINDOW=200000       # Context window size (tokens)
ANVIL_CONTEXT_BUDGET=50000        # Explicit token budget
ANVIL_MAX_AGENT_ITERATIONS=30     # Max agentic loop iterations (default: 30)
ANVIL_HTTP_TIMEOUT=300            # LLM request timeout (seconds)
ANVIL_API_KEY=sk-...              # API key for OpenAI-compatible backends
```

### CLI Options

```
anvil [OPTIONS]

  -p, --provider <PROVIDER>              Provider (ollama|openai|lmstudio)
  -m, --model <MODEL>                    Model name
  -u, --provider-url <URL>               Provider base URL
      --sidecar-model <MODEL>            Sidecar model for summarization
      --context-window <SIZE>            Context window size
      --context-budget <TOKENS>          Explicit token budget
      --max-iterations <N>               Max agentic loop iterations (default: 30)
      --no-stream                        Disable streaming
      --debug                            Enable debug logging
      --no-approval                      Auto-approve all tools
      --fresh-session                    Start a new session
      --oneshot                          Non-interactive mode (pipe-friendly)
      --reasoning-visibility <LEVEL>     Reasoning display level (hidden|summary)
  -h, --help                             Print help
  -V, --version                          Print version
```

Priority order: CLI > environment variables > config file > defaults

### API Key Security

Set API keys as environment variables, not in the config file:

```bash
export ANVIL_API_KEY=sk-...
```

If an API key is found in `.anvil/config`, Anvil will display a warning at startup.
Add `.anvil/` to your `.gitignore` to prevent accidental commits:

```
# .gitignore
.anvil/
```

### Custom Slash Commands

Define project-specific slash commands in `.anvil/slash-commands.json`:

```json
{
  "commands": [
    {
      "name": "/review",
      "description": "Run a code review",
      "prompt": "Review the recent changes in this repository and point out areas for improvement."
    }
  ]
}
```

---

## Provider Support

| Provider | Example |
|----------|---------|
| Ollama | `anvil --model qwen2.5-coder:32b` (default) |
| LM Studio | `anvil --provider lmstudio --model your-model` |
| OpenAI-compatible | `anvil --provider openai --provider-url http://localhost:1234 --model your-model` |
| API key auth | Set `ANVIL_API_KEY=Bearer sk-...` as environment variable |

---

## Building from Source

Requires Rust 1.85+ ([rustup.rs](https://rustup.rs)):

```bash
git clone https://github.com/Kewton/Anvil.git
cd Anvil
cargo build --release
```

Development commands:

```bash
cargo build                       # Debug build
cargo test                        # Run all tests
cargo clippy --all-targets        # Lint
cargo fmt                         # Format
cargo run -- --model qwen2.5-coder:32b   # Run in debug mode
```

### Contributing

1. Create an issue
2. Create a branch: `feature/<issue>-<description>`
3. Implement → test → ensure `cargo clippy` passes
4. Open a pull request targeting the `develop` branch

See [CLAUDE.md](CLAUDE.md) for full development guidelines.

---

## インストール（日本語）

お使いのOSに合わせてコマンドをターミナルに貼り付けるだけでインストールできます。

| OS | コマンド |
|----|---------|
| macOS (Apple Silicon) | `curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-darwin-arm64.gz \| gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil` |
| macOS (Intel) | `curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-darwin-amd64.gz \| gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil` |
| Linux (x86_64) | `curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-linux-amd64.gz \| gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil` |
| Linux (ARM64) | `curl -sL https://github.com/Kewton/AnvilBinary/releases/latest/download/anvil-linux-arm64.gz \| gunzip -c > /usr/local/bin/anvil && chmod +x /usr/local/bin/anvil` |

インストール後、プロジェクトディレクトリで以下を実行：

```bash
cd /path/to/your/project
anvil --model qwen2.5-coder:32b
```

詳細な設定・使い方は上記英語セクションを参照してください。

---

## License

[MIT](LICENSE)

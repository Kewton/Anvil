# Installation

Anvil is distributed as a single `anvil` binary. It talks only to a local
Ollama server and does not bundle models.

## Supported Platforms

Prebuilt GitHub Release assets are produced for:

| OS | Architecture | Asset |
| --- | --- | --- |
| Linux | x86_64 | `anvil-linux-amd64.gz` |
| Linux | aarch64 / arm64 | `anvil-linux-arm64.gz` |
| macOS | Intel x86_64 | `anvil-darwin-amd64.gz` |
| macOS | Apple Silicon arm64 | `anvil-darwin-arm64.gz` |

Windows is not currently supported as a release target. Build from source may
work on other Unix-like systems, but those paths are not release-tested.

## Install Script

```bash
curl -fsSL 'https://raw.githubusercontent.com/Kewton/Anvil/main/scripts/install.sh' | bash
```

Optional environment variables:

```bash
ANVIL_VERSION=v0.6.0 ANVIL_INSTALL_DIR="$HOME/.local/bin" bash scripts/install.sh
ANVIL_REPO=owner/repo bash scripts/install.sh
```

The script detects OS/architecture, downloads the matching `.gz` release asset
and its `.sha256` file, verifies the SHA256 checksum with `shasum -a 256 -c`,
then installs `anvil` into `$ANVIL_INSTALL_DIR` (default: `$HOME/.local/bin`).

## Manual Install

1. Download the matching `anvil-*.gz` and `anvil-*.gz.sha256` from GitHub
   Releases.
2. Verify the checksum:

   ```bash
   shasum -a 256 -c anvil-linux-amd64.gz.sha256
   ```

3. Unpack and install:

   ```bash
   gzip -dc anvil-linux-amd64.gz > anvil
   chmod +x anvil
   install -m 755 anvil "$HOME/.local/bin/anvil"
   ```

## Build From Source

```bash
cargo build --release
./target/release/anvil --help
```

## Homebrew Plan

Homebrew is not published yet. The intended formula should download the release
asset, verify the SHA256 checksum from the release, install the `anvil` binary,
and declare Ollama as an external runtime prerequisite rather than bundling it.

## Known Limitations

- A local Ollama server is required. Run `ollama serve` and pull at least one
  supported local coding model before using Anvil.
- Bash commands run with the current user privileges. Anvil has safety guards,
  but it is not a complete OS sandbox.
- Live E2E behavior varies by model, quantization, hardware, and local Node /
  Python / Rust toolchains.
- Release assets are checksummed. They are not signed yet.
- SBOM and third-party license bundle generation are planned but not yet part
  of the release workflow.

---
description: "Anvil のリリースPR、タグ、GitHub Releaseを作成する"
---

# /release

`/release` は Anvil のリリースを作成するためのコマンドです。

詳細な手順と安全ルールは `.codex/skills/release/SKILL.md` を正とします。
このファイルは Claude/CommandMate 側から同じハーネス手順を呼び出すための薄い入口です。

## Usage

```text
/release patch
/release minor
/release major
/release 1.2.3
```

## Summary

- `main` を release base にする。
- `release/vX.Y.Z` worktree を作る。
- `Cargo.toml` と `Cargo.lock` を更新する。
- `CHANGELOG.md` と必要に応じて `README.md` を更新する。
- `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test --all`、`cargo build --release` を実行する。
- release PR を `main` 向けに作成する。
- PR が `main` に merge された後で `vX.Y.Z` tag を pushする。
- tag push により `.github/workflows/release.yml` が GitHub Release を作成する。

## Guardrails

- 未コミットのtracked変更があるrelease worktreeでは進めない。
- failing check のままPR/タグ作成を進めない。
- force push、tag削除、強制worktree削除はユーザー明示承認なしで実行しない。

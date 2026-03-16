---
name: release
description: "Create a new release with version bump, CHANGELOG update, Git tag, and GitHub Release. Use when releasing a new version of the project."
disable-model-invocation: true
allowed-tools: "Bash, Read, Edit, Write"
argument-hint: "[version-type] (major|minor|patch) or [version] (e.g., 1.2.3)"
---

# リリーススキル

新しいバージョンをリリースします。リリースブランチ経由でPRを作成し、mainへマージ後にタグ・GitHub Releasesを作成します。

## 使用方法

```bash
/release patch      # パッチバージョンアップ (0.1.0 → 0.1.1)
/release minor      # マイナーバージョンアップ (0.1.0 → 0.2.0)
/release major      # メジャーバージョンアップ (0.1.0 → 1.0.0)
/release 1.0.0      # 直接バージョン指定
```

## ブランチフロー

```
main ← PR ← release/v0.2.0 ← main (リリースブランチ作成)
  ↓
タグ v0.2.0 作成 → GitHub Actions が自動でバイナリビルド・GitHub Release作成
```

CLAUDE.mdの「mainへはPRマージのみ」ルールに準拠しています。

## 実行手順

### 1. 事前チェック

以下を確認してください：

```bash
# 現在のブランチがmainであることを確認
git branch --show-current

# 未コミットの変更がないことを確認
git status

# リモートと同期していることを確認
git fetch origin
git pull origin main
```

**エラーケースの対応:**

| 状況 | 対応 |
|------|------|
| mainブランチでない | `git checkout main` を実行 |
| 未コミットの変更がある | コミットまたはスタッシュを促す |
| リモートと差分がある | `git pull origin main` を促す |

### 2. 現在のバージョン取得

```bash
# Cargo.tomlからバージョンを取得
current_version=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
echo "Current version: $current_version"
```

### 3. 新バージョンの計算

引数に基づいて新バージョンを計算します：

- `patch`: PATCH部分を+1 (例: 0.1.0 → 0.1.1)
- `minor`: MINOR部分を+1、PATCHを0に (例: 0.1.1 → 0.2.0)
- `major`: MAJOR部分を+1、MINOR/PATCHを0に (例: 0.2.0 → 1.0.0)
- 直接指定: 指定されたバージョンをそのまま使用

### 4. タグ存在チェック

```bash
# タグが既に存在しないことを確認
if git rev-parse "v$new_version" >/dev/null 2>&1; then
  echo "Error: Tag v$new_version already exists"
  exit 1
fi
```

### 5. リリースブランチ作成

```bash
git checkout -b "release/v$new_version"
```

### 6. Cargo.toml更新

Editツールを使用して `Cargo.toml` の `version = "x.y.z"` を新バージョンに変更します。

```toml
# 変更前
version = "0.1.0"

# 変更後
version = "0.1.1"
```

### 7. Cargo.lock更新

```bash
cargo generate-lockfile
```

### 8. README.md更新

README.md内のバージョン番号をすべて新バージョンに更新します。

対象箇所:
- ダウンロードURL内のバージョン（例: `/download/v0.1.0/` → `/download/v0.1.1/`）

Editツールで以下のパターンを置換：
```
/releases/download/v{旧バージョン}/ → /releases/download/v{新バージョン}/
```

### 9. ビルド・テスト検証

```bash
# ビルドが通ることを確認
cargo build

# テストが通ることを確認
cargo test

# Clippyが通ることを確認
cargo clippy --all-targets
```

ビルドまたはテストが失敗した場合はリリースを中断します。

### 10. CHANGELOG.md更新

1. CHANGELOG.mdが存在しない場合は新規作成
2. `[Unreleased]`セクションが空でないことを確認
3. `[Unreleased]`の内容を新バージョンセクションとして追加
4. 日付を`YYYY-MM-DD`形式で追記
5. 空の`[Unreleased]`セクションを残す

**注意:** `[Unreleased]`セクションが空の場合は警告を表示し、続行するか確認します。

### 11. コミット作成

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md README.md
git commit -m "chore: release v$new_version"
```

### 12. リリースブランチをプッシュ・PR作成

```bash
git push origin "release/v$new_version"

gh pr create \
  --base main \
  --head "release/v$new_version" \
  --title "chore: release v$new_version" \
  --body "## Release v$new_version

### 変更内容
（CHANGELOG.mdの該当バージョンセクションの内容を記載）

### チェックリスト
- [ ] Cargo.toml バージョン更新済み
- [ ] Cargo.lock 更新済み
- [ ] README.md バージョン更新済み
- [ ] CHANGELOG.md 更新済み
- [ ] cargo build 成功
- [ ] cargo test 全パス
- [ ] cargo clippy 警告ゼロ
"
```

### 13. PRマージ後: タグ作成・プッシュ

**PRがマージされた後に以下を実行します：**

```bash
# mainに切り替え
git checkout main
git pull origin main

# タグ作成・プッシュ
git tag "v$new_version"
git push origin "v$new_version"
```

タグのプッシュにより GitHub Actions（`.github/workflows/release.yml`）が自動で以下を実行します：
- 4プラットフォーム向けバイナリビルド（linux-amd64, linux-arm64, darwin-amd64, darwin-arm64）
- GitHub Release作成とバイナリアップロード

### 14. リリースブランチの削除

```bash
git branch -d "release/v$new_version"
git push origin --delete "release/v$new_version"
```

### 15. developへの反映

```bash
git checkout develop
git pull origin develop
git merge main
git push origin develop
```

## 完了確認

リリース完了後、以下を確認します：

```bash
# タグ一覧
git tag -l

# 最新タグ
git describe --tags --abbrev=0

# GitHub Releases（バイナリがアップロードされていることを確認）
gh release view "v$new_version"

# GitHub Actions のビルド状況
gh run list --limit 3
```

## エラーハンドリング

| エラーケース | 対応 |
|-------------|------|
| 未コミットの変更がある | エラー表示し、コミットまたはスタッシュを促す |
| リモートとの差分がある | `git pull`を促す |
| タグが既に存在する | エラー表示し、別バージョンの指定を促す |
| CHANGELOG.mdが存在しない | 新規作成するか確認 |
| [Unreleased]セクションが空 | 警告を表示し、続行するか確認 |
| `cargo build` が失敗 | エラー修正を促し、リリースを中断 |
| `cargo test` が失敗 | テスト修正を促し、リリースを中断 |
| `cargo clippy` に警告がある | 警告修正を促し、リリースを中断 |
| PRのCIが失敗 | CI修正後にリリースブランチに追加コミット |
| GitHub Actionsのビルド失敗 | ワークフロー修正後にタグを削除して再作成 |

## 参考

- [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/)
- [Semantic Versioning](https://semver.org/lang/ja/)
- [GitHub Actions Release Workflow](../../.github/workflows/release.yml)

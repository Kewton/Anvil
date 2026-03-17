---
model: opus
description: "Issueの受入テストをdevelopブランチ上でAnvilを実際に起動して実施し、HTMLレポートを生成"
---

# ユーザー受入テスト（UAT）

## 概要
developブランチでIssueの受入テストを実施します。Anvilを実際に起動して動作確認を行い、結果をHTMLレポートとして出力します。

**重要**: コンテキストを綺麗に保つため、テスト実行はサブエージェントで行います。

## 使用方法
```
/uat [Issue番号]
```

## 実行手順

### 1. 事前チェック

```bash
# developブランチであることを確認
current_branch=$(git branch --show-current)
if [ "$current_branch" != "develop" ]; then
  echo "ERROR: developブランチで実行してください（現在: $current_branch）"
  exit 1
fi

# 未コミットの変更がないことを確認
git status --porcelain
```

### 2. Issue情報の取得

```bash
gh issue view $ARGUMENTS --json title,body,labels
```

Issue本文から以下を抽出：
- **受け入れ基準**（「受け入れ基準」「Acceptance Criteria」セクション）
- **提案する解決策**（期待される機能の概要）
- **関連するファイル・モジュール**

### 3. テスト項目の決定

Issueに受け入れ基準が明記されている場合はそれに従う。

明記されていない場合は、Issue本文からテスト項目を推定し、**ユーザーに確認**する：

```
以下のテスト項目で受入テストを実施します。追加・修正はありますか？

  AT-1: [テスト項目1の説明]
  AT-2: [テスト項目2の説明]
  AT-3: [テスト項目3の説明]
  ...

(y: 続行 / 追加・修正があれば入力してください)
```

不明確なテスト項目がある場合は、追加で質問してから進める。

### 4. 作業環境の準備

```bash
ISSUE_NUM=$ARGUMENTS
SANDBOX_DIR="./sandbox/${ISSUE_NUM}"
mkdir -p "$SANDBOX_DIR"
```

### 5. サブエージェントによるテスト実行

**各テスト項目をサブエージェント（general-purpose）で実行します。**

サブエージェントへの指示テンプレート：

```
Issue #${ISSUE_NUM} の受入テスト AT-${N} を実施してください。

## テスト内容
${テスト項目の説明}

## テスト手順
1. テストに必要な前提条件をセットアップ
2. Anvilバイナリを実際にビルド・起動して動作確認
   - cargo build --release
   - echo "${テスト入力}" | ./target/release/anvil --model qwen3.5:35b --no-approval --oneshot
   または対話的なテストが必要な場合は手順を記述
3. 出力結果を期待値と照合
4. テスト結果（PASS/FAIL）と詳細を報告

## 作業ディレクトリ
./sandbox/${ISSUE_NUM}/AT-${N}/

## 出力形式
以下をJSON形式で報告：
{
  "test_id": "AT-${N}",
  "title": "テスト項目名",
  "status": "PASS" or "FAIL",
  "description": "テスト内容の説明",
  "steps": ["手順1", "手順2", ...],
  "expected": "期待結果",
  "actual": "実際の結果",
  "evidence": "根拠となるコマンド出力やログ",
  "screenshot_path": null,
  "notes": "補足事項"
}
```

### 6. スクリーンショット取得（可能な場合）

macOS環境の場合、TUI出力をキャプチャ：

```bash
# Anvilの出力をファイルに保存してエビデンスとする
echo "${テスト入力}" | ./target/release/anvil --model qwen3.5:35b --no-approval --oneshot 2>"$SANDBOX_DIR/AT-${N}/stderr.log" >"$SANDBOX_DIR/AT-${N}/stdout.log"

# macOSの場合、screencaptureでターミナルのスクリーンショットを取得可能
# screencapture -x "$SANDBOX_DIR/AT-${N}/screenshot.png"
```

スクリーンショット取得が難しい場合は、コマンド出力ログをエビデンスとして代用する。

### 7. HTMLレポート生成

全テスト結果を集約し、HTMLレポートを `$SANDBOX_DIR/report.html` に生成する。

#### HTMLレポートの構成

```html
<!DOCTYPE html>
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <title>受入テストレポート - Issue #${ISSUE_NUM}</title>
</head>
<body>
  <!-- ヘッダー: Issue情報 -->
  <!-- サマリー: PASS/FAIL数、合格率 -->
  <!-- テスト結果テーブル: 各テスト項目の詳細 -->
  <!-- エビデンス: コマンド出力やスクリーンショット -->
  <!-- フッター: 実行環境情報 -->
</body>
</html>
```

#### HTMLデザイン要件

- **モダンなデザイン**: ダークテーマベース、カード型レイアウト
- **色分け**: PASS=緑（#22c55e）、FAIL=赤（#ef4444）、SKIP=グレー
- **レスポンシブ**: ブラウザで見やすいレイアウト
- **情報量**: テスト手順、期待値、実際の結果、エビデンスを各テストに表示
- **サマリーセクション**: 円グラフ風の合格率表示、全テスト数/PASS数/FAIL数
- **エビデンス折りたたみ**: 長いログ出力は `<details>` タグで折りたたみ
- **実行環境情報**: 日時、ブランチ、コミットハッシュ、Rustバージョン、OS情報

#### HTMLテンプレートの主要セクション

1. **ヘッダー**
   - Issue番号・タイトル
   - テスト実施日時
   - ブランチ名・コミットハッシュ

2. **サマリーダッシュボード**
   - 合格率（パーセンテージ + 視覚的バー）
   - PASS / FAIL / SKIP の件数
   - 全体の判定（ALL PASS → ACCEPTED / それ以外 → REJECTED）

3. **テスト結果一覧**
   - 各テスト項目をカード形式で表示
   - テストID、タイトル、ステータスバッジ
   - テスト手順（番号付きリスト）
   - 期待結果 vs 実際の結果
   - エビデンス（`<details>` で折りたたみ）

4. **フッター**
   - 実行環境: OS, Rustバージョン, Anvilバージョン
   - 生成ツール: Claude Code

### 8. 結果報告

HTMLレポートのパスと結果サマリーをユーザーに報告：

```
受入テスト完了: Issue #${ISSUE_NUM}

  結果: X/Y PASS （合格率 XX%）
  判定: ACCEPTED / REJECTED

  レポート: ./sandbox/${ISSUE_NUM}/report.html
```

## 完了条件

- [ ] 全テスト項目が実行されている
- [ ] HTMLレポートが `./sandbox/${ISSUE_NUM}/report.html` に生成されている
- [ ] レポートがブラウザで正しく表示される
- [ ] 各テストにエビデンス（ログ出力）が含まれている
- [ ] 結果サマリーがユーザーに報告されている

## エラーハンドリング

| エラーケース | 対応 |
|-------------|------|
| developブランチでない | エラー表示し中断 |
| `cargo build` 失敗 | エラー表示し中断（テスト不可） |
| Issue番号が無効 | エラー表示し中断 |
| Issueに受入基準がない | テスト項目を推定しユーザーに確認 |
| Anvilの起動に失敗 | エラーログを記録しFAILとして報告 |
| テスト途中でエラー | 該当テストをFAILとし、残りのテストを続行 |

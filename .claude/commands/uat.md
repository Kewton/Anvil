---
model: opus
description: "Issueの受入テストをdevelopブランチ上でAnvilを実際に起動して実施し、HTMLレポートを生成"
---

# ユーザー受入テスト（UAT）

## 概要
developブランチでIssueの受入テストを実施します。Anvilを実際に起動して動作確認を行い、結果をHTMLレポートとして出力します。複数Issueの一括テストに対応しています。

**重要**: コンテキストを綺麗に保つため、テスト実行はサブエージェントで行います。

## 使用方法
```
/uat [Issue番号]              # 単一Issue
/uat [Issue番号1] [Issue番号2] ...  # 複数Issue（スペース区切り）
```

例：
```
/uat 8
/uat 8 9 10
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

### 2. 引数の解析

`$ARGUMENTS` をスペースで分割し、Issue番号のリストを生成する。

```
入力例: "8 9 10"
→ ISSUE_LIST = [8, 9, 10]
```

各Issue番号に対して `gh issue view` で存在を確認する。無効な番号があれば警告を表示し、有効なIssueのみ続行する。

```bash
for issue_num in $ARGUMENTS; do
  gh issue view "$issue_num" --repo Kewton/Anvil --json title -q '.title' 2>/dev/null
done
```

### 3. Issue情報の取得（Issueごと）

各Issueについて以下を取得：

```bash
gh issue view $issue_num --json title,body,labels
```

Issue本文から以下を抽出：
- **受け入れ基準**（「受け入れ基準」「Acceptance Criteria」セクション）
- **提案する解決策**（期待される機能の概要）
- **関連するファイル・モジュール**

### 4. テスト項目の決定（全Issueまとめて確認）

全Issueのテスト項目を一覧化し、**ユーザーにまとめて確認**する：

```
以下のテスト項目で受入テストを実施します。追加・修正はありますか？

Issue #8: システムプロンプトのshell.execガイド強化とコマンド権限制御
  AT-8-1: [テスト項目の説明]
  AT-8-2: [テスト項目の説明]

Issue #9: file.editツール追加（部分編集対応）
  AT-9-1: [テスト項目の説明]
  AT-9-2: [テスト項目の説明]

Issue #10: ANVIL.mdプロジェクト指示ファイル対応
  AT-10-1: [テスト項目の説明]

(y: 続行 / 追加・修正があれば入力してください)
```

Issueに受け入れ基準が明記されている場合はそれに従う。明記されていない場合はIssue本文から推定する。不明確なテスト項目がある場合は、追加で質問してから進める。

### 5. 作業環境の準備

```bash
for issue_num in $ISSUE_LIST; do
  mkdir -p "./sandbox/${issue_num}"
done
```

### 6. サブエージェントによるテスト実行

**各Issueのテスト項目をサブエージェント（general-purpose）で実行します。**

独立したIssueのテストは並列にサブエージェントを起動して効率化する。

サブエージェントへの指示テンプレート：

```
Issue #${ISSUE_NUM} の受入テスト AT-${ISSUE_NUM}-${N} を実施してください。

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
  "test_id": "AT-${ISSUE_NUM}-${N}",
  "issue_number": ${ISSUE_NUM},
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

### 7. スクリーンショット取得（可能な場合）

macOS環境の場合、TUI出力をキャプチャ：

```bash
# Anvilの出力をファイルに保存してエビデンスとする
echo "${テスト入力}" | ./target/release/anvil --model qwen3.5:35b --no-approval --oneshot 2>"$SANDBOX_DIR/AT-${N}/stderr.log" >"$SANDBOX_DIR/AT-${N}/stdout.log"

# macOSの場合、screencaptureでターミナルのスクリーンショットを取得可能
# screencapture -x "$SANDBOX_DIR/AT-${N}/screenshot.png"
```

スクリーンショット取得が難しい場合は、コマンド出力ログをエビデンスとして代用する。

### 8. HTMLレポート生成

#### Issue単位のレポート

各Issueのテスト結果を `./sandbox/${ISSUE_NUM}/report.html` に生成する。

#### 全体サマリーレポート（複数Issue時）

複数Issueが指定された場合、全Issueの結果を集約した `./sandbox/uat-summary.html` を追加で生成する。

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

#### 全体サマリーHTMLの構成（複数Issue時）

```html
<!DOCTYPE html>
<html lang="ja">
<head>
  <meta charset="UTF-8">
  <title>UAT全体サマリー</title>
</head>
<body>
  <!-- ヘッダー: テスト実施日時、対象Issue一覧 -->
  <!-- 全体サマリー: 全Issue合算のPASS/FAIL数、合格率 -->
  <!-- Issue別サマリーテーブル -->
  <!--   Issue番号 | タイトル | テスト数 | PASS | FAIL | 判定 | レポートリンク -->
  <!-- フッター: 実行環境情報 -->
</body>
</html>
```

#### HTMLデザイン要件

- **モダンなデザイン**: ダークテーマベース、カード型レイアウト
- **色分け**: PASS=緑（#22c55e）、FAIL=赤（#ef4444）、SKIP=グレー
- **レスポンシブ**: ブラウザで見やすいレイアウト
- **情報量**: テスト手順、期待値、実際の結果、エビデンスを各テストに表示
- **サマリーセクション**: 合格率表示（パーセンテージ + 視覚的バー）、全テスト数/PASS数/FAIL数
- **エビデンス折りたたみ**: 長いログ出力は `<details>` タグで折りたたみ
- **実行環境情報**: 日時、ブランチ、コミットハッシュ、Rustバージョン、OS情報
- **Issue別レポートへのリンク**: 全体サマリーから各Issueのレポートへリンク

#### HTMLテンプレートの主要セクション（Issue単位レポート）

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

### 9. 結果報告

全Issue の結果サマリーをユーザーに報告：

```
受入テスト完了

  Issue #8:  5/5 PASS (100%) → ACCEPTED
  Issue #9:  3/4 PASS ( 75%) → REJECTED
  Issue #10: 2/2 PASS (100%) → ACCEPTED

  全体: 10/11 PASS (91%)

  レポート:
    ./sandbox/8/report.html
    ./sandbox/9/report.html
    ./sandbox/10/report.html
    ./sandbox/uat-summary.html  (全体サマリー)
```

単一Issueの場合は全体サマリーは生成せず、Issue単体のレポートのみ出力する。

## 完了条件

- [ ] 全Issueの全テスト項目が実行されている
- [ ] 各IssueのHTMLレポートが `./sandbox/${ISSUE_NUM}/report.html` に生成されている
- [ ] 複数Issue時は全体サマリーが `./sandbox/uat-summary.html` に生成されている
- [ ] レポートがブラウザで正しく表示される
- [ ] 各テストにエビデンス（ログ出力）が含まれている
- [ ] 結果サマリーがユーザーに報告されている

## エラーハンドリング

| エラーケース | 対応 |
|-------------|------|
| developブランチでない | エラー表示し中断 |
| `cargo build` 失敗 | エラー表示し中断（テスト不可） |
| Issue番号が無効 | 該当Issueをスキップし、有効なIssueのみ続行 |
| Issueに受入基準がない | テスト項目を推定しユーザーに確認 |
| Anvilの起動に失敗 | エラーログを記録しFAILとして報告 |
| テスト途中でエラー | 該当テストをFAILとし、残りのテストを続行 |
| 全Issue番号が無効 | エラー表示し中断 |

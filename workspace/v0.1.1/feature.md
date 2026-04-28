  修正後のマッピング

  ┌─────────────┬────────────────────────────────────┐
  │    Epic     │    含まれる Issue (GitHub 番号)    │
  ├─────────────┼────────────────────────────────────┤
  │ #444 Epic A │ #450, #451, #452, #453, #454, #455 │
  ├─────────────┼────────────────────────────────────┤
  │ #445 Epic B │ #456, #457, #458, #459, #460, #461 │
  ├─────────────┼────────────────────────────────────┤
  │ #446 Epic C │ #462, #463, #464                   │
  ├─────────────┼────────────────────────────────────┤
  │ #447 Epic D │ #465, #466, #467                   │
  ├─────────────┼────────────────────────────────────┤
  │ #448 Epic E │ #468, #469, #470                   │
  ├─────────────┼────────────────────────────────────┤
  │ #449 Epic F │ #471, #472, #473                   │
  └─────────────┴────────────────────────────────────┘

  各行末尾に括弧付きで論理番号（#1〜#24）も併記しています。

---
以下では、Anvil のエンハンスを Epic Issue と Feature Issue に分けて、GitHub Issue としてそのまま使いやすい形に整理します。

前提として、Anvil の表面 UX は引き続き Plan / Act のまま維持します。
今回の設計は、Anvil を「複数エージェントが自由会話するプロダクト」にするのではなく、内部 runtime に Feedback / Precaution / Verification / Memory / Skill / Repo Context を段階的に追加するものです。

⸻

0. 並列開発のための全体設計

0.1 Epic 構成

Epic	名前	主な目的	含まれる Issue
Epic A	Dynamic Precaution Runtime	失敗から注意事項を作り、次の生成を制約する	#1〜#6
Epic B	Verification & Temporary Testing	テスト・ビルド・repo 差分で進捗を検証する	#7〜#11, #23
Epic C	Case Memory & CBR	成功・失敗パターンを保存して再利用する	#12〜#14
Epic D	Agentic Skills Layer	Reminder / Verifier / Tester を内部 skill として整理する	#15〜#17
Epic E	Repo Graph & Domain Context	repo 構造を使って探索・編集対象を絞る	#18〜#20
Epic F	Observability / Evaluation / Training Export	評価ログ・A/B 評価・学習データ出力を整える	#21〜#22, #24

⸻

0.2 並列開発のために先に固定する共有インターフェース

並列開発する場合、以下の型・境界だけ先に合意しておくと衝突が減ります。

FeedbackFrame

実行結果・失敗・検証結果を統一的に表す。

関係する Issue:

* #1
* #3
* #6
* #7
* #8
* #10
* #14
* #21
* #24

Precaution

次の生成・編集・検証で守るべき注意事項を表す。

関係する Issue:

* #2
* #3
* #4
* #5
* #6
* #12
* #13
* #14
* #24

AnvilScore

1 turn / 1 actor loop の検証可能な進捗を表す。

関係する Issue:

* #7
* #8
* #12
* #21
* #22
* #24

TemporaryTestStore

生成テストを repo へ直接書かず、一時保存・実行・promote / discard する。

関係する Issue:

* #9
* #10
* #11
* #23

AgentSkill

Reminder / Verifier / Tester / CaseRecall などの内部補助処理を統一的に呼ぶ。

関係する Issue:

* #15
* #16
* #17

RepoGraph

repo 内の file / import / symbol / test / instruction 関係を軽量に持つ。

関係する Issue:

* #18
* #19
* #20

⸻

0.3 推奨される並列開発順

最初に #1 FeedbackFrame と #2 active_precautions を薄く入れると、他のチームが mock / stub で進めやすくなります。

Track 1: Runtime / Precaution
  #1 -> #2 -> #3 -> #4 -> #5 -> #6
Track 2: Verification / Safety
  #7 -> #8
  #9 -> #10 -> #11
  #23 は早期に独立着手可能
Track 3: Memory
  #12 -> #13 -> #14
  ただし #12 は #1, #2, #7 の安定後が望ましい
Track 4: Skills Architecture
  #15 は早期設計可能
  #16 は #3, #7, #8 後
  #17 は #15 後
Track 5: Repo Context
  #18 -> #19 -> #20
  他 Track と比較的独立
Track 6: Observability / Eval
  #21 は #1, #2, #7 の stub だけで着手可能
  #22 は #7 後
  #24 は #3, #12, #21 後

⸻

Epic A: Dynamic Precaution Runtime

Epic A 概要

Anvil が実行失敗や検証失敗を観測したとき、それを単なる再試行に使うのではなく、次の生成で守るべき precaution に変換する。
これにより、小規模ローカル LLM が同じ失敗を繰り返すことを減らす。

背景

現在の Anvil は、no-tool response、bash loop、focused edit failure、auto test failure などに対して runtime recovery を持っている。
しかし、それらの失敗はまだ統一的な意味構造になっておらず、次の生成に対する明示的な制約として保存・注入されていない。

課題

* エラーや失敗が文字列ログとして散らばっている
* 同じ compile error / edit failure / bash loop を繰り返しやすい
* sidecar model を使った補助判断の入出力形式が未定義
* ユーザーが「今 Anvil が何を注意しているか」を確認できない

提案する解決策

* 実行結果を FeedbackFrame に正規化する
* WorkingMemory に active_precautions を追加する
* Reminder Sidecar が FeedbackFrame から precaution を生成する
* Act mode prompt に ACTIVE PRECAUTIONS を注入する
* /precautions コマンドで確認・追加・retire できるようにする

Epic A 受け入れ条件

* 実行失敗が FeedbackFrame として保存される
* 失敗後に Reminder が precaution を生成できる
* 生成された precaution が session に保存される
* 次の Act mode request に ACTIVE PRECAUTIONS が入る
* resume 後も active precautions が復元される
* malformed Reminder output で runtime が落ちない

⸻

Issue #1: FeedbackFrame を導入する

概要

Bash、auto test、tool failure、edit failure、no-progress などの runtime feedback を、共通の FeedbackFrame として扱えるようにする。

背景

現在の Anvil は実行結果を複数の場所で扱っているが、共通の意味構造がない。
Reminder、Verifier、Case Memory、Evaluation Log などの後続機能は、実行結果を構造化して参照する必要がある。

課題

* stderr / stdout / test failure / edit failure が統一的に扱われていない
* 失敗種別の分類が後続処理で再利用しづらい
* session resume 後に直近の失敗概要を扱いにくい
* log には残っていても runtime policy として使いにくい

提案する解決策

FeedbackFrame と FeedbackKind を導入し、実行結果を共通形式に正規化する。

仕様

想定構造:

pub struct FeedbackFrame {
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub kind: FeedbackKind,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub primary_error: Option<String>,
    pub suspected_files: Vec<PathBuf>,
    pub changed_files: Vec<PathBuf>,
    pub suggested_focus: Option<String>,
}
pub enum FeedbackKind {
    BuildPass,
    TestPass,
    CompileError,
    TestFailure,
    TypeError,
    LintFailure,
    Timeout,
    ToolProtocolFailure,
    EditFailure,
    NoRepoProgress,
    UnsafeCommandBlocked,
    NoVerifierAvailable,
    UnknownFailure,
}

対象入力:

* Bash 実行結果
* auto_test 実行結果
* parser failure
* native tool failure
* XML fallback failure
* focused edit failure
* no repo progress
* unsafe command blocked

実装上の制約:

* stdout / stderr は excerpt 化する
* secret-looking な値は mask する
* parse 不能なものは UnknownFailure に落とす
* runtime を panic させない

受け入れ条件

* cargo test が compile error で失敗した場合、FeedbackKind::CompileError が生成される
* pytest assertion failure が FeedbackKind::TestFailure として扱われる
* bash timeout が FeedbackKind::Timeout になる
* tool call parse failure が FeedbackKind::ToolProtocolFailure になる
* unsafe command block が FeedbackKind::UnsafeCommandBlocked になる
* stdout / stderr excerpt が上限文字数を超えない
* FeedbackFrame が session log に保存される
* session resume 後に直近の FeedbackFrame を参照できる

⸻

Issue #2: WorkingMemory に active_precautions を追加する

概要

WorkingMemory に、次の生成・編集・検証で守るべき注意事項を保存する active_precautions を追加する。

背景

Anvil の working_memory は active task、constraints、touched files、unresolved errors を保持している。
しかし、実行失敗から得られた「次に守るべき具体的注意事項」は、まだ明示的な state として保持されていない。

課題

* 同じ失敗を繰り返す
* error recovery が一時的な note にとどまる
* prompt に入れるべき注意事項を管理できない
* ユーザーが注意事項を確認・修正できない

提案する解決策

Precaution 型を導入し、WorkingMemory.active_precautions として session に保存する。

仕様

想定構造:

pub struct Precaution {
    pub id: String,                    // SHA-256 full hex (64 chars) over text + source + canonical applies_to
    pub source: PrecautionSource,
    pub severity: Severity,            // High / Medium (default) / Low
    pub text: String,                  // canonicalized: raw cap (8 KiB) -> mask_secrets -> truncate (240 chars)
    pub applies_to: Vec<PathBuf>,      // canonical: workspace-relative, sorted/deduped by "/"-joined UTF-8 key
    pub status: PrecautionStatus,
    pub retired_reason: Option<RetiredReason>,
}
pub enum PrecautionSource {
    UserRequirement,
    PlanConstraint,
    BuildFailure,
    TestFailure,
    ToolFailure,
    NoProgress,
    SafetyPolicy,
    Manual,                            // Default
    Unknown,                           // serde(other) — forward-compat only
}
pub enum PrecautionStatus {
    Active,                            // Default
    Resolved,
    Retired,
    Unknown,                           // serde(other) — sanitized to Retired(Unknown) on load
}
pub enum Severity {
    High,
    Medium,                            // Default
    Low,
}
pub enum RetiredReason {
    UserRetired,                       // Default — terminal, blocks re-Activation of same key
    CapacityEvicted,                   // FIFO push-out, allows re-Activation of same key
    Unknown,                           // serde(other) — forward-compat only
}
pub enum AddPrecautionOutcome {
    Added,
    DuplicateIgnored,
    Truncated,                         // text exceeded MAX_PRECAUTION_TEXT, was truncated and stored
}

挙動:

* Active の precaution だけが通常 prompt に注入される
* Resolved / Retired は履歴として残せる（MAX_PRECAUTIONS_TOTAL = 64 まで bounded）
* 同一内容の precaution は重複登録しない（id ベース、status × retired_reason matrix で判定）
* 1 session あたりの Active 上限は 16（MAX_ACTIVE_PRECAUTIONS）。超過時は最古 Active を Retired(CapacityEvicted) に FIFO 押し出し
* 1 件あたりの text は 240 chars に truncate（MAX_PRECAUTION_TEXT、`...` 込みで合計 243 chars）
* mask_secrets 適用前の raw text は 8 KiB（MAX_PRECAUTION_RAW_TEXT）で先に切り詰める（regex DoS 防御）
* CapacityEvicted で押し出された key は再追加で Active 化される（同じ失敗の再発時に再注入）
* 正規入口は WorkingMemory::add_precaution。bypass を防ぐために load 後は sanitize_active_precautions_after_load を呼ぶ
* canonicalization 順序: raw cap (8 KiB) → mask_secrets → truncate (240 chars) → applies_to canonicalize → SHA-256 id
* applies_to は `..` (ParentDir) を入口で reject、normalize_path_to_workspace で workspace 外を drop、非 UTF-8 component で path 全体を drop（部分削除しない）
* serde は snake_case rename。未知 source/status/retired_reason は Unknown variant に fallback
* Unknown status は load 後 sanitize で Retired(Unknown) に落とし、Active として prompt 注入されない

受け入れ条件

* session.json に active_precautions が保存される
* session resume 後に active precautions が復元される
* 同一 text / source / applies_to の precaution が重複登録されない
* Retired の precaution は prompt に注入されない
* manual precaution を追加できる内部 API（add_precaution / resolve_precaution / retire_precaution）がある
* compaction 後も active precautions が消えない
* 旧 session.json (active_precautions フィールドなし) が空 vec として load される
* 改ざんされた session.json (raw secret / 不正 path / 偽 id / 未知 status) を load しても sanitizer で安全側に正規化される

⸻

Issue #3: Reminder Sidecar を実装する

概要

最新の FeedbackFrame と現在の task state から、次回の生成で守るべき precautions を生成する Reminder Sidecar を実装する。

背景

小規模 LLM は、コード生成、失敗分析、次の方針決定を同時に行うと崩れやすい。
Reminder はコードを書かず、失敗から「次に守るべき制約」だけを作ることで、Programmer の負荷を下げる。

課題

* 失敗ログをそのまま main model に渡しても修正方針が散らばる
* 小規模 LLM は自己反省を長く続けると目的を忘れやすい
* runtime recovery note が次 iteration の明確な制約になっていない

提案する解決策

sidecar model を使って、FeedbackFrame から最大 5 件の actionable precautions を生成する。

仕様

入力:

* user task
* current mode
* current plan summary
* active precautions
* latest FeedbackFrame
* touched files
* unresolved errors
* current AnvilScore があればそれも含める

出力:

{
  "precautions": [
    {
      "source": "BuildFailure",
      "severity": "High",
      "text": "When modifying SessionSnapshot, update serialization and resume restoration consistently.",
      "applies_to": ["src/session/store.rs"]
    }
  ]
}

制約:

* Reminder はコードを書かない
* diff を返さない
* tool call をしない
* JSON parse failure 時は no-op または fallback parser
* sidecar model がなければ main model で fallback
* Reminder failure で actor loop を止めない

受け入れ条件

* auto test 失敗後に Reminder が呼ばれる
* Reminder 出力が active_precautions に反映される
* Reminder は code diff や修正コードを返さない
* malformed JSON が返っても runtime が落ちない
* 同一 FeedbackFrame から同じ precaution が過剰に増えない
* Reminder 呼び出し結果が structured log に残る

⸻

Issue #4: Act mode prompt に ACTIVE PRECAUTIONS を注入する

概要

Act mode のモデル入力に、現在有効な precautions を明示的な制約リストとして注入する。

背景

precaution は保存するだけでは効果がない。
次のコード生成・編集時に、LLM が参照できる短い制約として prompt に入れる必要がある。

課題

* active precautions が prompt に反映されないと実行時に意味がない
* すべての precaution を入れると token budget を圧迫する
* Plan mode に write 誘導を入れると mode 制約と矛盾する

提案する解決策

Act mode の runtime context に ACTIVE PRECAUTIONS ブロックを追加する。

仕様

例:

ACTIVE PRECAUTIONS:
- Preserve path confinement behavior in src/safety/path_guard.rs.
- Previous cargo test failed due to a missing SessionSnapshot field.
- Make a focused edit and rerun the relevant verification command.

選択ルール:

* status = Active のみ対象
* severity が高いものを優先
* current touched files / suspected files に関連するものを優先
* 最大件数を持つ
* token budget に応じて低優先度を省略
* Plan mode では原則非表示、または plan constraints として別扱い

受け入れ条件

* Act mode の LLM request に ACTIVE PRECAUTIONS が含まれる
* active precaution がない場合はブロックを出さない
* Retired / Resolved の precaution は出ない
* severity high の precaution が優先される
* token budget 超過時に低優先度 precaution が省略される
* LLM I/O log で注入内容を確認できる

⸻

Issue #5: /precautions コマンドを追加する

概要

REPL から active precautions を確認・追加・retire・clear できる slash command を追加する。

背景

Anvil は TUI / terminal UX を重視している。
内部 state として precaution が増えると、ユーザーが「なぜ今この制約で動いているのか」を確認できる必要がある。

課題

* 誤った precaution が prompt に入り続ける可能性がある
* ユーザーが手動で注意事項を追加できない
* resume 後の内部状態が不透明になる

提案する解決策

/precautions コマンド群を追加する。

仕様

想定操作:

/precautions
/precautions add "Do not modify generated files under target/"
/precautions retire <id>
/precautions clear

挙動:

* /precautions は active precautions を一覧表示する
* add は source = Manual として追加する
* retire は status を Retired にする
* clear は active precautions をまとめて retire または削除する
* clear は confirmation または yes mode に従う

受け入れ条件

* /precautions で active precautions が表示される
* /precautions add した内容が session に保存される
* 手動追加した precaution が次の Act prompt に入る
* /precautions retire <id> 後、その precaution は prompt に入らない
* resume 後も manual precaution が残る
* 不正 ID 指定で runtime が落ちない

⸻

Issue #6: runtime recovery を Precaution 更新に接続する

概要

既存の recovery event を FeedbackFrame 化し、Reminder に渡して precaution 更新へ接続する。

背景

Anvil は no-tool response、focused edit failure、bash loop、deterministic fallback などの recovery を既に持っている。
これらを単なる再試行にせず、「次に同じ失敗を避けるための制約」に変換する。

課題

* recovery note が一時的で、次 iteration の制約として残らない
* 同じ bash command や edit failure を繰り返す
* no-progress が「なぜ進んでいないか」として構造化されていない

提案する解決策

以下の event を FeedbackFrame に変換し、Reminder に渡す。

* no repo progress
* repeated bash command
* focused edit failure
* no tool call despite action expectation
* unsafe command blocked
* parser failure
* deterministic fallback triggered

仕様

* 同一 event で sidecar を何度も呼ばない
* recovery loop と Reminder loop が無限循環しない
* generated precaution は既存 recovery note と矛盾しない
* Reminder 失敗時は既存 recovery を継続する

受け入れ条件

* focused edit failure 後に read-before-edit 系 precaution が生成される
* repeated bash command 後に同じ command の反復を避ける precaution が生成される
* unsafe command block 後に safety policy 系 precaution が生成される
* no repo progress 後に探索または編集方針を狭める precaution が生成される
* Reminder が失敗しても既存 recovery が継続する

⸻

Epic B: Verification & Temporary Testing

Epic B 概要

Anvil が生成・編集した結果を、テスト、ビルド、repo 差分、sandbox policy によって検証可能にする。

背景

現行 Anvil は auto_test、repo snapshot、quality gate を持っている。
しかし、それらはまだ統合的な score や temporary test workflow として整理されていない。

課題

* 進捗評価が複数箇所に散っている
* test がない repo では実行フィードバックが弱い
* generated test を repo に直接書くのは危険
* 生成コード・生成テストの実行安全性をさらに強化する必要がある

提案する解決策

* AnvilScore を導入する
* auto_test を FeedbackFrame / AnvilScore に接続する
* temporary test workspace を作る
* Tester Skill v1 で smoke test を生成する
* generated test の promote / discard フローを作る
* sandbox policy を強化する

Epic B 受け入れ条件

* test/build 結果が AnvilScore に反映される
* test がない repo で temporary smoke test を生成できる
* generated test は無承認で repo に保存されない
* generated test 実行には timeout と safety policy が適用される
* unsafe command は FeedbackFrame として記録される

⸻

Issue #7: AnvilScore を導入する

概要

1 turn または 1 actor loop における検証可能な進捗を AnvilScore として表す。

背景

Anvil は repo snapshot、auto test、quality gate を持つが、それらを統一して「今回の実行は前進したか」を評価する構造がない。

課題

* build/test/repo 差分/unsafe action が別々に扱われる
* Reminder や Case Memory が進捗を参照しにくい
* 将来の A/B 評価や fine-tuning dataset export に使う指標がない

提案する解決策

AnvilScore を導入し、検証可能な状態を deterministic に計算する。

仕様

想定構造 (Issue #456 確定版、12 field / 5 lifecycle group):

pub struct AnvilScore {
    // Group 1: outcome (turn-local)
    pub build_passed: Option<bool>,
    pub tests_passed: Option<bool>,

    // Group 2: delta (turn-local, 直前 turn 比)
    pub compile_errors_delta: Option<i32>,
    pub test_failures_delta: Option<i32>,

    // Group 3: next-turn baseline (turn-local 絶対値、#456 では fixture のみ、本番経路は #457)
    pub compile_error_count: Option<usize>,
    pub test_failure_count: Option<usize>,

    // Group 4: repo diff (turn-local、RepoVerification 由来)
    pub implementation_files_changed: Option<usize>,
    pub test_files_changed: Option<usize>,
    pub setup_files_changed: Option<usize>,

    // Group 5: counter / accumulator / flag
    pub unsafe_actions_blocked: usize,           // turn-local
    pub consecutive_no_progress_turns: usize,    // session-cumulative の denormalize copy
    pub user_visible_artifact: bool,             // turn-local
}

制約:

* LLM judge に依存しない
* 未実行項目は None
* false と unknown を混同しない
* repo snapshot と auto test 結果から計算する

受け入れ条件

* test pass 時に tests_passed = Some(true) になる
* test failure 時に tests_passed = Some(false) になる
* test 未実行時に tests_passed = None になる
* implementation file 変更数が repo snapshot から計算される
* unsafe command block 数が反映される
* session log に AnvilScore が記録される

⸻

Issue #8: auto_test を FeedbackFrame / AnvilScore に接続する

概要

auto_test の実行結果を FeedbackFrame と AnvilScore に変換する。

背景

auto_test は Anvil の検証可能性の中心になる。
単に command を実行するだけでなく、結果を分類して後続の Reminder / Score / Case Memory が使えるようにする必要がある。

課題

* compile error の file:line が構造化されていない
* pytest / cargo / npm の失敗分類が統一されていない
* verifier がない場合の扱いが曖昧
* auto test 結果が score と直結していない

提案する解決策

言語別に auto test 結果を分類し、FeedbackFrame と AnvilScore を生成する。

仕様

対象:

* Rust: cargo test
* Node: npm test, npm run build
* Python: pytest, py_compile
* ANVIL.md の preferred verify command

分類:

* compile error
* test failure
* build failure
* timeout
* no verifier available
* pass

制約:

* dangerous command は実行しない
* timeout 必須
* stderr から primary error を抽出
* file:line があれば suspected_files に入れる

受け入れ条件

* Cargo compile error の file path が suspected_files に入る
* pytest failure の test name が primary_error に入る
* npm build failure が build/test failure として分類される
* verifier が見つからない場合 NoVerifierAvailable になる
* auto test 結果が AnvilScore に反映される
* dangerous preferred command は実行されない

⸻

Issue #9: Temporary Test Workspace を追加する

概要

生成テストを repo に直接書かず、session-scoped な一時領域に保存する。

背景

test がない repo では実行フィードバックが弱い。
一方で LLM が生成したテストをいきなり repo に保存するのは危険である。

課題

* generated test の保存先が未定義
* repo test と temporary test を区別できない
* cleanup / promote / discard の基盤がない
* path traversal や workspace escape のリスクがある

提案する解決策

state_root/sessions/<session-id>/tmp-tests/ に temporary test workspace を作る。

仕様

* session ごとに隔離
* path traversal を拒否
* cleanup 可能
* generated test は repo snapshot の通常変更として扱わない
* promote されるまで repo には書かない
* 実行時は timeout と approval policy に従う

受け入れ条件

* generated test が tmp-tests 配下に作成される
* tmp-tests が session ごとに分離される
* cleanup で削除できる
* generated test が無承認で repo に書かれない
* path traversal を含む filename が拒否される
* temporary test の metadata が session に保存される

⸻

Issue #10: Tester Skill v1 — smoke test を生成する

概要

テストが存在しない、または検証不能な repo に対して、最小限の smoke test を生成する。

背景

MASDP 的な構成では Tester が実行フィードバックの足場を作る。
Anvil ではまず、production code を直接触らない temporary smoke test に限定する。

課題

* test がない repo では FeedbackFrame が弱い
* 生成コードが正しいか実行で確認しづらい
* generated test の安全な扱いが必要

提案する解決策

Tester Skill v1 を実装し、repo stack に応じて temporary smoke test を生成する。

仕様

対象:

Stack	smoke test
Rust	compile / public API / integration smoke
Node	import / build / minimal script
Python	import / pytest / py_compile
Unknown	README / entrypoint 由来の smoke test 案

制約:

* Tester は production file を直接編集しない
* generated test は temporary workspace に保存
* 実行前に approval を取る
* 失敗結果は FeedbackFrame に変換

受け入れ条件

* test がない Rust repo で smoke test 案が生成される
* generated test は temporary workspace に保存される
* 実行前に approval が求められる
* 失敗時に FeedbackKind::TestFailure が生成される
* Tester が source file を直接 edit しない
* Tester 出力が malformed でも runtime が落ちない

⸻

Issue #11: generated test の promote / discard フローを追加する

概要

temporary workspace に生成されたテストを、ユーザー操作で repo に取り込む、または破棄できるようにする。

背景

generated test は有用な場合があるが、自動で repo に混ぜるべきではない。
ユーザーが明示的に選べる promote / discard フローが必要。

課題

* temporary test を repo に取り込む導線がない
* 不要な generated test を削除する導線がない
* promote 時の approval / overwrite handling が必要

提案する解決策

/tests コマンド群を追加する。

仕様

想定操作:

/tests
/tests promote <id>
/tests discard <id>

挙動:

* /tests で temporary tests を一覧表示
* promote は repo path へ書き込み
* discard は temporary test を削除
* promote 時は Write approval を通す
* overwrite は追加 confirmation を要求

受け入れ条件

* /tests で generated tests が表示される
* /tests promote <id> で repo に書き込まれる
* promote 時に approval が発生する
* overwrite 時に明示 confirmation が出る
* /tests discard <id> で temporary test が削除される
* 不正 ID で runtime が落ちない

⸻

Issue #23: generated code / generated test の sandbox policy を強化する

概要

生成コード・生成テストを実行する際の安全境界を強化する。

背景

Tester Skill や temporary test を導入すると、LLM 生成コードを実行する機会が増える。
既存の dangerous command block、offline policy、path confinement をさらに強化する必要がある。

課題

* timeout 後に子プロセスが残る可能性
* secret-looking な env var が log に出る可能性
* destructive command の分類を強化する必要
* network/offline policy を generated test 実行にも明確に適用する必要

提案する解決策

sandbox policy を runtime 側で強化する。

仕様

必須機能:

* process group kill
* execution timeout
* env secret masking
* destructive command pattern 強化
* network/offline policy の明示
* block event の FeedbackFrame 化

制約:

* safety は prompt ではなく runtime で強制
* block された command は UnsafeCommandBlocked
* approval message に危険理由を表示

受け入れ条件

* timeout 超過時に process group が kill される
* generated test が workspace 外に write できない
* secret-looking env var が log に出ない
* destructive command が approval 前に block される
* block event が FeedbackKind::UnsafeCommandBlocked になる
* offline mode で network command が拒否される

⸻

Epic C: Case Memory & CBR

Epic C 概要

成功した作業や失敗した作業を短い case として保存し、次回の類似タスクで再利用する。

背景

DomAgent 的な Case-Based Reasoning を Anvil に軽量導入する。
最初から本格的な Knowledge Graph を作るのではなく、session から成功・失敗の短い記録を抽出する。

課題

* 過去に成功した修正手順が再利用されない
* 同じ repo で同じ罠に再度はまりやすい
* raw conversation を保存・注入すると長く危険
* 成功と失敗のどちらも短く構造化する必要がある

提案する解決策

* CaseRecord を保存する
* 類似 task / stack / file / feedback で retrieval する
* failed case を anti-pattern として保存する
* relevant case を短く prompt に注入する

Epic C 受け入れ条件

* 成功 session から CaseRecord が生成される
* 類似 task で relevant case が retrieval される
* 失敗パターンが avoid precaution として再利用される
* raw conversation は case に含まれない
* secret-looking な値は保存されない

⸻

Issue #12: CaseRecord を抽出する

概要

成功した session から、再利用可能な短い CaseRecord を生成する。

背景

Anvil は session と working memory を持っているため、成功した作業を case 化しやすい。
ただし raw conversation を保存・再注入するのではなく、短い手続き記憶として残す必要がある。

課題

* 成功した修正の要点が次回に残らない
* 成功時に有効だった precautions が再利用されない
* verify command や変更ファイルの要約が保存されない

提案する解決策

成功条件を満たした session から CaseRecord を抽出する。

仕様

想定構造:

pub struct CaseRecord {
    pub repo_fingerprint: RepoFingerprint,
    pub task_signature: String,
    pub language_stack: Vec<String>,
    pub initial_feedback: Vec<FeedbackKind>,
    pub successful_precautions: Vec<String>,
    pub changed_files_summary: Vec<String>,
    pub verify_commands: Vec<String>,
    pub outcome_score: AnvilScore,
}

成功条件の例:

* tests passed
* build passed
* user approved final result
* explicit success marker
* AnvilScore が一定以上

制約:

* raw conversation は保存しない
* secret / absolute path を scrub
* 保存数上限を持つ

受け入れ条件

* build/test pass 後に CaseRecord が生成される
* case に raw LLM conversation が含まれない
* verify command が保存される
* successful precautions が保存される
* changed files summary が保存される
* scrub 後の case に secret-looking 値が含まれない

⸻

Issue #13: Case retrieval を実装する

概要

現在の task / repo / stack / feedback に類似する過去 case を検索し、短い summary として prompt に注入する。

背景

Case Memory は保存するだけでは意味がない。
類似タスクで短く再利用できる必要がある。

課題

* 類似度計算が未定義
* 無関係な case を prompt に入れると逆効果
* 大量の case を入れると token budget を圧迫する

提案する解決策

lexical / stack / repo fingerprint / file / feedback kind ベースの軽量 retrieval を実装する。

仕様

類似度要素:

task token overlap
language stack match
repo fingerprint match
same files/modules
same FeedbackKind
successful precautions overlap

注入形式:

RELEVANT LOCAL CASE:
- Similar Rust session bug fixed by editing src/session/store.rs.
- Key precaution: preserve session schema compatibility.
- Verification: cargo test.

制約:

* 最大 1〜3 件
* 類似度閾値未満は注入しない
* retrieval reason を debug log に残す

受け入れ条件

* 類似 task で relevant case が注入される
* 無関係 task では case が注入されない
* 注入 case は 3 件以下
* prompt に入る case は短い summary
* debug log に retrieval score と理由が残る
* retrieval failure で actor loop が止まらない

⸻

Issue #14: failed case / anti-pattern を保存する

概要

繰り返し失敗した行動を anti-pattern として保存し、次回は avoid precaution として使う。

背景

成功事例だけでなく、失敗パターンも小規模 LLM には重要である。
同じ focused edit failure や bash loop を避けるため、失敗パターンを短く記録する。

課題

* 同じ無効な修正を繰り返す
* 同じ command を再実行する
* 失敗した探索方針が記憶されない

提案する解決策

AntiPatternRecord を保存し、類似 task 時に avoid precaution として注入する。

仕様

想定構造:

pub struct AntiPatternRecord {
    pub task_signature: String,
    pub failed_action_summary: String,
    pub feedback_kind: FeedbackKind,
    pub avoid_precaution: String,
}

生成対象:

* repeated edit failure
* repeated bash failure
* no-progress loop
* repeated unsafe command attempt
* repeated malformed tool call

制約:

* TTL または manual clean 可能
* prompt への注入件数上限あり
* false positive は retire 可能

受け入れ条件

* 同じ focused edit が複数回失敗した場合 anti-pattern が生成される
* repeated bash failure が anti-pattern として保存される
* 次回類似 task で avoid precaution が注入される
* anti-pattern を削除または retire できる
* anti-pattern が prompt を過剰に支配しない

⸻

Epic D: Agentic Skills Layer

Epic D 概要

Reminder、Verifier、Tester、CaseRecall などの内部補助処理を、SoK 的な agentic skill として整理する。

背景

v0.2 / v0.3 の機能をそのまま turn.rs に追加し続けると、actor loop が肥大化する。
一定の挙動が固まったら、適用条件・実行・終了条件・権限を持つ内部 skill として整理する。

課題

* turn.rs に recovery / verification / reminder 分岐が集中する
* 補助処理の適用条件が散らばる
* skill ごとの権限境界がない
* Tester や CaseRecall の安全制御が明示されていない

提案する解決策

* AgentSkill trait を導入する
* skill registry を作る
* Precaution / Verifier を skill に移す
* skill permission / trust tier を導入する

Epic D 受け入れ条件

* skill registry 経由で内部補助処理を呼べる
* skill applicability が評価される
* skill failure は recoverable event として扱われる
* skill が tool safety policy を迂回できない
* read-only skill は Write/Edit/Bash を実行できない

⸻

Issue #15: AgentSkill trait と skill registry を追加する

概要

内部補助処理を skill として登録・実行できる共通 interface を追加する。

背景

Reminder、Verifier、Tester、CaseRecall が増えると、turn.rs 直書きでは保守が難しくなる。
SoK の考え方に近く、適用条件・実行ポリシー・終了条件・I/O を持つ内部 skill として整理する。

課題

* runtime 補助処理が分散・肥大化する
* どのタイミングで何を呼ぶかが見通しにくくなる
* skill の失敗や権限を統一的に扱えない

提案する解決策

AgentSkill trait と skill registry を追加する。

仕様

想定 trait:

pub trait AgentSkill {
    fn name(&self) -> &'static str;
    fn applicability(&self, state: &RuntimeState) -> bool;
    fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput>;
    fn termination(&self, output: &SkillOutput) -> bool;
}

registry:

* skill を登録
* applicability を評価
* 実行結果を structured log に保存
* failure を recoverable event として扱う

受け入れ条件

* registry 経由で skill を呼べる
* applicability false の skill は実行されない
* skill 実行ログが残る
* skill failure で actor loop 全体が落ちない
* skill から tool safety policy を迂回できない

⸻

Issue #16: PrecautionSkill と VerifierSkill を registry に移す

概要

v0.2 / v0.3 で実装した Reminder / Verification 処理を skill registry に移す。

背景

最初は実装速度を優先して turn.rs に直書きしてよいが、挙動が固まったら skill として整理するべきである。

課題

* turn.rs に Reminder / Verifier 分岐が残り続ける
* 補助処理の呼び出し条件が複雑化する
* 将来 Tester / CaseRecall を追加しづらくなる

提案する解決策

PrecautionSkill と VerifierSkill を追加し、既存処理を移設する。

仕様

* PrecautionSkill は FeedbackFrame から precautions を更新
* VerifierSkill は auto test / AnvilScore 生成を担当
* 既存 session format を壊さない
* v0.3 と同等の挙動を維持する

受け入れ条件

* v0.3 と同等の precautions が生成される
* v0.3 と同等の AnvilScore が生成される
* turn.rs の該当分岐が skill 呼び出しに置き換わる
* 既存 regression test が通る
* skill 化によって behavior が大きく変わらない

⸻

Issue #17: skill permission / trust tier を導入する

概要

skill ごとに実行可能な tool / write scope / trust level を制限する。

背景

内部 skill が増えると、補助処理が誤って Write/Edit/Bash を実行するリスクがある。
外部 skill や plugin を将来考えるなら、最初から trust tier を持つべきである。

課題

* skill の権限境界が曖昧
* Tester が repo source file を直接書けてしまう可能性
* CaseRecall のような read-only skill に write 権限は不要
* prompt instruction だけでは安全境界にならない

提案する解決策

skill ごとの trust tier と permission enforcement を runtime 側で実装する。

仕様

想定 enum:

pub enum SkillTrustTier {
    BuiltInReadOnly,
    BuiltInCanWriteTemp,
    BuiltInCanRequestWrite,
    ExternalDisabled,
}

例:

Skill	Trust tier
CaseRecallSkill	BuiltInReadOnly
PrecautionSkill	BuiltInReadOnly
VerifierSkill	BuiltInReadOnly / request bash through policy
TesterSkill	BuiltInCanWriteTemp
ExternalSkill	ExternalDisabled

受け入れ条件

* read-only skill が Write/Edit を実行できない
* TesterSkill が repo source file を直接 write できない
* permission violation が FeedbackFrame と log に残る
* External skill は明示 enable なしに実行されない
* enforcement は prompt ではなく runtime policy として行われる

⸻

Epic E: Repo Graph & Domain Context

Epic E 概要

repo 構造を使って、読むべきファイル、編集すべき場所、検証すべき command を狭める。

背景

小規模 LLM に大きな context を渡すより、repo 構造から関連箇所を正確に選ぶ方が効果的である。
Anvil には既に repo context ranking があるため、その発展として軽量 RepoGraph を導入する。

課題

* task 文字列だけでは関連ファイル推定が弱い
* implementation と test の関係を使えていない
* ANVIL.md instructions が path-aware ではない
* large repo で無関係ファイルが prompt に入りやすい

提案する解決策

* RepoGraph v1 を実装する
* graph-aware repo context ranking を導入する
* ANVIL.md instructions を path-aware にする

Epic E 受け入れ条件

* Rust / Node / Python の基本 graph が作れる
* graph を使って関連 file ranking が改善される
* implementation file と対応 test file がセットで候補化される
* path-scoped instruction だけが適切に注入される
* graph build failure でも agent は継続する

⸻

Issue #18: RepoGraph v1 を実装する

概要

repo 内の file / import / symbol / test / command / instruction 関係を軽量 graph として持つ。

背景

DomAgent 的な Knowledge Graph の軽量版として、まず repo 内の構造を持つ。
これにより、小規模 LLM に渡す context を狭く正確にできる。

課題

* ファイル間関係が runtime に明示されていない
* test file と implementation file の対応が推定されていない
* import / module 関係が context ranking に使われていない

提案する解決策

RepoGraph v1 を導入する。

仕様

対象関係:

file -> imports -> module/package
file -> defines -> function/type/module
test_file -> likely_covers -> implementation_file
command -> validates -> subsystem
ANVIL.md instruction -> applies_to -> path_pattern

初期対象:

* Rust
* Node
* Python

制約:

* huge repo では上限を持つ
* ignored directory / symlink を安全に扱う
* graph build failure は non-fatal
* graph は prompt に丸ごと入れない

受け入れ条件

* Rust repo で module / file 関係が生成される
* Node repo で package.json / import 関係が生成される
* Python repo で import 関係が生成される
* huge directory は上限により skip される
* graph build failure でも agent は継続する
* graph cache が利用される

⸻

Issue #19: graph-aware repo context ranking を導入する

概要

RepoGraph を使って、task に関連する file を rank し、prompt context に入れる。

背景

現在の repo context ranking は task 文字列ベースが中心。
Graph を使えば、compile error file の近傍や、implementation に対応する test を優先できる。

課題

* 関連 file の選択が lexical に偏る
* test と implementation の関係が使われない
* suspected files の周辺 context が足りない

提案する解決策

Graph neighbors、changed files、suspected files、task terms を組み合わせて ranking する。

仕様

ranking 要素:

* task terms
* file path
* symbol name
* imports
* suspected files
* changed files
* test-to-implementation relation
* graph neighbors

制約:

* context budget を超えない
* graph がない場合は既存 ranking に fallback
* ranking reason を debug log に出す

受け入れ条件

* compile error file の周辺 file が context に入る
* implementation file と対応 test file が一緒に候補化される
* unrelated large files が入りにくくなる
* graph がない repo でも既存挙動で動く
* debug log に ranking reason が出る

⸻

Issue #20: ANVIL.md instructions を path-aware にする

概要

ANVIL.md に path pattern ごとの instruction を書けるようにする。

背景

現在の ANVIL.md instructions は project-wide に注入される。
しかし実際には、session、bash tool、安全系、UI など path ごとに異なる制約がある。

課題

* 無関係な instruction が prompt に入る
* path-specific な安全制約を表現しづらい
* instruction injection が大きくなりやすい

提案する解決策

path-scoped instruction block を parse し、関連 path のみ prompt に注入する。

仕様

例:

[paths: src/session/**]
- Preserve session resume compatibility.
- Do not change session.json schema without migration.
[paths: src/tools/bash.rs]
- Never relax dangerous command blocking.

挙動:

* global instruction は従来通り
* path-scoped instruction は touched / suspected / target files に応じて注入
* invalid syntax は warning
* path pattern は workspace 内限定

受け入れ条件

* src/session/store.rs 作業時に session 用 instruction が注入される
* 無関係 path の instruction は注入されない
* global instruction は常に注入される
* invalid block で runtime が落ちない
* prompt injection 量が上限を超えない

⸻

Epic F: Observability / Evaluation / Training Export

Epic F 概要

Anvil の runtime event、feedback、score、precaution、case retrieval を構造化ログとして保存し、ローカルモデル比較や将来の fine-tuning dataset export に使えるようにする。

背景

Anvil は local-first であるため、モデル・設定・runtime policy の A/B 評価をローカルで行える。
また、Reminder 専用モデルを将来 fine-tuning するには、構造化された trace が必要になる。

課題

* LLM I/O だけでは runtime の評価に足りない
* precautions や scores が分析可能な形で残らない
* local model の比較基盤がない
* fine-tuning 用 dataset を安全に export できない

提案する解決策

* structured evaluation log を追加
* local model A/B evaluation harness を作る
* fine-tuning dataset export を追加する

Epic F 受け入れ条件

* 1 turn ごとに structured log が残る
* FeedbackFrame / Precaution / AnvilScore が log に含まれる
* 2 model のローカル比較ができる
* training dataset を JSONL export できる
* secret-looking な値は export されない

⸻

Issue #21: structured evaluation log を追加する

概要

LLM I/O だけでなく、runtime event、feedback、score、precaution、case retrieval を JSONL として保存する。

背景

Anvil の改善には、どの失敗が起き、どの precaution が入り、どの score になったかを後から分析できる必要がある。

課題

* LLM I/O log だけでは runtime の判断が追えない
* Reminder / Verifier / Case retrieval の効果を測れない
* 将来の dataset export に必要な構造化 trace がない

提案する解決策

turn-level の structured evaluation log を追加する。

仕様

保存対象:

- task
- model
- mode
- tool protocol
- tool calls
- FeedbackFrame
- active precautions
- AnvilScore
- changed file classes
- verify commands
- case retrieval result
- final outcome

制約:

* JSONL
* secret masking
* absolute path scrub option
* session clean で削除
* malformed event で runtime を止めない

受け入れ条件

* 1 turn ごとに structured log が出る
* FeedbackFrame が log に含まれる
* AnvilScore が log に含まれる
* active precautions が log に含まれる
* secret-looking env value が log に出ない
* session clean で削除される

⸻

Issue #22: local model A/B evaluation harness を作る

概要

同一タスクを複数 model / 複数 config で実行し、AnvilScore や wall time で比較する。

背景

Anvil は Ollama 専用・local-first なので、モデルごとの性能比較をローカルで行いやすい。
Precaution あり / なし、case memory あり / なしの比較も必要になる。

課題

* model selection の効果を比較しづらい
* runtime feature の on/off 比較ができない
* 成功率・時間・tool calls が統一的に見えない

提案する解決策

ローカル benchmark harness を追加する。

仕様

比較軸:

* model A vs model B
* precautions on/off
* case memory on/off
* native tools on/off
* auto test on/off

出力:

* AnvilScore
* wall time
* token usage
* tool call count
* success/failure
* failure kind

制約:

* destructive task は実行しない
* benchmark workspace を隔離
* cleanup 可能

受け入れ条件

* 同一 task を 2 model で実行できる
* 結果に AnvilScore が含まれる
* wall time が記録される
* tool call 数が記録される
* benchmark workspace が cleanup できる
* destructive command が実行されない

⸻

Issue #24: fine-tuning dataset export を追加する

概要

Reminder 専用モデルなどの将来学習用に、session log から curated dataset を export する。

背景

Reminder を最初は prompt-based で実装し、後から successful trace を使って SFT / LoRA するのが自然。
そのためには安全に scrub された dataset export が必要。

課題

* raw session log は学習データとして冗長かつ危険
* secret / absolute path / private repo 情報の scrub が必要
* success / failure trace を分けたい
* Reminder input/output 形式で抽出したい

提案する解決策

structured log から training dataset JSONL を export する。

仕様

出力例:

{
  "input": {
    "task": "...",
    "repo_summary": "...",
    "active_precautions": [],
    "feedback_frame": {}
  },
  "output": {
    "updated_precautions": []
  },
  "label": {
    "next_anvil_score": {},
    "success": true
  }
}

機能:

* success traces のみ export
* failed traces のみ export
* Reminder training format
* Verifier / classifier format は将来拡張
* manual review 用 summary 生成

制約:

* secret scrub
* absolute path anonymization
* raw conversation は原則含めない
* export は明示コマンドでのみ実行

受け入れ条件

* JSONL export ができる
* success traces のみ export できる
* failed traces のみ export できる
* secret-looking 値が除去される
* absolute path が匿名化される
* manual review 用 summary が生成される

⸻

推奨 MVP

最初の MVP は、Epic A と Epic B の中核だけに絞るのがよいです。

MVP 対象 Issue

#1  FeedbackFrame
#2  active_precautions
#3  Reminder Sidecar
#4  ACTIVE PRECAUTIONS prompt injection
#7  AnvilScore
#8  auto_test integration
#23 sandbox policy strengthening

MVP 仕様

Anvil は Act mode 中に test/build/edit/tool 失敗を検出した場合、それを FeedbackFrame に変換し、Reminder に渡し、次回生成用の active_precautions を更新し、次の model request に注入する。
その結果は AnvilScore と structured log に残る。

MVP 受け入れ条件

* compile error を起こす task で、次 iteration に compile error 起因の precaution が入る
* focused edit failure 後に read-before-edit 系 precaution が入る
* repeated bash command 後に同一 command 反復を避ける precaution が入る
* session resume 後も active precautions が復元される
* cargo test pass 後に AnvilScore.tests_passed = Some(true) になる
* malformed Reminder output で runtime が落ちない
* Plan mode 中に precaution 経由で source file write が許可されない
* generated test / generated code 実行に timeout と safety policy が適用される

⸻

GitHub 用 Epic Issue テンプレート

# Epic: <name>
## Overview
## Background
## Problems
## Proposed Solution
## Scope
### Included Issues
- #
### Out of Scope
- 
## Shared Interfaces
- 
## Dependencies
- 
## Parallelization Notes
- 
## Acceptance Criteria
- [ ] 

⸻

GitHub 用 Feature Issue テンプレート

# Feature: <name>
## Overview
## Background
## Problems
## Proposed Solution
## Specification
## Acceptance Criteria
- [ ] Given ..., When ..., Then ...
- [ ] Given ..., When ..., Then ...
## Dependencies
## Related Files
## Out of Scope

⸻

まとめ

並列開発を考えると、最初に固定すべき軸はこの 3 つです。

FeedbackFrame:
  失敗・実行結果を全機能で共有する入口
Precaution:
  次の生成を制約する working memory
AnvilScore:
  進捗を deterministic に評価する出口

この 3 つが入ると、Anvil の runtime は次のループを持てます。

実行
  ↓
FeedbackFrame
  ↓
Reminder
  ↓
Precaution
  ↓
Act prompt injection
  ↓
Verification
  ↓
AnvilScore
  ↓
Case Memory / Eval / Training Export

Anvil の方向性としては、外から見える UX は Plan / Act のまま、内部では feature を Epic 単位で並列に育てるのが最も安全です。
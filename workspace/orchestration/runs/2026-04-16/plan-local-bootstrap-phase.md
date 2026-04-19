# ローカル bootstrap phase 改善計画 — 2026-04-16

## 対象

- Task A: `LocalPhase` enum 導入
- Task B: pre/post bootstrap 専用 request builder
- Task C: required-target tracking
- Task D: target-specific tool catalog
- Task E: phase-aware completion/recovery
- Task F: external 5-run regression harness

## 成功指標

- scaffold 後に `page.tsx` / `globals.css` / `layout.tsx` が未実装のまま lane 完了しない
- `git.status` / read-only `shell.exec` / plan drift を bootstrap lane 中に再発させない
- `file.write` / `file.rewrite` の直接到達率を上げる
- follow-up request の prompt token / latency / timeout を下げる

## 仕様具体化

1. local bootstrap 制御は `PreBootstrap` と `PostBootstrap` を別 struct のまま持ちつつ、外側は `LocalPhase` で単一状態機械として扱う。
2. post-bootstrap は target queue と required target 集合を別管理し、required target は実 mutation 完了まで queue から外れても未完了扱いを維持する。
3. bootstrap lane 中の follow-up / guarded retry request は通常 request builder を使い回さず、必要最小限の message tail と phase 専用 system prompt を使う。
4. bootstrap lane 中の許可ツールは phase と current target から決定し、prompt / native tools / rejection reason が同じ catalog を参照する。
5. local mode の completion/recovery は phase-aware にする。lane が active な間は `ANVIL_FINAL` や prose-only completion を受理しない。
6. external regression harness は CLI 外部実行を 5 回繰り返し、成功回数と平均時間を確認できる形で残す。

## 仕様レビュー

- 指摘 1: `local_mode_active()` だけで Done path / post-tool final を受理すると、required target 未達でも lane を抜ける。
- 指摘 2: allowed tools の定義が prompt 文言・native tool catalog・runtime rejection に分散しており、挙動がずれやすい。
- 指摘 3: bootstrap lane follow-up は一部だけ短縮されているが、guarded retry や初回 follow-up では通常 request builder に戻る箇所がある。
- 指摘 4: required target 管理は page/globals/layout の実 mutation 完了を見たいが、現状は queue 主体で completion 判定と recovery 判定が分離している。

## 仕様レビュー指摘対応

- `LocalPhase::PostBootstrap` の active 判定を required target 完了に従属させる。
- phase-aware final gate を導入し、lane active 中の final acceptance を抑止する。
- tool catalog と request builder を phase 単位の helper に寄せる。

## 設計

1. `LocalPhase` accessor を維持しつつ、`LocalBootstrapLock` に required target 完了判定と target 固有 catalog 参照 API を集約する。
2. `App` 側に `build_local_phase_request(...)` を追加し、top-level turn / follow-up / guarded retry から共通利用する。
3. `LocalToolCatalog` 相当の helper を `src/app/mod.rs` に置き、allowed tool 名・forbidden 文言・tool schema filter を一元化する。
4. `handle_done_path_anvil_final_guard` と `handle_post_tool_anvil_final_gate` に phase-aware suppression を追加する。
5. provider integration test で required target 未完了時の lane 継続、plan drift/read drift/shell drift 抑止、target 固有 catalog を検証する。
6. external harness は複数回コマンド実行を共通化した軽量スクリプトを追加し、対象 integration test 群を 5 回回す。

## 設計レビュー

- 指摘 1: request builder を `agentic.rs` 側だけに置くと top-level turn と guarded retry が再び分岐する。
- 指摘 2: tool catalog helper が native tool calling 専用だと text protocol 側の rejection 文言と再分岐する。
- 指摘 3: harness 対象が broad すぎると CI 時間を圧迫するため、bootstrap lane regression 群に限定すべき。

## 設計レビュー指摘対応

- request builder は `App` helper として `mod.rs` に置く。
- tool catalog helper は prompt 文言・schema filter・runtime validation で共用する。
- harness は local bootstrap regression の高価値テスト群だけを対象にする。

## 実装計画

1. 既存の `LocalPhase` 差分をコンパイル可能に整える。
2. failing test を追加して、required target 未完了で final を受理しないことを固定する。
3. phase-aware request builder と tool catalog helper を導入する。
4. completion/recovery を phase-aware に寄せる。
5. external 5-run harness を追加する。
6. targeted test → regression harness → 必要なら関連 test suite の順で確認する。

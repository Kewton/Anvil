# CommandAgent Local Recovery Plan Review

作成日: 2026-06-17

対象:

- `.workspace/CommandAgent/local-recovery-plan.md`

## レビュー観点

以下の観点でレビューした。

1. 問題箇所の洗い出しが latest large root の証拠に基づいているか
2. Anvil との差分を正しく説明しているか
3. 対策が局所的で、profile / repair / policy を不必要に太らせていないか
4. 実装順序が妥当か
5. 受け入れ条件が成功率ではなく failure class の改善を見ているか

## 総合判定

判定: 妥当。ただし LR-3 は範囲管理が必要。

理由:

- 最大問題である `required_artifacts` enforcement timing を P0/LR-1 に置いている。
- Anvil の構造差分を「最終成果物契約」と「step-local expected paths」の分離として
  説明できている。
- Next.js dependency policy や Python import isolation をすぐ profile 拡張で直さず、
  P0/P1 修正後に再評価する方針になっている。
- large eval の成功率改善ではなく、失敗分類の明確化を成功条件にしている。

## 良い点

### 1. P0 を最優先にしている

`large-rust-app-modify` の failure は profile 固有ではない。分析 phase の後に
final artifact を要求して止まるのは runtime の境界バグである。

この問題を先に直さず profile を触ると、Rust profile に不要な「最初の phase で
lib.rs を作れ」のようなルールを入れる誘惑が出る。計画はそれを避けている。

### 2. `required_artifacts` を即削除しない判断は現実的

理想的には `StepPlan.required_artifacts` 自体を削りたいが、既存 plan YAML や
prompt/reporting との互換がある。今回の局所リカバリでは hard gate から外し、
将来的な schema 整理に回す判断が妥当。

### 3. LR-2 を LR-1 から分けている

重複注入は P0 と同時に見えているが、根本原因は別である。

- LR-1: いつ enforce するか
- LR-2: どう canonicalize するか

この分離はレビューしやすい。

### 4. LR-3 で broad Bash whitelist に逃げていない

`grep` が block されたから plain `grep` を全部許す、という対処は簡単だが危険。
計画はまず planner 側の canonical verifier を `grep -q` / `python -m py_compile`
などに寄せる。これは設計思想に合う。

## 懸念

### 懸念 1: LR-3 が大きくなりやすい

verifier command 契約は、lint、Bash policy、planner prompt、profile guidance の
4 箇所にまたがる。ここを一気に直すと PR が大きくなり、局所リカバリではなく
広い redesign になる。

推奨:

- LR-3a: quote-aware shell-control detection
- LR-3b: canonical verifier prompt/snapshot
- LR-3c: residual Bash allowlist decision

に分けてもよい。最初の PR では `python -c` semicolon 誤判定と canonical prompt
だけで十分。

### 懸念 2: `required_artifacts` を hard gate から外すと検出が弱くなる可能性

`/run-plan` 単体で `required_artifacts` が欠けても成功扱いになる可能性がある。

ただし、これは本来の意味に戻るだけである。単体 step plan で作るべきものは
`expected_paths` に入れるべきで、final artifact は ultra 全体または eval
success_check で見るべき。

推奨:

- `/run-plan` の report には missing final artifacts を warning として出す余地はある。
- ただし今回の LR-1 では warning 実装も入れない。まず hard gate を外す。

### 懸念 3: LR-2 の dedupe でユーザー入力の誤りを隠す可能性

同じ artifact が複数回指定された場合、dedupe すると入力ミスに気づきにくい。

ただし、重複指定は意味を持たない。stable dedupe は妥当。

推奨:

- meta/report に duplicate count を出すほどではない。
- unit test で order-preserving dedupe を確認する。

### 懸念 4: dependency_missing を後回しにすると Next.js はまだ失敗する

LR-1 から LR-3 を直しても、Next.js large は dependency_missing で落ちる可能性が
高い。

これは問題ない。今回の目的は full pass ではなく、failure class を整理すること。
dependency policy は product/eval policy の判断を含むため、runtime bug と分けるべき。

### 懸念 5: Python import isolation は再現環境依存

外部 repo の `app.schemas` が拾われる現象は、ユーザー環境の PYTHONPATH や
site-packages に依存する可能性がある。

推奨:

- P0/P1 修正後に再現するか確認してから扱う。
- 扱う場合は profile rule より先に eval runner の test env isolation を確認する。

## スコープ監査

| 項目 | 計画に含むか | 判定 |
| --- | --- | --- |
| final artifact enforcement timing | 含む | 妥当。最大の共通 runtime bug。 |
| required_artifacts dedupe | 含む | 妥当。noisy failure を減らす局所修正。 |
| verifier quote-aware lint | 含む | 妥当。ただし PR 分割推奨。 |
| canonical verifier prompt | 含む | 妥当。planner と policy の語彙を揃える。 |
| plain grep allowlist 拡張 | 原則含まない | 妥当。まず生成を `grep -q` に寄せる。 |
| Next.js dependency policy | 含まない | 妥当。共通修正後に判断。 |
| Python import package marker | 含まない | 妥当。再現確認後。 |
| repair 回数増加 | 含まない | 妥当。思想に合う。 |
| profile 太らせ | 含まない | 妥当。P0/P1 後に必要最小限。 |

## 追加すべきテスト観点

計画に概ね含まれているが、実装時は以下を必ず固定する。

1. `execute_step_plan` は `required_artifacts` 欠落だけでは fail しない。
2. `execute_ultra_plan` は全 phase 完了後に `required_artifacts` 欠落で fail する。
3. inspect/report step は final artifacts 未作成でも pass できる。
4. create/edit step の `expected_paths` 欠落は fail のまま。
5. duplicate required artifacts は saved plan/rendered plan で重複しない。
6. quoted semicolon は shell chaining として扱わない。
7. unquoted `;`, `&&`, `||` は引き続き reject する。
8. generated verifier examples は `python -m py_compile` / `cargo test` /
   `npm run build` / `grep -q` を示す。

## リスク評価

### 低リスク

- LR-1: hard gate の位置修正
- LR-2: stable dedupe

理由:

- Anvil の元構造に戻す方向であり、機能追加ではない。
- 既存の `expected_paths` gate は残る。

### 中リスク

- LR-3: verifier command 契約

理由:

- lint / policy / prompt の境界を触る。
- 安全性と使いやすさのバランスが必要。

対策:

- quote-aware 判定と canonical prompt を先に行う。
- Bash allowlist の拡張は rerun 結果を見てから判断する。

### 今回は扱わない高リスク

- dependency install policy
- profile-specific package layout rule
- repair loop 強化

## 最終レビュー結論

局所リカバリ計画は妥当である。

特に、今回の最大問題を `required_artifacts` の「型そのもの」ではなく
「enforcement timing / contract boundary」と捉えている点が正しい。

実装順は以下を推奨する。

1. LR-1 と LR-2 を同じ小 PR で実施する。
2. unit test を追加して `large-rust-app-modify` 型の早期停止を固定する。
3. その後 LR-3 を別 PR に分ける。
4. focused large rerun で residual を再分類する。
5. Next.js dependency / Python import は residual として残った場合だけ扱う。

この順序なら、CommandAgent の思想である「能力を足す前に曖昧さを削る」に
沿っている。


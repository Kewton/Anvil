# v0.6.7 Architecture Continuation Validation - 2026-06-07

## Scope

`future-architecture-direction-20260607.md` の方向性に対して、小さく実装し、実際のローカル LLM を使って検証した結果を整理する。

重視した観点は次の通り。

- coding 以外の docs / data / ops / research に拡張できること
- coding / feature improvement / TDD のような難しいタスクでも破綻しないこと
- controller が過度なパターンマッチングや個別ルールに戻らないこと
- LLM の意味判断を typed contract / typed repair decision へ落とし、実行側は単純で検証可能な責務に留めること

## Implemented Small Steps

### 1. ProjectProfile adoption guard

ProjectProfileConfirmation が README 形式の Node CLI タスクを `CommandObservation` や docs 系成果物へ弱めるケースを止めた。

変更方針は task kind 固定のルールではなく、first-pass ObjectiveContract が `SourceFiles` と implementation / test / setup を要求している場合に、profile 側の non-source deliverable 採用を拒否する形にした。

これにより、coding タスクが sidecar LLM の分類揺れで docs / ops / command-observation lifecycle へ落ちる問題はかなり抑えられた。

### 2. ObjectiveContract projection cleanup

README / package.json / `npm test` / `cargo test` / `node --test` のような project intent から、implementation / test / setup の contract を補強した。

これは coding 専用の大きな分岐を足すのではなく、ProjectProfile から contract input を作る境界を少し強くするための最小対応である。

### 3. EvidenceRunner timeout split

通常の verifier / structured evidence command は 60 秒で止め、dependency setup は 300 秒まで許容するように分離した。

理由は、`npm test` が自己再帰や hang に入った時に controller lifecycle が止まる一方、`npm install` のような setup は 60 秒では短すぎるため。

### 4. Structured npm dependency setup

structured `npm test` 実行前に、`package.json` が dependencies / devDependencies / optionalDependencies / peerDependencies を宣言していて `node_modules` が無い場合だけ、`npm install --ignore-scripts` を実行するようにした。

これは provider abstraction ではなく EvidenceRunner の責務内に収めた。Rust / Python と同様に、証拠実行の前提を整える最小の setup step として扱う。

### 5. Diagnostic parser normalization

実LLMの diagnostic JSON は、失敗時に次のような形で崩れた。

- semantic report fields が `failure_clusters` 配列の末尾へ漏れる
- legacy diagnostic object と semantic failure report object を隣接して2個出す
- retry 時に cluster を増やし過ぎて truncation する

前者2つは parser normalization で受けられるようにした。これは task-specific な文字列判定ではなく、diagnostic schema の shape drift を正規化する処理である。

truncation については未解決で、次の P0 候補に残る。

## Actual LLM Validation

実行にはローカル Ollama の `qwen3.5:9b` / `qwen3.5:2b` を使った。

### Docs task

Prompt: `docs/runbook.md` に Overview / Backup / Deploy / Rollback / Verification を含む runbook を作成する。コードやテストは不要。

Result: pass。

`docs/runbook.md` が作成され、1 iteration で完了した。docs-only lifecycle は現時点でかなり自然に動く。

### Data task

Prompt: inline JSON から `output/users.csv` を作成する。列は `id,name,team`。

Result: mostly pass。

CSV は正しく作成され、最終状態も done になった。ただし途中で `Command evidence missing` が繰り返し出た。

これは data output の evidence が schema/content check ではなく command observation 側へ揺れる余地がまだ残っていることを示す。coding 以外へ広げるには、data の evidence_kind を command-observation へ安易に寄せない必要がある。

### Node CLI task

Prompt: CSV を読み、部署別人数を出力する Node CLI。`package.json` / implementation / tests / README を作り、`npm test` を通す。

Result: fail, but useful failures.

観測した失敗は次の通り。

- tests が `describe` / `it` を使うが runner が plain `node` で失敗
- `vitest` を使うが dependencies が install されず失敗
- `node:test` の API を誤用して失敗
- diagnostic JSON が shape drift し、parser が落ちる
- 長い diagnostic retry で truncation が起きる

この検証から、profile adoption guard、diagnostic parser normalization、structured npm dependency setup、verifier timeout split を入れた。

ただし、Node coding task はまだ安定 pass ではない。LLM がテストランナー選択を誤るケースと、diagnostic response が長大化するケースが残る。

### Rust TDD task

Prompt: `is_palindrome` を Rust library として TDD で作る。`Cargo.toml` / `src/lib.rs` / `tests/palindrome.rs` を作り、`cargo test` を通す。

Result: fail, root cause clarified.

1回目は `Cargo.toml` の `[[test]]` に `name` が無く、manifest parse で失敗した。diagnostic LLM は `Cargo.toml` / setup が対象だと判断したが、JSON が adjacent object shape で崩れて parser が落ちた。parser normalization でこれは受けられるようにした。

2回目は parser が diagnostic を受け取れた。LLM は again `Cargo.toml` / setup を正しく指摘した。しかし repair pipeline の accepted action は `src/lib.rs` / implementation に固定され、`Cargo.toml` 修正へ移らず、patch rejected を繰り返して `repair_exhausted` になった。

これは今回の最重要発見である。

## Current Root Bottleneck

最大のボトルネックは、DiagnosticRepairWorker が LLM の意味判断を受け取れても、RepairJob / patch admission 側の target authority が古い `current_role` / `failure_location` に固定されること。

つまり「LLM が弱い」だけではない。

今回の Rust TDD では、LLM は `Cargo.toml` が原因だと正しく診断した。それでも controller 側が repair target を `src/lib.rs` に戻したため、正しい修復に進めなかった。

現状の構造では次が起きる。

1. EvidenceRunner が失敗を観測する
2. Diagnostic LLM が semantic target を返す
3. Parser は target を読める
4. しかし RepairJob / shadow pipeline が stale current role を優先する
5. patch admission が target mismatch として reject する
6. 同じ repair prompt を繰り返し、`repair_exhausted` に収束する

これはモグラ叩きの中心である。parser や prompt を改善しても、typed diagnostic target が repair authority へ渡らなければ成功率は伸びにくい。

## Direction Revision

`future-architecture-direction-20260607.md` の方向性は概ね妥当だが、優先順位を修正する。

### P0: Diagnostic target authority

accepted diagnostic target を RepairJob の authoritative target candidate として扱う必要がある。

ただし無制限に LLM target を採用すると危険なので、採用条件は typed boundary に寄せる。

- target path が changed candidates / safe excerpts / allowed workspace 内にある
- target role が ObjectiveContract 上の deliverable component と矛盾しない
- failure kind と target role が明確に衝突しない
- user-required artifact や verifier artifact を弱めない

これは task-kind 固定分岐ではなく、contract / evidence / safe candidate に基づく admission policy として実装すべき。

### P1: Diagnostic response budget control

shape drift の parser normalization は有効だったが、truncation は parser だけでは救えない。

次は diagnostic prompt / retry policy に、短い JSON 1個だけを返す制約を typed に持たせる必要がある。ルールベースの文字列増築ではなく、DiagnosticRequestPacket に max clusters / max observations / no prose / no duplicate cluster のような構造制約を持たせる方が保守しやすい。

### P1: Data evidence stabilization

data task は最終 pass したが、途中で command evidence を要求した。

data の evidence は `SchemaCheck` / `ContentCheck` / `OutputFileExists` を第一候補にし、`CommandObservation` は明示的に command 実行を成果物とする task のみに限定すべき。

### P2: Hard coding validation set

docs/data の pass だけでは偶然を排除できない。

今後の最小検証は、少なくとも次を毎回含める。

- docs: section-required runbook
- data: CSV / JSON schema output
- ops: command observation only task
- coding: Node CLI with owned tests
- TDD: Rust library with integration tests
- feature improvement: existing codeへの小変更と既存テスト維持

## Maintainability Assessment

今回の対応は、まだ許容範囲の複雑性に収まっている。

理由は、追加した分岐の多くが task-specific prompt 文字列ではなく、次の typed boundary に置かれているため。

- ObjectiveContract projection
- ProjectProfile adoption condition
- EvidenceRunner setup / timeout
- diagnostic schema normalization
- recovery job admission

一方で、このまま `task_contract.rs` や repair pipeline に helper を足し続けると再び複雑化する。

次のリファクタリング候補は、ProjectProfile -> Contract inputs 変換、Diagnostic target selection、Repair target admission をそれぞれ独立した小モジュールに分離すること。

## Current Status

近づいた点:

- coding task が profile drift で docs / ops lifecycle に落ちる問題は改善した
- diagnostic parser は実LLMの代表的な shape drift を受けられるようになった
- EvidenceRunner は hang しにくくなった
- docs-only は自然に完了する
- data-only は完了するが evidence drift が残る

まだ遠い点:

- hard coding / TDD は repair target authority の不整合で失敗する
- diagnostic retry が長大化すると truncation で失敗する
- data / ops / research の evidence_kind 安定性はまだ十分ではない
- 成功率を語るには、固定評価セットでの継続計測がまだ必要

## Next Small Implementation Hypothesis

次に検証すべき仮説は次の1つに絞る。

「accepted diagnostic target が safe candidate かつ ObjectiveContract と矛盾しない場合、RepairJob target をその target に rebind すれば、Rust TDD の `Cargo.toml` 欠落修復は `repair_exhausted` ではなく次の verifier run へ進む」

この仮説が通れば、repair_exhausted の主因が prompt 品質ではなく controller の target authority mismatch であることをさらに強く確認できる。

この仮説が通らない場合は、DiagnosticRepairWorker の role / target / patch admission の責務分離を再設計する必要がある。

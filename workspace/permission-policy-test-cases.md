# PermissionPolicy 表駆動テストケース一覧

## 目的

`PermissionPolicy` の `base_requirement()` と `effective_requirement()` を表駆動で検証するためのケース一覧。

確認対象:

- `PermissionMode` ごとの基本要件
- `InteractionMode` による最終要件の変化
- 非対話時の `NonInteractiveBehavior`
- `HardConfirm` が非対話で常に `Deny` になること

## 推奨 Rust テストケース型

```rust
struct Case {
    name: &'static str,
    mode: PermissionMode,
    category: PermissionCategory,
    cx: ExecutionContext,
    expected_base: PermissionRequirement,
    expected_effective: PermissionRequirement,
}
```

## 共通コンテキスト

```rust
const INTERACTIVE: ExecutionContext = ExecutionContext {
    interaction_mode: InteractionMode::Interactive,
    non_interactive_ask: NonInteractiveBehavior::Deny,
    non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
    non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
};

const NON_INTERACTIVE_STRICT: ExecutionContext = ExecutionContext {
    interaction_mode: InteractionMode::NonInteractive,
    non_interactive_ask: NonInteractiveBehavior::Deny,
    non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
    non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
};

const NON_INTERACTIVE_PERMISSIVE: ExecutionContext = ExecutionContext {
    interaction_mode: InteractionMode::NonInteractive,
    non_interactive_ask: NonInteractiveBehavior::Allow,
    non_interactive_soft_confirm: NonInteractiveBehavior::Allow,
    non_interactive_hard_confirm: NonInteractiveBehavior::Allow,
};
```

注意:

- `NON_INTERACTIVE_PERMISSIVE` でも `HardConfirm` は `Deny` であることが正しい

## ケース一覧

| name | mode | category | context | expected_base | expected_effective |
|---|---|---|---|---|---|
| ask_read_interactive | `Ask` | `Read` | `INTERACTIVE` | `Allow` | `Allow` |
| ask_edit_interactive | `Ask` | `Edit` | `INTERACTIVE` | `Ask` | `Ask` |
| ask_exec_safe_interactive | `Ask` | `ExecSafe` | `INTERACTIVE` | `Ask` | `Ask` |
| ask_exec_sensitive_interactive | `Ask` | `ExecSensitive` | `INTERACTIVE` | `Ask` | `Ask` |
| ask_exec_dangerous_interactive | `Ask` | `ExecDangerous` | `INTERACTIVE` | `HardConfirm` | `HardConfirm` |
| ask_subagent_read_interactive | `Ask` | `SubagentRead` | `INTERACTIVE` | `Ask` | `Ask` |
| ask_subagent_write_interactive | `Ask` | `SubagentWrite` | `INTERACTIVE` | `Ask` | `Ask` |
| accept_edits_read_interactive | `AcceptEdits` | `Read` | `INTERACTIVE` | `Allow` | `Allow` |
| accept_edits_edit_interactive | `AcceptEdits` | `Edit` | `INTERACTIVE` | `Allow` | `Allow` |
| accept_edits_exec_safe_interactive | `AcceptEdits` | `ExecSafe` | `INTERACTIVE` | `Ask` | `Ask` |
| accept_edits_exec_sensitive_interactive | `AcceptEdits` | `ExecSensitive` | `INTERACTIVE` | `Ask` | `Ask` |
| accept_edits_exec_dangerous_interactive | `AcceptEdits` | `ExecDangerous` | `INTERACTIVE` | `HardConfirm` | `HardConfirm` |
| accept_edits_subagent_read_interactive | `AcceptEdits` | `SubagentRead` | `INTERACTIVE` | `Ask` | `Ask` |
| accept_edits_subagent_write_interactive | `AcceptEdits` | `SubagentWrite` | `INTERACTIVE` | `Ask` | `Ask` |
| bypass_read_interactive | `BypassPermissions` | `Read` | `INTERACTIVE` | `Allow` | `Allow` |
| bypass_edit_interactive | `BypassPermissions` | `Edit` | `INTERACTIVE` | `Allow` | `Allow` |
| bypass_exec_safe_interactive | `BypassPermissions` | `ExecSafe` | `INTERACTIVE` | `Allow` | `Allow` |
| bypass_exec_sensitive_interactive | `BypassPermissions` | `ExecSensitive` | `INTERACTIVE` | `SoftConfirm` | `SoftConfirm` |
| bypass_exec_dangerous_interactive | `BypassPermissions` | `ExecDangerous` | `INTERACTIVE` | `HardConfirm` | `HardConfirm` |
| bypass_subagent_read_interactive | `BypassPermissions` | `SubagentRead` | `INTERACTIVE` | `Allow` | `Allow` |
| bypass_subagent_write_interactive | `BypassPermissions` | `SubagentWrite` | `INTERACTIVE` | `Ask` | `Ask` |
| ask_edit_non_interactive_strict | `Ask` | `Edit` | `NON_INTERACTIVE_STRICT` | `Ask` | `Deny` |
| ask_exec_safe_non_interactive_strict | `Ask` | `ExecSafe` | `NON_INTERACTIVE_STRICT` | `Ask` | `Deny` |
| ask_exec_dangerous_non_interactive_strict | `Ask` | `ExecDangerous` | `NON_INTERACTIVE_STRICT` | `HardConfirm` | `Deny` |
| accept_edits_edit_non_interactive_strict | `AcceptEdits` | `Edit` | `NON_INTERACTIVE_STRICT` | `Allow` | `Allow` |
| accept_edits_exec_sensitive_non_interactive_strict | `AcceptEdits` | `ExecSensitive` | `NON_INTERACTIVE_STRICT` | `Ask` | `Deny` |
| accept_edits_exec_dangerous_non_interactive_strict | `AcceptEdits` | `ExecDangerous` | `NON_INTERACTIVE_STRICT` | `HardConfirm` | `Deny` |
| bypass_exec_safe_non_interactive_strict | `BypassPermissions` | `ExecSafe` | `NON_INTERACTIVE_STRICT` | `Allow` | `Allow` |
| bypass_exec_sensitive_non_interactive_strict | `BypassPermissions` | `ExecSensitive` | `NON_INTERACTIVE_STRICT` | `SoftConfirm` | `Deny` |
| bypass_exec_dangerous_non_interactive_strict | `BypassPermissions` | `ExecDangerous` | `NON_INTERACTIVE_STRICT` | `HardConfirm` | `Deny` |
| ask_edit_non_interactive_permissive | `Ask` | `Edit` | `NON_INTERACTIVE_PERMISSIVE` | `Ask` | `Allow` |
| ask_exec_safe_non_interactive_permissive | `Ask` | `ExecSafe` | `NON_INTERACTIVE_PERMISSIVE` | `Ask` | `Allow` |
| ask_exec_dangerous_non_interactive_permissive | `Ask` | `ExecDangerous` | `NON_INTERACTIVE_PERMISSIVE` | `HardConfirm` | `Deny` |
| accept_edits_exec_sensitive_non_interactive_permissive | `AcceptEdits` | `ExecSensitive` | `NON_INTERACTIVE_PERMISSIVE` | `Ask` | `Allow` |
| accept_edits_exec_dangerous_non_interactive_permissive | `AcceptEdits` | `ExecDangerous` | `NON_INTERACTIVE_PERMISSIVE` | `HardConfirm` | `Deny` |
| bypass_exec_sensitive_non_interactive_permissive | `BypassPermissions` | `ExecSensitive` | `NON_INTERACTIVE_PERMISSIVE` | `SoftConfirm` | `Allow` |
| bypass_exec_dangerous_non_interactive_permissive | `BypassPermissions` | `ExecDangerous` | `NON_INTERACTIVE_PERMISSIVE` | `HardConfirm` | `Deny` |
| bypass_subagent_write_non_interactive_permissive | `BypassPermissions` | `SubagentWrite` | `NON_INTERACTIVE_PERMISSIVE` | `Ask` | `Allow` |

## 最低限入れるべきテスト群

### 1. mode x category の base requirement 全件

- 3 mode x 7 category = 21ケース

### 2. 非対話 strict の境界ケース

- `Ask` -> `Deny`
- `SoftConfirm` -> `Deny`
- `HardConfirm` -> `Deny`

### 3. 非対話 permissive の境界ケース

- `Ask` -> `Allow`
- `SoftConfirm` -> `Allow`
- `HardConfirm` -> `Deny`

### 4. 回帰防止ケース

- `bypass_permissions + exec_dangerous` が `Allow` にならない
- `accept_edits + edit` が `Ask` に戻らない
- `bypass_permissions + subagent_write` が自動 `Allow` に広がりすぎない

## 推奨 Rust 実装イメージ

```rust
#[test]
fn permission_policy_table() {
    let cases = vec![
        Case {
            name: "ask_read_interactive",
            mode: PermissionMode::Ask,
            category: PermissionCategory::Read,
            cx: INTERACTIVE,
            expected_base: PermissionRequirement::Allow,
            expected_effective: PermissionRequirement::Allow,
        },
        // ...
    ];

    for case in cases {
        let policy = PermissionPolicy::from_mode(case.mode, case.category);
        assert_eq!(policy.base_requirement(), case.expected_base, "{}", case.name);
        assert_eq!(policy.effective_requirement(case.cx), case.expected_effective, "{}", case.name);
    }
}
```

## 補助テスト

- `PermissionPolicy::from_mode()` が pure であること
- serialize / deserialize 後も挙動が変わらないこと
- 新しい `PermissionCategory` を追加した際に `match` 漏れでコンパイルエラーになること

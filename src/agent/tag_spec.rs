//! タグベース形式のツール構文定義テーブル
//!
//! プロンプト生成（tool_protocol_system_prompt）とパーサー（tag_parser）の
//! 両方がこのテーブルを参照することで、ツール定義の二重管理を防止する。

/// タグベース形式のツール構文定義
pub struct ToolTagSpec {
    /// ツール名（例: "file.read"）
    pub name: &'static str,
    /// 属性名一覧（例: &["path"]）
    pub attributes: &'static [&'static str],
    /// 子要素名一覧（例: &["content"]）。空なら自己閉じタグ形式
    pub child_elements: &'static [&'static str],
    /// プロンプト用の構文例
    pub example: &'static str,
}

/// 全ツールのタグベース構文定義（一元管理）
pub const TOOL_TAG_SPECS: &[ToolTagSpec] = &[
    ToolTagSpec {
        name: "file.read",
        attributes: &["path"],
        child_elements: &[],
        example: r#"<tool name="file.read" path="./src/main.rs"/>"#,
    },
    ToolTagSpec {
        name: "file.write",
        attributes: &["path"],
        child_elements: &["content"],
        example: r#"<tool name="file.write" path="./src/main.rs"><content>...</content></tool>"#,
    },
    ToolTagSpec {
        name: "file.edit",
        attributes: &["path"],
        child_elements: &["old_string", "new_string"],
        example: r#"<tool name="file.edit" path="./src/main.rs"><old_string>...</old_string><new_string>...</new_string></tool>"#,
    },
    ToolTagSpec {
        name: "file.search",
        attributes: &["root", "pattern"],
        child_elements: &[],
        example: r#"<tool name="file.search" root="." pattern="search term"/>"#,
    },
    ToolTagSpec {
        name: "shell.exec",
        attributes: &["command"],
        child_elements: &[],
        example: r#"<tool name="shell.exec" command="ls -la"/>"#,
    },
    ToolTagSpec {
        name: "web.fetch",
        attributes: &["url"],
        child_elements: &[],
        example: r#"<tool name="web.fetch" url="https://example.com"/>"#,
    },
    ToolTagSpec {
        name: "web.search",
        attributes: &["query"],
        child_elements: &[],
        example: r#"<tool name="web.search" query="search keywords"/>"#,
    },
    ToolTagSpec {
        name: "file.edit_anchor",
        attributes: &["path"],
        child_elements: &["old_content", "new_content"],
        example: r#"<tool name="file.edit_anchor" path="./src/main.rs"><old_content>fn old_code()</old_content><new_content>fn new_code()</new_content></tool>"#,
    },
    ToolTagSpec {
        name: "agent.explore",
        attributes: &["scope"],
        child_elements: &["prompt"],
        example: r#"<tool name="agent.explore" scope="..."><prompt>...</prompt></tool>"#,
    },
    ToolTagSpec {
        name: "agent.plan",
        attributes: &["scope"],
        child_elements: &["prompt"],
        example: r#"<tool name="agent.plan" scope="..."><prompt>...</prompt></tool>"#,
    },
];

/// ツール名からスペックを検索
pub fn find_spec(tool_name: &str) -> Option<&'static ToolTagSpec> {
    TOOL_TAG_SPECS.iter().find(|s| s.name == tool_name)
}

pub fn tool_result_message(name: &str, output: &str) -> String {
    format!("[tool:{name}]\n{output}")
}

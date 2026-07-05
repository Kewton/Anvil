pub fn floor_char_boundary(value: &str, max: usize) -> usize {
    let mut end = max.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub fn truncate_at_char_boundary(value: &str, max: usize) -> &str {
    &value[..floor_char_boundary(value, max)]
}

pub fn excerpt_with_marker(value: &str, max: usize, marker: &str) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut out = truncate_at_char_boundary(value, max).to_string();
    out.push_str(marker);
    out
}

pub fn excerpt_with_newline_marker(value: &str, max: usize, marker: &str) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut out = truncate_at_char_boundary(value, max).to_string();
    out.push('\n');
    out.push_str(marker);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_at_char_boundary_never_panics_on_multibyte_lengths() {
        let fixture = "abc日本語def除外🙂かなカナ";
        for max in 0..=fixture.len() + 8 {
            let truncated = truncate_at_char_boundary(fixture, max);
            assert!(truncated.len() <= max.min(fixture.len()));
            assert!(fixture.starts_with(truncated));
        }
    }

    #[test]
    fn excerpt_with_marker_preserves_valid_utf8() {
        let fixture = "prefix日本語除外suffix";
        let excerpt = excerpt_with_marker(fixture, 10, "...[truncated]");
        assert!(excerpt.starts_with("prefix日"));
        assert!(excerpt.ends_with("...[truncated]"));
    }
}

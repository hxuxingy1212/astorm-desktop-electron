//! 文本处理小工具（按字符截断，保证 UTF-8 边界安全）。
//!
//! 用途：把可能超长的文本塞进提示词或工具结果时做上限收口
//! （记忆工具的概念正文、注入会话的相关记忆摘要）。

/// 按**字符**（非字节）截断；超长时在末尾追加 `…`。
///
/// 中文一个字算一个字符，因此不会切断 UTF-8 字节序列（`s[..n]` 的字节切片会 panic）。
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_chars_not_bytes() {
        assert_eq!(truncate_chars("abcdef", 3), "abc…");
        assert_eq!(truncate_chars("中文测试", 2), "中文…");
        assert_eq!(truncate_chars("abc", 3), "abc", "等长不截断");
        assert_eq!(truncate_chars("abc", 10), "abc");
        assert_eq!(truncate_chars("abc", 0), "");
        assert_eq!(truncate_chars("", 3), "");
        // 混合中英与 emoji：按字符数而非字节数
        assert_eq!(truncate_chars("a中🦀b", 3), "a中🦀…");
    }
}

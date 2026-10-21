//! 文本行级 diff / 增删统计：过程行「编辑 +N -N」「写入 +N」与展开 diff 预览的唯一实现。
//!
//! 与前端 `utils/lineChangeStat.ts` / `utils/unifiedDiff.ts` 同口径：去公共前后缀 + 行级 LCS，
//! 超过单元格上限时退化为「全删再全加」，避免大文件把界面卡住。

/// LCS 单元格上限（超出退化为保守估算）。
const MAX_LCS_CELLS: usize = 60_000;

/// 拆逻辑行：末尾单个换行不算多出一行（与编辑器行号一致）。
fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// 公共前后缀裁剪后的 (before 中段, after 中段, 公共前缀长度, 公共后缀长度)。
fn trim_common<'a>(before: &'a [&'a str], after: &'a [&'a str]) -> (&'a [&'a str], &'a [&'a str], usize, usize) {
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut before_tail = before.len();
    let mut after_tail = after.len();
    while before_tail > prefix && after_tail > prefix && before[before_tail - 1] == after[after_tail - 1] {
        before_tail -= 1;
        after_tail -= 1;
    }
    (
        &before[prefix..before_tail],
        &after[prefix..after_tail],
        prefix,
        before.len() - before_tail,
    )
}

/// 计算 before → after 的真实增删行数。
pub(crate) fn compute_line_change_stat(before: &str, after: &str) -> (usize, usize) {
    let before_lines = split_lines(before);
    let after_lines = split_lines(after);
    let (mid_before, mid_after, _, _) = trim_common(&before_lines, &after_lines);

    if mid_before.is_empty() {
        return (mid_after.len(), 0);
    }
    if mid_after.is_empty() {
        return (0, mid_before.len());
    }
    if mid_before.len() * mid_after.len() > MAX_LCS_CELLS {
        return (mid_after.len(), mid_before.len());
    }

    // 行级 LCS 长度（滚动一维数组）
    let mut lcs = vec![0usize; mid_after.len() + 1];
    for i in 1..=mid_before.len() {
        let mut previous_diagonal = 0;
        for j in 1..=mid_after.len() {
            let previous_row = lcs[j];
            if mid_before[i - 1] == mid_after[j - 1] {
                lcs[j] = previous_diagonal + 1;
            } else {
                lcs[j] = lcs[j].max(lcs[j - 1]);
            }
            previous_diagonal = previous_row;
        }
    }
    let unchanged = lcs[mid_after.len()];
    (mid_after.len() - unchanged, mid_before.len() - unchanged)
}

/// 中段按 LCS 输出 ` ` / `+` / `-` 前缀行（超出上限退化全删全加）。
fn append_middle(out: &mut String, before: &[&str], after: &[&str]) {
    if before.is_empty() {
        for line in after {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
        return;
    }
    if after.is_empty() {
        for line in before {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
        return;
    }
    if before.len() * after.len() > MAX_LCS_CELLS {
        for line in before {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
        for line in after {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
        return;
    }

    let (n, m) = (before.len(), after.len());
    // lcs[i][j] = before[i..] 与 after[j..] 的最长公共子序列长度
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let (mut i, mut j) = (0usize, 0usize);
    while i < n || j < m {
        if i < n && j < m && before[i] == after[j] {
            out.push(' ');
            out.push_str(before[i]);
            out.push('\n');
            i += 1;
            j += 1;
        } else if i >= n {
            out.push('+');
            out.push_str(after[j]);
            out.push('\n');
            j += 1;
        } else if j >= m {
            out.push('-');
            out.push_str(before[i]);
            out.push('\n');
            i += 1;
        } else if lcs[i][j + 1] >= lcs[i + 1][j] {
            out.push('+');
            out.push_str(after[j]);
            out.push('\n');
            j += 1;
        } else {
            out.push('-');
            out.push_str(before[i]);
            out.push('\n');
            i += 1;
        }
    }
}

/// 构造限上下文的单 hunk unified diff（`file_label` 用于 `--- a/…` / `+++ b/…`）。
///
/// `context` 为每个改动块两侧保留的未修改行数；新建 = `/dev/null`，删除 = `+++ /dev/null`。
pub(crate) fn build_unified_diff(before: &str, after: &str, file_label: &str, context: usize) -> String {
    let before_lines = split_lines(before);
    let after_lines = split_lines(after);
    let (mid_before, mid_after, prefix, suffix) = trim_common(&before_lines, &after_lines);
    let limited_prefix = prefix.min(context);
    let limited_suffix = suffix.min(context);

    let created = before_lines.is_empty() && !after_lines.is_empty();
    let deleted = !before_lines.is_empty() && after_lines.is_empty();

    let old_start = if before_lines.is_empty() { 0 } else { prefix - limited_prefix + 1 };
    let new_start = if after_lines.is_empty() { 0 } else { prefix - limited_prefix + 1 };
    let old_count = limited_prefix + mid_before.len() + limited_suffix;
    let new_count = limited_prefix + mid_after.len() + limited_suffix;

    let mut out = String::new();
    out.push_str(if created { "--- /dev/null\n" } else { "--- a/" });
    if !created {
        out.push_str(file_label);
        out.push('\n');
    }
    out.push_str(if deleted { "+++ /dev/null\n" } else { "+++ b/" });
    if !deleted {
        out.push_str(file_label);
        out.push('\n');
    }
    out.push_str(&format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    ));

    for line in &before_lines[prefix - limited_prefix..prefix] {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    append_middle(&mut out, mid_before, mid_after);
    for line in &before_lines[prefix + mid_before.len()..prefix + mid_before.len() + limited_suffix] {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 文件路径的末段（diff 的 `a/…` `b/…` 显示名）。
pub(crate) fn file_leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 统计真实增删行数() {
        // 单行替换 → +1 -1
        assert_eq!(compute_line_change_stat("a", "b"), (1, 1));
        // 纯新增
        assert_eq!(compute_line_change_stat("", "a\nb"), (2, 0));
        // 末尾追加
        assert_eq!(compute_line_change_stat("a", "a\nb"), (1, 0));
        // 中间插入不把后续行算成改动（LCS）
        assert_eq!(compute_line_change_stat("a\nb\nc", "a\nx\nb\nc"), (1, 0));
        // 未变化
        assert_eq!(compute_line_change_stat("a\nb", "a\nb"), (0, 0));
    }

    #[test]
    fn 限上下文diff_头与上下文行数正确() {
        let before = "1\n2\n3\n4\n5\n6\n7\n8";
        let after = "1\n2\n3\nX\n5\n6\n7\n8";
        let patch = build_unified_diff(before, after, "f.txt", 2);
        // 改动在第 4 行:上下各 2 行 → old/new 计数 = 2 + 1 + 2
        assert!(patch.contains("@@ -2,5 +2,5 @@"), "patch:\n{patch}");
        assert!(patch.contains("-4"));
        assert!(patch.contains("+X"));
        assert!(!patch.contains(" 8"), "尾部超出上下文的行不应出现");
    }

    #[test]
    fn 新建与删除用dev_null() {
        let created = build_unified_diff("", "a\nb", "new.txt", 3);
        assert!(created.contains("--- /dev/null"));
        assert!(created.contains("+++ b/new.txt"));
        assert!(created.contains("@@ -0,0 +1,2 @@"));

        let deleted = build_unified_diff("a\nb", "", "old.txt", 3);
        assert!(deleted.contains("--- a/old.txt"));
        assert!(deleted.contains("+++ /dev/null"));
        assert!(deleted.contains("@@ -1,2 +0,0 @@"));
    }

    #[test]
    fn 叶子名取末段() {
        assert_eq!(file_leaf("a/b/c.rs"), "c.rs");
        assert_eq!(file_leaf("c.rs"), "c.rs");
        assert_eq!(file_leaf("a\\b\\c.rs"), "c.rs");
    }
}

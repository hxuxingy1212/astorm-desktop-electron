//! OKF Bundle 合规校验。
//!
//! 检查项（`Error` 阻断「Bundle 合规」，`Warning` 仅为可改进提示）：
//!
//! | 项 | 级别 |
//! |---|---|
//! | 概念文件 frontmatter 解析失败 | Error |
//! | frontmatter 缺 `type` 或为空 | Error |
//! | 概念 slug 命中 OKF 保留文件名（`index` / `log`） | Error |
//! | 相对 `.md` 链接指向不存在的文件 | Error |
//! | 相对链接越出 Bundle 根 | Warning |
//! | 相对非 `.md` 资源（图片等）不存在 | Warning |
//! | bundle-absolute 链接（`/xxx`，OKF 不鼓励） | Warning |
//! | 缺 `title`（OKF 推荐字段） | Warning |
//! | 缺根 `index.md` | Warning |
//!
//! 相对链接口径：OKF 用**相对 Markdown 链接表达概念之间的关系**，故 `.md` 目标缺失属实质错误；
//! 图片等资源是可选的（本管道不做资源内联），缺失只提示不判错。

use std::path::{Path, PathBuf};

use super::paths::{BundlePaths, is_reserved_file_name};
use super::writer::load_all_concepts;

/// 问题级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueLevel {
    Error,
    Warning,
}

/// 单条校验问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateIssue {
    pub level: IssueLevel,
    /// 相关概念 ID（bundle 级问题为空串）。
    pub concept_id: String,
    pub message: String,
}

impl ValidateIssue {
    fn error(concept_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: IssueLevel::Error,
            concept_id: concept_id.into(),
            message: message.into(),
        }
    }

    fn warning(concept_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level: IssueLevel::Warning,
            concept_id: concept_id.into(),
            message: message.into(),
        }
    }
}

/// 校验报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidateReport {
    /// 检查的概念文件数。
    pub checked: usize,
    pub issues: Vec<ValidateIssue>,
}

impl ValidateReport {
    pub fn errors(&self) -> usize {
        self.count(IssueLevel::Error)
    }

    pub fn warnings(&self) -> usize {
        self.count(IssueLevel::Warning)
    }

    fn count(&self, level: IssueLevel) -> usize {
        self.issues.iter().filter(|i| i.level == level).count()
    }

    /// 无 Error 即视为合规（Warning 不阻断）。
    pub fn is_ok(&self) -> bool {
        self.errors() == 0
    }
}

/// 校验 Bundle（`BundlePaths` 指向的 `wiki/`）。
pub fn validate_bundle(paths: &BundlePaths) -> ValidateReport {
    let mut report = ValidateReport::default();

    if !paths.wiki_dir().is_dir() {
        report
            .issues
            .push(ValidateIssue::error("", format!("wiki 目录不存在：{}", paths.wiki_dir().display())));
        return report;
    }

    let loaded = load_all_concepts(paths);
    report.checked = loaded.concepts.len() + loaded.errors.len();

    // ① 解析失败
    for (concept_id, message) in &loaded.errors {
        report
            .issues
            .push(ValidateIssue::error(concept_id.clone(), message.clone()));
    }

    // ② 逐概念检查
    let wiki = paths.wiki_dir();
    for (concept_id, _, doc) in &loaded.concepts {
        if doc.frontmatter.concept_type.trim().is_empty() {
            report.issues.push(ValidateIssue::error(
                concept_id.clone(),
                "frontmatter 缺少必填字段 type（必须为非空字符串）",
            ));
        }
        if is_reserved_file_name(&format!("{}.md", doc.slug())) {
            report.issues.push(ValidateIssue::error(
                concept_id.clone(),
                "概念 slug 命中了 OKF 保留文件名（index.md / log.md 不得作为概念）",
            ));
        }
        if doc
            .frontmatter
            .title
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            report
                .issues
                .push(ValidateIssue::warning(concept_id.clone(), "建议补充 title（OKF 推荐字段）"));
        }

        let base_dir = paths
            .concept_path(concept_id)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| wiki.clone());
        for link in extract_links(&doc.body) {
            check_link(&mut report, concept_id, &base_dir, &wiki, &link);
        }
    }

    // ③ bundle 级：根索引存在性
    if !paths.root_index_path().is_file() {
        report.issues.push(ValidateIssue::warning(
            "",
            format!("缺少根索引 {}", paths.root_index_path().display()),
        ));
    }

    report
}

/// 按类型校验一条链接。
fn check_link(
    report: &mut ValidateReport,
    concept_id: &str,
    base_dir: &Path,
    wiki: &Path,
    url: &str,
) {
    let target = strip_fragment_and_query(url);
    if target.is_empty() {
        return; // 纯锚点（`#section`）或空链接
    }
    if target.starts_with('/') {
        report.issues.push(ValidateIssue::warning(
            concept_id,
            format!("bundle-absolute 链接不被推荐（OKF 要求相对链接）：{url}"),
        ));
        return;
    }
    if has_scheme(target) {
        return; // 外部链接 / mailto / data: 等，不校验
    }

    // 「越出 Bundle 根」按 **Bundle 根**判，不能按 `wiki/` 判：`raw/` 也是 Bundle 的一部分，
    // 概念正文里链接回原始会话（`../../raw/sessions/x.md`）是合法且被鼓励的溯源方式。
    // 真正该报的是「链接指向 Bundle 之外的文件」（如 `../../../../etc/passwd`）。
    let bundle_root = wiki.parent().unwrap_or(wiki);
    let resolved = resolve_relative(base_dir, target);
    if !resolved.starts_with(bundle_root) {
        report.issues.push(ValidateIssue::warning(
            concept_id,
            format!("相对链接越出 Bundle 根：{url}"),
        ));
        return;
    }
    // 指向目录时按该目录的索引文件判定
    let resolved = if resolved.is_dir() {
        resolved.join(super::paths::RESERVED_INDEX)
    } else {
        resolved
    };
    if resolved.is_file() {
        return;
    }

    let is_markdown = target.to_ascii_lowercase().ends_with(".md");
    let shown = resolved
        .strip_prefix(bundle_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| resolved.display().to_string());
    if is_markdown {
        report.issues.push(ValidateIssue::error(
            concept_id,
            format!("相对链接指向不存在的文件：{url}（应为 {shown}）"),
        ));
    } else {
        report.issues.push(ValidateIssue::warning(
            concept_id,
            format!("相对资源不存在（可选资源，不影响合规）：{url}"),
        ));
    }
}

/// 从 Markdown 正文提取链接目标（`](target)`，覆盖链接与图片）。
fn extract_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find("](") {
        let after = &rest[idx + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        let raw = after[..end].trim();
        // 允许 `url "title"` / `<url>` 写法：取首个空白前的片段并去尖括号
        let url = raw
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(['<', '>']);
        if !url.is_empty() {
            out.push(url.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

/// 去掉 `#fragment` 与 `?query`。
fn strip_fragment_and_query(url: &str) -> &str {
    let cut = url.find(['#', '?']).unwrap_or(url.len());
    &url[..cut]
}

/// 是否带 URL scheme（`http:` / `https:` / `mailto:` / `data:` …）。
fn has_scheme(url: &str) -> bool {
    match url.find(':') {
        Some(idx) if idx > 0 => {
            let scheme = &url[..idx];
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        _ => false,
    }
}

/// 相对路径解析（处理 `.` / `..` / 多余分隔符；不做符号链接解析）。
fn resolve_relative(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for seg in rel.replace('\\', "/").split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            s => path.push(s),
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_bundle(tag: &str) -> (PathBuf, BundlePaths) {
        let root = std::env::temp_dir().join(format!("astorm_mem_validate_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = BundlePaths::new(&root);
        paths
            .ensure_dirs(&["decisions".into(), "concepts".into()])
            .unwrap();
        (root, paths)
    }

    fn write(paths: &BundlePaths, id: &str, content: &str) {
        let path = paths.concept_path(id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// 校验前先重建索引：把「缺根索引」这条 bundle 级告警排除在逐概念用例之外。
    fn check(paths: &BundlePaths) -> ValidateReport {
        super::super::writer::rebuild_indexes(paths).unwrap();
        validate_bundle(paths)
    }

    fn concept(type_line: &str, title: &str, body: &str) -> String {
        format!("---\n{type_line}\ntitle: {title}\n---\n\n{body}\n")
    }

    #[test]
    fn valid_bundle_has_no_errors() {
        let (root, paths) = tmp_bundle("ok");
        write(&paths, "decisions/use-jwt", &concept("type: Decision", "用 JWT", "见 [相关](./other.md)"));
        write(&paths, "decisions/other", &concept("type: Decision", "相关", "正文"));
        super::super::writer::rebuild_indexes(&paths).unwrap();

        let report = validate_bundle(&paths);
        assert!(report.is_ok(), "应合规: {:?}", report.issues);
        assert_eq!(report.checked, 2);
        assert_eq!(report.errors(), 0);
        assert_eq!(report.warnings(), 0, "{:?}", report.issues);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_or_empty_type_is_error() {
        let (root, paths) = tmp_bundle("type");
        write(&paths, "concepts/no-type", "---\ntitle: 无类型\n---\n\n正文\n");
        write(&paths, "concepts/empty-type", "---\ntype: \"  \"\ntitle: 空\n---\n\n正文\n");
        let report = validate_bundle(&paths);
        assert_eq!(report.errors(), 2, "{:?}", report.issues);
        assert!(report.issues.iter().all(|i| i.message.contains("type")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_frontmatter_is_error() {
        let (root, paths) = tmp_bundle("broken");
        write(&paths, "concepts/bad", "# 没有 frontmatter\n");
        let report = validate_bundle(&paths);
        assert_eq!(report.errors(), 1);
        assert_eq!(report.checked, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 相对 `.md` 链接缺失 → Error（OKF 用它表达概念关系）。
    #[test]
    fn dangling_markdown_link_is_error() {
        let (root, paths) = tmp_bundle("dangling");
        write(
            &paths,
            "decisions/a",
            &concept("type: Decision", "A", "见 [不存在](./nope.md)"),
        );
        let report = check(&paths);
        assert_eq!(report.errors(), 1, "{:?}", report.issues);
        assert!(report.issues[0].message.contains("不存在的文件"));
        // Windows 下相对路径以 `\` 渲染，统一成 `/` 再断言
        assert!(
            report.issues[0]
                .message
                .replace('\\', "/")
                .contains("decisions/nope.md"),
            "应给出 Bundle 相对路径: {:?}",
            report.issues
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 相对非 `.md` 资源缺失 → 仅 Warning（本管道不做资源内联）。
    #[test]
    fn dangling_image_link_is_warning_only() {
        let (root, paths) = tmp_bundle("img");
        write(
            &paths,
            "concepts/a",
            &concept("type: Fact", "A", "![图](./diagram.png)"),
        );
        let report = check(&paths);
        assert!(report.is_ok(), "图片缺失不应判错: {:?}", report.issues);
        assert_eq!(report.warnings(), 1);
        assert!(report.issues[0].message.contains("相对资源不存在"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn absolute_link_is_warning() {
        let (root, paths) = tmp_bundle("abs");
        write(
            &paths,
            "decisions/a",
            &concept("type: Decision", "A", "见 [绝对](/concepts/x.md)"),
        );
        let report = check(&paths);
        assert!(report.is_ok(), "{:?}", report.issues);
        assert_eq!(report.warnings(), 1);
        assert!(report.issues[0].message.contains("bundle-absolute"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn external_and_anchor_links_are_skipped() {
        let (root, paths) = tmp_bundle("ext");
        write(
            &paths,
            "concepts/a",
            &concept(
                "type: Fact",
                "A",
                "[外链](https://example.com/x) [锚点](#section) [邮件](mailto:a@b.c) [数据](data:text/plain,x)",
            ),
        );
        let report = check(&paths);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 跨目录与带锚点的链接应正确解析。
    #[test]
    fn cross_dir_and_fragment_links_resolve() {
        let (root, paths) = tmp_bundle("cross");
        write(&paths, "decisions/use-jwt", &concept("type: Decision", "A", "x"));
        write(
            &paths,
            "concepts/auth-note",
            &concept(
                "type: Fact",
                "B",
                "见 [决策](../decisions/use-jwt.md#理由) 与 [本节](#小节)",
            ),
        );
        let report = validate_bundle(&paths);
        assert!(report.is_ok(), "跨目录 ../ 链接应能解析: {:?}", report.issues);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escaping_bundle_is_warning() {
        let (root, paths) = tmp_bundle("escape");
        write(
            &paths,
            "concepts/a",
            &concept("type: Fact", "A", "见 [逃逸](../../../../etc/passwd)"),
        );
        let report = check(&paths);
        assert!(report.is_ok());
        assert_eq!(report.warnings(), 1);
        assert!(report.issues[0].message.contains("越出 Bundle 根"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `raw/` 也在 Bundle 内：概念正文链回原始会话（`../../raw/sessions/x.md`）是合法溯源，
    /// 不能按「离开 wiki/」误报。dangling 时才该报 Error。
    #[test]
    fn raw_session_link_inside_bundle_is_allowed() {
        let (root, paths) = tmp_bundle("rawlink");
        std::fs::create_dir_all(paths.raw_sessions_dir()).unwrap();
        std::fs::write(paths.raw_file("2025-10-09-s"), "# 会话\n").unwrap();
        write(
            &paths,
            "decisions/a",
            &concept("type: Decision", "A", "见 [原文](../../raw/sessions/2025-10-09-s.md)"),
        );
        let report = check(&paths);
        assert!(report.is_ok(), "{:?}", report.issues);
        assert_eq!(report.warnings(), 0, "Bundle 内的 raw 链接不应报警: {:?}", report.issues);

        // dangling 的 raw 链接仍然是 Error（错别字/会话被删要能发现）
        write(
            &paths,
            "decisions/b",
            &concept("type: Decision", "B", "见 [原文](../../raw/sessions/不存在.md)"),
        );
        let report = check(&paths);
        assert_eq!(report.errors(), 1, "{:?}", report.issues);
        assert!(report.issues.iter().any(|i| i.message.contains("不存在的文件")), "{:?}", report.issues);
        assert!(report.issues.iter().any(|i| i.message.contains("raw/sessions/不存在.md")), "应给出 Bundle 相对路径: {:?}", report.issues);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_title_is_warning() {
        let (root, paths) = tmp_bundle("title");
        write(&paths, "concepts/untitled", "---\ntype: Fact\n---\n\n正文\n");
        let report = check(&paths);
        assert!(report.is_ok());
        assert_eq!(report.warnings(), 1);
        assert!(report.issues[0].message.contains("title"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_root_index_is_warning() {
        let (root, paths) = tmp_bundle("noindex");
        write(&paths, "concepts/a", &concept("type: Fact", "A", "x"));
        let report = validate_bundle(&paths);
        assert!(report.issues.iter().any(|i| i.message.contains("缺少根索引")));
        // 重建索引后该告警消失
        super::super::writer::rebuild_indexes(&paths).unwrap();
        let report = validate_bundle(&paths);
        assert!(!report.issues.iter().any(|i| i.message.contains("缺少根索引")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_wiki_dir_is_error() {
        let root = std::env::temp_dir().join(format!("astorm_mem_validate_nowiki_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = BundlePaths::new(&root);
        let report = validate_bundle(&paths);
        assert_eq!(report.errors(), 1);
        assert!(report.issues[0].message.contains("wiki 目录不存在"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn link_extraction_handles_titles_and_brackets() {
        let links = extract_links("[a](./x.md \"标题\") ![b](<./y.png>) [c](./z.md 't')");
        assert_eq!(links, vec!["./x.md", "./y.png", "./z.md"]);
    }

    #[test]
    fn scheme_detection() {
        assert!(has_scheme("https://a.b"));
        assert!(has_scheme("mailto:a@b"));
        assert!(has_scheme("data:text/plain,x"));
        assert!(!has_scheme("./relative.md"));
        assert!(!has_scheme("../up.md"));
        assert!(!has_scheme("plain.md"));
        // 冒号在路径中但不构成 scheme
        assert!(!has_scheme("./a:b.md"));
    }

    #[test]
    fn relative_resolution_handles_dots() {
        let base = Path::new("/w/wiki/concepts");
        assert_eq!(resolve_relative(base, "../decisions/x.md"), Path::new("/w/wiki/decisions/x.md"));
        assert_eq!(resolve_relative(base, "./y.md"), Path::new("/w/wiki/concepts/y.md"));
        assert_eq!(resolve_relative(base, "z.md"), Path::new("/w/wiki/concepts/z.md"));
        assert_eq!(resolve_relative(base, "a//b/./c.md"), Path::new("/w/wiki/concepts/a/b/c.md"));
        // 越界：pop 到根之后仍继续 pop 不再上溯
        assert_eq!(resolve_relative(Path::new("/w"), "../../x"), Path::new("/x"));
    }

    /// 读侧容忍未知 type（OKF 要求），自定义分类目录不影响校验。
    #[test]
    fn unknown_type_and_custom_dir_pass() {
        let (root, paths) = tmp_bundle("unknown");
        write(&paths, "runbooks/rollback", "---\ntype: Runbook\ntitle: 回滚\n---\n\n正文\n");
        let report = check(&paths);
        assert!(report.is_ok(), "未知 type 必须被容忍: {:?}", report.issues);
        assert_eq!(report.warnings(), 0, "自定义目录 + 未知 type 不应产生任何告警: {:?}", report.issues);
        let _ = std::fs::remove_dir_all(&root);
    }
}

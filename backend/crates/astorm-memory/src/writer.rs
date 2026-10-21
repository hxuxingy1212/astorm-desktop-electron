//! Bundle 写入与索引维护：原子写、各级 `index.md` 重建、`log.md` 变更历史。
//!
//! 约定：
//! - **原子写**：先写 `*.md.tmp` 再 `rename`，写一半崩溃不会留下半个概念文件
//! - **保留文件**：`index.md`（目录导航）与 `log.md`（变更历史）由本模块生成，**永不**被当作概念
//! - **读侧动态**：目录索引按 `wiki/` 下**实际存在**的分类目录生成（不做白名单），
//!   故手工放入的自定义分类目录同样会出现在根索引里
//! - **链接修复**：[`repair_relative_links`] 把编译器自造的悬空相对链接降级为纯文本
//!   （单次调用、无工具的编译器无法知道哪些概念存在，必然偶发悬空链接）

use std::path::{Path, PathBuf};

use super::concept::ConceptDocument;
use super::paths::{BundlePaths, is_reserved_file_name};
use super::types::LogEntry;

/// `log.md` 标题行。
const LOG_HEADER: &str = "# 变更历史";
/// `log.md` 最多保留的条目数（超出后截断尾部最旧记录）。
const MAX_LOG_ENTRIES: usize = 2_000;

/// 扫描到的概念文件（含最后修改时间）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedConcept {
    pub concept_id: String,
    pub path: PathBuf,
    /// 最后修改时间（unix 秒；取不到为 0）。
    pub updated_at: i64,
}

/// 全量扫描 + 解析结果。
#[derive(Debug, Default)]
pub struct LoadedConcepts {
    /// 解析成功的概念：(concept_id, updated_at, 文档)，按 concept_id 升序。
    pub concepts: Vec<(String, i64, ConceptDocument)>,
    /// 解析失败：(concept_id, 错误信息)——不阻断其他概念，交由 validate 报告。
    pub errors: Vec<(String, String)>,
}

/// 原子写入概念文件（tmp + rename）。
///
/// 返回 `true` 表示覆盖了已有文件，`false` 表示新建。
pub fn write_concept(paths: &BundlePaths, doc: &ConceptDocument) -> Result<bool, String> {
    let path = paths.concept_path(&doc.concept_id);
    let markdown = doc.to_markdown()?;
    let existed = path.exists();
    atomic_write(&path, &markdown)?;
    Ok(existed)
}

/// 删除概念文件（不存在视为成功）。
pub fn remove_concept(paths: &BundlePaths, concept_id: &str) -> Result<(), String> {
    let path = paths.concept_path(concept_id);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("删除概念失败({}): {e}", path.display()))?;
    }
    Ok(())
}

/// 原子写文件：先写同目录临时文件，再 rename。
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!("非法路径（无父目录）: {}", path.display()));
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("创建目录失败({}): {e}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("非法文件名: {}", path.display()))?;
    let tmp = parent.join(format!("{file_name}.tmp"));
    std::fs::write(&tmp, content.as_bytes())
        .map_err(|e| format!("写入失败({}): {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("提交失败({}): {e}", path.display()))
}

/// 扫描概念文件（不解析内容），按 concept_id 升序。
pub fn scan_concepts(paths: &BundlePaths) -> Vec<ScannedConcept> {
    paths
        .concept_files()
        .into_iter()
        .map(|(concept_id, path)| {
            let updated_at = mtime_secs(&path);
            ScannedConcept {
                concept_id,
                path,
                updated_at,
            }
        })
        .collect()
}

/// 全量扫描并解析：损坏文件计入 `errors` 而不中断。
pub fn load_all_concepts(paths: &BundlePaths) -> LoadedConcepts {
    let mut out = LoadedConcepts::default();
    for scanned in scan_concepts(paths) {
        match std::fs::read_to_string(&scanned.path) {
            Ok(content) => match ConceptDocument::from_markdown(&scanned.concept_id, &content) {
                Ok(doc) => out.concepts.push((scanned.concept_id, scanned.updated_at, doc)),
                Err(e) => out.errors.push((scanned.concept_id, e)),
            },
            Err(e) => out
                .errors
                .push((scanned.concept_id, format!("读取概念文件失败: {e}"))),
        }
    }
    out
}

/// 重建根索引与各分类目录索引（按 `wiki/` 下实际存在的目录）。
///
/// 返回被写入索引文件的相对目录名（空串 = 根）。
pub fn rebuild_indexes(paths: &BundlePaths) -> Result<Vec<String>, String> {
    let loaded = load_all_concepts(paths);
    let mut written = vec![String::new()];

    // 按目录分组（根下散落概念归入 `""`）
    let mut by_dir: std::collections::BTreeMap<String, Vec<&ConceptDocument>> = Default::default();
    for (_, _, doc) in &loaded.concepts {
        by_dir.entry(doc.dir()).or_default().push(doc);
    }

    // ① 各分类目录索引
    for (dir, docs) in &by_dir {
        if dir.is_empty() {
            continue;
        }
        let mut list = docs.clone();
        list.sort_by(|a, b| a.concept_id.cmp(&b.concept_id));
        let body = format!("# {dir}\n\n{}", render_links(&list, "./"));
        atomic_write(&paths.index_path(dir), &body)?;
        written.push(dir.clone());
    }

    // ② 根索引：frontmatter 声明 okf_version + 分类导航 + 根下散落概念
    let root_dir = paths.root_index_path();
    let root_docs = by_dir.get("").cloned().unwrap_or_default();
    let mut root = String::from("---\nokf_version: \"");
    root.push_str(super::paths::OKF_VERSION);
    root.push_str("\"\n---\n\n# 记忆 Wiki\n\n");
    root.push_str(&format!("共 {} 个概念。\n", loaded.concepts.len()));

    let mut dirs: Vec<&String> = by_dir.keys().filter(|d| !d.is_empty()).collect();
    dirs.sort();
    if !dirs.is_empty() {
        root.push_str("\n## 分类\n\n");
        for dir in dirs {
            let count = by_dir.get(dir).map(|v| v.len()).unwrap_or(0);
            root.push_str(&format!("- [{dir}](./{dir}/{})（{count}）\n", super::paths::RESERVED_INDEX));
        }
    }
    if !root_docs.is_empty() {
        let mut list = root_docs;
        list.sort_by(|a, b| a.concept_id.cmp(&b.concept_id));
        root.push_str("\n## 未分类\n\n");
        root.push_str(&render_links(&list, "./"));
        root.push('\n');
    }
    atomic_write(&root_dir, &root)?;

    Ok(written)
}

/// 渲染概念链接列表：`- [标题](./slug.md) — 描述`。
fn render_links(docs: &[&ConceptDocument], prefix: &str) -> String {
    docs.iter()
        .map(|doc| {
            let slug = doc.slug();
            let title = doc
                .frontmatter
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(slug);
            match doc
                .frontmatter
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                Some(desc) => format!("- [{title}]({prefix}{slug}.md) — {desc}\n"),
                None => format!("- [{title}]({prefix}{slug}.md)\n"),
            }
        })
        .collect()
}

/// 找到 frontmatter 结束位置（闭合 `---` 之后的字节下标）。
fn frontmatter_end(content: &str) -> Option<usize> {
    let rest_start = if content.starts_with("\u{feff}") { 3 } else { 0 };
    let rest = &content[rest_start..];
    if !rest.starts_with("---") {
        return None;
    }
    let after = &rest[3..];
    for (idx, _) in after.match_indices("---") {
        let before_ok = idx > 0 && after.as_bytes()[idx - 1] == b'\n';
        let after_ok = after[idx + 3..]
            .chars()
            .next()
            .map(|c| c == '\n' || c == '\r')
            .unwrap_or(true);
        if before_ok && after_ok {
            return Some(rest_start + 3 + idx + 3);
        }
    }
    None
}

/// 把**无法解析的相对 `.md` 链接**降级为纯文本，使 Bundle 保持 OKF 合规（幂等）。
///
/// 为什么需要：编译器是**单次调用、无工具**的，它无法知道哪些概念真的存在，因此会自造
/// 跨概念链接（真实的第一次全量编译就出现了 `./approval-tool-call-id.md` 这种悬空链接）。
/// 若放任不修，`validate` 会一直报 Error。
///
/// 策略：能解析的链接**原样保留**（概念间关系照旧表达）；解析不到的只去掉链接语法、保留
/// 可见文字（信息不丢）。在**整轮编译末尾**统一执行，因此本轮稍后才创建的目标也能被解析到。
///
/// 返回 `(检查的概念数, 被降级的链接数)`。
pub fn repair_relative_links(paths: &BundlePaths) -> Result<(usize, usize), String> {
    use std::collections::HashSet;

    let files = paths.concept_files();
    let existing: HashSet<String> = files.iter().map(|(id, _)| id.clone()).collect();
    let mut checked = 0usize;
    let mut degraded = 0usize;

    for (_concept_id, path) in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(fm_end) = frontmatter_end(&content) else {
            continue;
        };
        checked += 1;
        let (fixed_body, n) = repair_body_links(&content[fm_end..], &path, &existing);
        if n > 0 {
            degraded += n;
            atomic_write(&path, &format!("{}{fixed_body}", &content[..fm_end]))?;
        }
    }
    Ok((checked, degraded))
}

/// 修复单份正文里的悬空相对链接：`[文字](./x.md)` → `文字`。
fn repair_body_links(
    body: &str,
    file_path: &std::path::Path,
    existing: &std::collections::HashSet<String>,
) -> (String, usize) {
    let base_dir = file_path.parent();
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    let mut degraded = 0usize;

    while let Some(rel) = body[cursor..].find("](") {
        let close_bracket = cursor + rel; // `]` 的位置
        let target_start = close_bracket + 2;
        let Some(paren_rel) = body[target_start..].find(')') else {
            break;
        };
        let target_end = target_start + paren_rel;
        let target = body[target_start..target_end].trim();
        // 链接文字：向前找最近的 `[`
        let text_start = body[..close_bracket].rfind('[');

        let keep = match (text_start, target) {
            (Some(_), t) if is_relative_md_link(t) => match base_dir {
                Some(dir) => {
                    let resolved = resolve_for_repair(dir, t);
                    resolved.is_file() || concept_id_of(&resolved).is_some_and(|id| existing.contains(&id))
                }
                None => false,
            },
            // 非相对 .md 链接（外链 / 锚点 / 绝对 / 图片等）一律保留
            _ => true,
        };

        if keep || text_start.is_none() {
            out.push_str(&body[cursor..target_end + 1]);
        } else {
            let ts = text_start.unwrap();
            out.push_str(&body[cursor..ts]);
            out.push_str(&body[ts + 1..close_bracket]); // 只保留链接文字
            degraded += 1;
        }
        cursor = target_end + 1;
    }
    out.push_str(&body[cursor..]);
    (out, degraded)
}

/// 是否为需要校验的相对 `.md` 链接。
fn is_relative_md_link(target: &str) -> bool {
    let path = target.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() || path.starts_with('/') || path.starts_with('#') {
        return false;
    }
    if !path.to_ascii_lowercase().ends_with(".md") {
        return false;
    }
    // 带 scheme（http: / data: …）的不算相对链接
    !match path.find(':') {
        Some(i) if i > 0 => path[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')),
        _ => false,
    }
}

/// 相对路径解析（处理 `.` / `..`）。
fn resolve_for_repair(base: &std::path::Path, rel: &str) -> std::path::PathBuf {
    let mut p = base.to_path_buf();
    for seg in rel.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                p.pop();
            }
            s => p.push(s),
        }
    }
    p
}

/// 由概念文件路径反推 Concept ID（相对 wiki/ 去 `.md`）。
fn concept_id_of(path: &std::path::Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let dir = path.parent()?.file_name()?.to_str()?;
    Some(format!("{dir}/{stem}"))
}

/* ---------- 「相关」小节（确定性生成，不靠模型猜） ---------- */

/// 小节标题。整段按此标题替换 ⇒ 重复编译不会堆叠。
pub const RELATED_HEADING: &str = "## 相关";

/// 一条相关概念链接。
#[derive(Debug, Clone, PartialEq)]
pub struct RelatedLink {
    pub concept_id: String,
    pub title: String,
    /// 与当前概念的共同标签（有则作为注解；没有就只给链接）
    pub shared_tags: Vec<String>,
}

/// 一条来源（原文）链接：展示标题 + 已算好的相对 href。
#[derive(Debug, Clone, PartialEq)]
pub struct RelatedSource {
    pub title: String,
    pub href: String,
}

/// 从 `wiki/<from_dir>/x.md` 指向 `to_id` 的**相对**链接（OKF 只允许相对链接）。
pub fn relative_concept_link(from_dir: &str, to_id: &str) -> String {
    let (to_dir, slug) = match to_id.rsplit_once('/') {
        Some((d, s)) => (d, s),
        None => ("", to_id),
    };
    if to_dir == from_dir {
        format!("./{slug}.md")
    } else if from_dir.is_empty() {
        format!("./{to_dir}/{slug}.md")
    } else {
        format!("../{to_dir}/{slug}.md")
    }
}

/// 从概念文件指向 raw 会话的相对链接（`raw/sessions/x.md` 是 bundle 相对路径）。
pub fn relative_raw_link(from_dir: &str, resource: &str) -> String {
    let depth = from_dir.split('/').filter(|s| !s.is_empty()).count();
    format!("{}{}", "../".repeat(depth + 1), resource)
}

/// 渲染「相关」小节（概念链接 + 原文链接）。两者都为空时返回空串（由调用方删除该小节）。
pub fn render_related_section(
    links: &[RelatedLink],
    from_dir: &str,
    sources: &[RelatedSource],
) -> String {
    if links.is_empty() && sources.is_empty() {
        return String::new();
    }
    let mut out = String::from(RELATED_HEADING);
    out.push_str("\n\n");
    for l in links {
        out.push_str(&format!(
            "- [{}]({})",
            l.title,
            relative_concept_link(from_dir, &l.concept_id)
        ));
        if !l.shared_tags.is_empty() {
            out.push_str(&format!(" — 共同标签：{}", l.shared_tags.join("、")));
        }
        out.push('\n');
    }
    for s in sources {
        out.push_str(&format!("- 原文：[{}]({})\n", s.title, s.href));
    }
    out
}

/// 去掉正文里已有的「相关」小节（标题 → 下一个 `## ` 或文末）。返回 `(剩余正文, 是否删过)`。
fn strip_related_section(body: &str) -> (String, bool) {    let mut out = String::with_capacity(body.len());
    let mut skipping = false;
    let mut removed = false;
    for line in body.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']);
        if skipping {
            // 小节内的内容直到下一个**同级或更高级**标题为止（`### 追加` 之类不会提前结束）
            if t.trim_start().starts_with("## ") {
                skipping = false;
                out.push_str(line);
            }
            continue;
        }
        if t.trim() == RELATED_HEADING || t.trim() == "## Related" {
            skipping = true;
            removed = true;
            continue;
        }
        out.push_str(line);
    }
    (out, removed)
}

/// 「相关」小节是**机器生成的派生内容**（每轮检索结果都可能变），判等时必须剔除。
///
/// 否则 `--force` 重跑会把每条概念都判成「内容有变化」→ 无意义 UPDATE + 追加段膨胀。
pub fn without_related_section(body: &str) -> String {
    strip_related_section(body).0
}

/// 用 `section` 替换「相关」小节并**放到正文末尾**（幂等：重复调用结果一致）。
///
/// 为什么放到末尾：这一节是**机器生成**的派生信息，位置固定才能让「重复编译」产生
/// 逐字相同的正文（否则每轮追加/挪动位置 → 触发无意义的 UPDATE 与来源标记堆积）。
pub fn replace_related_section(body: &str, section: &str) -> String {
    let (rest, _) = strip_related_section(body);
    let base = rest.trim_end();
    let section = section.trim_end();
    if section.is_empty() {
        return if base.is_empty() {
            String::new()
        } else {
            format!("{base}\n")
        };
    }
    if base.is_empty() {
        format!("{section}\n")
    } else {
        format!("{base}\n\n{section}\n")
    }
}

/// 追加变更历史（**倒序**：最新在前，插入到标题行之后）。
pub fn append_log(paths: &BundlePaths, entries: &[LogEntry]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let existing = read_log_raw(&paths.log_path());
    let mut lines: Vec<String> = vec![LOG_HEADER.to_string(), String::new()];
    lines.extend(entries.iter().map(LogEntry::to_line));
    if !existing.is_empty() {
        lines.push(String::new());
        lines.extend(existing);
    }
    lines.truncate(2 + MAX_LOG_ENTRIES);
    let mut content = lines.join("\n");
    content.push('\n');
    atomic_write(&paths.log_path(), &content)
}

/// 读取变更历史（最新在前），最多 `limit` 条。
pub fn read_log(paths: &BundlePaths, limit: usize) -> Vec<LogEntry> {
    read_log_raw(&paths.log_path())
        .into_iter()
        .filter_map(|line| LogEntry::parse_line(&line))
        .take(limit)
        .collect()
}

/// 读取 `log.md` 中的条目行（已去掉标题 / 空行 / 非条目行）。
fn read_log_raw(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim_end)
        .filter(|line| LogEntry::parse_line(line).is_some())
        .map(str::to_string)
        .collect()
}

/// 文件最后修改时间（unix 秒；取不到为 0）。
fn mtime_secs(path: &Path) -> i64 {
    use std::time::UNIX_EPOCH;
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 文件名是否为保留文件（`index.md` / `log.md`；供上层判定，避免把索引当概念处理）。
pub fn is_reserved(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(is_reserved_file_name)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concept::{ConceptFrontmatter, GeneratedInfo};

    fn tmp_bundle(tag: &str) -> (PathBuf, BundlePaths) {
        let root = std::env::temp_dir().join(format!("astorm_mem_writer_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = BundlePaths::new(&root);
        paths
            .ensure_dirs(&["decisions".into(), "concepts".into()])
            .unwrap();
        (root, paths)
    }

    fn doc(id: &str, title: &str, desc: &str) -> ConceptDocument {
        ConceptDocument::new(
            id,
            ConceptFrontmatter {
                concept_type: "Decision".into(),
                title: Some(title.into()),
                description: Some(desc.into()),
                generated: Some(GeneratedInfo {
                    by: "astorm-memory/0.1.0".into(),
                    at: "2026-09-25T10:00:00Z".into(),
                    model: None,
                }),
                ..Default::default()
            },
            "正文内容",
        )
    }

    #[test]
    fn write_concept_reports_created_then_updated() {
        let (root, paths) = tmp_bundle("write");
        let d = doc("decisions/use-jwt", "标题", "描述");
        assert!(!write_concept(&paths, &d).unwrap(), "首次为新建");
        assert!(paths.concept_path("decisions/use-jwt").exists());
        assert!(write_concept(&paths, &d).unwrap(), "再次为覆盖");
        // 无临时残渣
        assert!(!paths.wiki_dir().join("decisions/use-jwt.md.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 原子写不会读到半截内容：tmp 残留不影响正式文件的解析。
    #[test]
    fn atomic_write_leaves_no_readable_partial() {
        let (root, paths) = tmp_bundle("atomic");
        // 人为制造一个残留 tmp（模拟上次崩溃）
        std::fs::write(paths.wiki_dir().join("decisions/x.md.tmp"), "---\ntype: [坏").unwrap();
        write_concept(&paths, &doc("decisions/x", "真", "内容")).unwrap();

        let loaded = load_all_concepts(&paths);
        assert_eq!(loaded.concepts.len(), 1, "残留 tmp 不得被当作概念: {loaded:?}");
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.concepts[0].2.frontmatter.title.as_deref(), Some("真"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_all_concepts_reports_broken_without_aborting() {
        let (root, paths) = tmp_bundle("broken");
        write_concept(&paths, &doc("decisions/ok", "好", "")).unwrap();
        std::fs::write(paths.wiki_dir().join("decisions/broken.md"), "# 没有 frontmatter").unwrap();

        let loaded = load_all_concepts(&paths);
        assert_eq!(loaded.concepts.len(), 1);
        assert_eq!(loaded.errors.len(), 1);
        assert_eq!(loaded.errors[0].0, "decisions/broken");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rebuild_indexes_skips_reserved_and_sorts_alphabetically() {
        let (root, paths) = tmp_bundle("index");
        write_concept(&paths, &doc("decisions/zebra", "Z", "最后")).unwrap();
        write_concept(&paths, &doc("decisions/alpha", "A", "最前")).unwrap();
        write_concept(&paths, &doc("concepts/napi-dll", "N", "跨目录")).unwrap();

        rebuild_indexes(&paths).unwrap();

        let dir_index = std::fs::read_to_string(paths.index_path("decisions")).unwrap();
        let alpha_at = dir_index.find("alpha.md").unwrap();
        let zebra_at = dir_index.find("zebra.md").unwrap();
        assert!(alpha_at < zebra_at, "目录索引应按 concept_id 字母序: {dir_index}");
        assert!(!dir_index.contains("index.md"), "目录索引不得自引用: {dir_index}");
        assert!(!dir_index.contains("log.md"), "目录索引不得包含保留文件: {dir_index}");
        assert!(dir_index.contains("[A](./alpha.md) — 最前"), "{dir_index}");

        let root_index = std::fs::read_to_string(paths.root_index_path()).unwrap();
        assert!(root_index.starts_with("---\nokf_version: \"0.2\"\n---"), "{root_index}");
        assert!(root_index.contains("[decisions](./decisions/index.md)（2）"), "{root_index}");
        assert!(root_index.contains("[concepts](./concepts/index.md)（1）"), "{root_index}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 根索引按实际目录生成：手工放入的自定义分类目录也应出现（无白名单）。
    #[test]
    fn rebuild_indexes_picks_up_manually_added_dirs() {
        let (root, paths) = tmp_bundle("custom");
        write_concept(&paths, &doc("decisions/a", "A", "")).unwrap();
        let custom = paths.wiki_dir().join("runbooks");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(
            custom.join("rollback.md"),
            "---\ntype: Runbook\ntitle: 回滚步骤\n---\n逐条执行\n",
        )
        .unwrap();

        rebuild_indexes(&paths).unwrap();
        let root_index = std::fs::read_to_string(paths.root_index_path()).unwrap();
        assert!(root_index.contains("runbooks"), "自定义目录应出现在根索引: {root_index}");
        let custom_index = std::fs::read_to_string(paths.index_path("runbooks")).unwrap();
        assert!(custom_index.contains("[回滚步骤](./rollback.md)"), "{custom_index}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 根下散落概念（无分类目录）应出现在根索引的「未分类」段。
    #[test]
    fn rebuild_indexes_lists_loose_concepts() {
        let (root, paths) = tmp_bundle("loose");
        std::fs::write(
            paths.wiki_dir().join("loose-note.md"),
            "---\ntype: Note\ntitle: 散落\n---\n正文\n",
        )
        .unwrap();
        rebuild_indexes(&paths).unwrap();
        let root_index = std::fs::read_to_string(paths.root_index_path()).unwrap();
        assert!(root_index.contains("## 未分类"), "{root_index}");
        assert!(root_index.contains("[散落](./loose-note.md)"), "{root_index}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn log_is_prepended_newest_first_and_bounded() {
        let (root, paths) = tmp_bundle("log");
        let first = LogEntry {
            at: "2026-09-25T10:00:00Z".into(),
            action: "CREATE".into(),
            concept_id: "decisions/a".into(),
            resource: "raw/sessions/2026-09-25-aaaa.md".into(),
            reason: String::new(),
        };
        let second = LogEntry {
            at: "2026-09-26T11:00:00Z".into(),
            action: "UPDATE".into(),
            concept_id: "decisions/b".into(),
            resource: "raw/sessions/2026-09-26-bbbb.md".into(),
            reason: String::new(),
        };
        append_log(&paths, &[first.clone()]).unwrap();
        append_log(&paths, &[second.clone()]).unwrap();

        let entries = read_log(&paths, 10);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].concept_id, "decisions/b", "最新应在前");
        assert_eq!(entries[1].concept_id, "decisions/a");

        let raw = std::fs::read_to_string(paths.log_path()).unwrap();
        assert!(raw.starts_with(LOG_HEADER), "标题行应在最前: {raw}");
        // 空追加不改变文件
        let before = raw.clone();
        append_log(&paths, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(paths.log_path()).unwrap(), before);

        // 反复追加不丢最新
        for i in 0..5 {
            append_log(
                &paths,
                &[LogEntry {
                    at: format!("2026-09-27T0{i}:00:00Z"),
                    action: "CREATE".into(),
                    concept_id: format!("decisions/n{i}"),
                    resource: "raw/sessions/x.md".into(),
                    reason: String::new(),
                }],
            )
            .unwrap();
        }
        assert_eq!(read_log(&paths, 100).len(), 7);
        assert_eq!(read_log(&paths, 1).len(), 1);
        assert_eq!(read_log(&paths, 1)[0].concept_id, "decisions/n4");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_log_on_missing_file_is_empty() {
        let (root, paths) = tmp_bundle("nolog");
        assert!(read_log(&paths, 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 链接修复：悬空相对链接降级、可解析/外链/锚点/图片保留、幂等。
    #[test]
    fn repair_relative_links_degrades_only_dangling_md_links() {
        let (root, paths) = tmp_bundle("repair");
        std::fs::write(
            paths.wiki_dir().join("decisions/target.md"),
            "---\ntype: Decision\ntitle: 目标\n---\n\n正文\n",
        )
        .unwrap();
        std::fs::write(
            paths.wiki_dir().join("decisions/src.md"),
            "---\ntype: Decision\ntitle: 源\n---\n\n\n见 [目标](./target.md#frag)、[跨目录](../concepts/x.md)、[没有](./ghost.md)、[外链](https://a.b/c.md)、[锚点](#s)、![图](./img.png)、[绝对](/decisions/target.md)。\n",
        )
        .unwrap();

        let (checked, degraded) = repair_relative_links(&paths).unwrap();
        assert_eq!(checked, 2);
        assert_eq!(degraded, 2, "ghost 与跨目录 x 都应降级");

        let body = std::fs::read_to_string(paths.wiki_dir().join("decisions/src.md")).unwrap();
        assert!(body.contains("[目标](./target.md#frag)"), "可解析（带锚点）应保留: {body}");
        assert!(!body.contains("ghost.md"), "{body}");
        assert!(body.contains("没有"), "链接文字保留: {body}");
        assert!(!body.contains("../concepts/x.md"), "跨目录悬空也应降级: {body}");
        assert!(body.contains("跨目录"), "{body}");
        assert!(body.contains("[外链](https://a.b/c.md)"), "外链不动: {body}");
        assert!(body.contains("[锚点](#s)"), "{body}");
        assert!(body.contains("![图](./img.png)"), "图片是资源不是 .md 链接，不动: {body}");
        assert!(body.contains("[绝对](/decisions/target.md)"), "{body}");
        // frontmatter 原样保留（只改正文）
        assert!(body.starts_with("---\ntype: Decision\ntitle: 源\n---\n"), "{body}");

        // 幂等
        let (_, again) = repair_relative_links(&paths).unwrap();
        assert_eq!(again, 0, "修复应幂等");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn repair_skips_files_without_frontmatter() {
        let (root, paths) = tmp_bundle("repair2");
        std::fs::write(paths.wiki_dir().join("decisions/broken.md"), "没有 frontmatter
").unwrap();
        let (checked, degraded) = repair_relative_links(&paths).unwrap();
        assert_eq!(checked, 0, "无法定位 frontmatter 的文件应跳过");
        assert_eq!(degraded, 0);
        assert_eq!(
            std::fs::read_to_string(paths.wiki_dir().join("decisions/broken.md")).unwrap(),
            "没有 frontmatter
",
            "不得改动"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn frontmatter_end_locates_body() {
        let content = "---\ntype: Fact\n---\n\n正文\n";
        let end = frontmatter_end(content).unwrap();
        assert_eq!(&content[end..], "\n\n正文\n");
        assert!(frontmatter_end("# 无 frontmatter").is_none());
    }

    #[test]
    fn relative_md_link_detection() {
        assert!(is_relative_md_link("./a.md"));
        assert!(is_relative_md_link("../b/c.md#x"));
        assert!(!is_relative_md_link("https://a/b.md"));
        assert!(!is_relative_md_link("/abs.md"));
        assert!(!is_relative_md_link("#anchor"));
        assert!(!is_relative_md_link("./img.png"));
        assert!(!is_relative_md_link(""));
    }

    #[test]
    fn remove_concept_is_idempotent() {
        let (root, paths) = tmp_bundle("remove");
        write_concept(&paths, &doc("decisions/gone", "x", "")).unwrap();
        remove_concept(&paths, "decisions/gone").unwrap();
        assert!(!paths.concept_path("decisions/gone").exists());
        remove_concept(&paths, "decisions/gone").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_reports_updated_at_and_excludes_reserved() {
        let (root, paths) = tmp_bundle("scan");
        write_concept(&paths, &doc("decisions/a", "A", "")).unwrap();
        std::fs::write(paths.index_path("decisions"), "# decisions\n").unwrap();
        std::fs::write(paths.log_path(), "# 变更历史\n").unwrap();

        let scanned = scan_concepts(&paths);
        assert_eq!(scanned.len(), 1, "{scanned:?}");
        assert_eq!(scanned[0].concept_id, "decisions/a");
        assert!(scanned[0].updated_at > 0, "应能取到最后修改时间");
        assert!(is_reserved(&paths.index_path("decisions")));
        assert!(is_reserved(&paths.log_path()));
        assert!(!is_reserved(&paths.concept_path("decisions/a")));

        let _ = std::fs::remove_dir_all(&root);
    }

    /* ---------- 「相关」小节 ---------- */

    #[test]
    fn relative_links_are_bundle_relative() {
        // 同目录 → ./x.md；跨目录 → ../dir/x.md；wiki 根 → ./dir/x.md，根到根 → ./x.md
        assert_eq!(relative_concept_link("decisions", "decisions/use-jwt"), "./use-jwt.md");
        assert_eq!(relative_concept_link("decisions", "concepts/auth"), "../concepts/auth.md");
        assert_eq!(relative_concept_link("", "concepts/auth"), "./concepts/auth.md");
        assert_eq!(relative_concept_link("", "loose"), "./loose.md");
        // raw 在 wiki 之外：从 wiki/<dir>/ 出发要退两级
        assert_eq!(
            relative_raw_link("decisions", "raw/sessions/2025-10-09-s.md"),
            "../../raw/sessions/2025-10-09-s.md"
        );
        assert_eq!(
            relative_raw_link("", "raw/sessions/2025-10-09-s.md"),
            "../raw/sessions/2025-10-09-s.md"
        );
    }

    #[test]
    fn renders_related_section_with_optional_shared_tags() {
        let links = vec![
            RelatedLink {
                concept_id: "decisions/auth".into(),
                title: "认证方案".into(),
                shared_tags: vec!["auth".into(), "jwt".into()],
            },
            RelatedLink {
                concept_id: "concepts/other".into(),
                title: "另一条".into(),
                shared_tags: Vec::new(),
            },
        ];
        let sources = vec![RelatedSource {
            title: "2026-09-20 会话".into(),
            href: "../../raw/sessions/2026-09-20-s.md".into(),
        }];
        let s = render_related_section(&links, "decisions", &sources);
        assert!(s.starts_with("## 相关\n\n"), "{s}");
        assert!(s.contains("- [认证方案](./auth.md) — 共同标签：auth、jwt"), "{s}");
        assert!(s.contains("- [另一条](../concepts/other.md)"), "{s}");
        assert!(!s.contains("另一条](../concepts/other.md) —"), "无共同标签时不加破折号注解: {s}");
        assert!(s.contains("- 原文：[2026-09-20 会话](../../raw/sessions/2026-09-20-s.md)"), "{s}");

        // 两者皆空 → 空串（调用方据此删除小节）
        assert_eq!(render_related_section(&[], "decisions", &[]), "");
        // 只有原文也要出小节
        assert!(render_related_section(&[], "decisions", &sources).contains("原文："));
    }

    #[test]
    fn replace_related_section_is_idempotent_and_moves_to_tail() {
        let section = "## 相关\n\n- [甲](./a.md)\n";
        // 首次插入
        let once = replace_related_section("正文内容。", section);
        assert_eq!(once, "正文内容。\n\n## 相关\n\n- [甲](./a.md)\n");
        // 幂等：再替换一次逐字相同
        assert_eq!(replace_related_section(&once, section), once);

        // 旧小节被替换（不是追加），且小节之后的内容不丢、整体重排到末尾
        let messy = "正文。\n\n## 相关\n\n- [旧](./old.md)\n\n## 后面还有一节\n\n内容。\n";
        let fixed = replace_related_section(messy, section);
        assert!(!fixed.contains("old.md"), "{fixed}");
        assert!(fixed.contains("## 后面还有一节"), "小节后的正文不能丢: {fixed}");
        assert!(fixed.trim_end().ends_with("- [甲](./a.md)"), "小节应落在末尾: {fixed}");
        assert_eq!(fixed.matches("## 相关").count(), 1, "{fixed}");

        // 空 section ⇒ 删除小节
        let removed = replace_related_section(&once, "");
        assert_eq!(removed, "正文内容。\n");
    }

    #[test]
    fn without_related_section_strips_only_that_section() {
        let body = "正文。\n\n## 相关\n\n- [甲](./a.md)\n\n## 另一节\n\n保留。\n";
        let bare = without_related_section(body);
        assert!(!bare.contains("## 相关"));
        assert!(bare.contains("## 另一节") && bare.contains("保留。"), "{bare}");
        // 没有该小节时原样返回
        assert_eq!(without_related_section("只有正文。\n"), "只有正文。\n");
    }
}

//! OKF 概念文档：frontmatter 结构、`---` 块切分与往返、Concept ID 归一化。
//!
//! Concept 文档 = YAML frontmatter（`---` 围栏）+ 自由 Markdown 正文。
//! **Concept ID** = 相对 `wiki/` 去掉 `.md` 的路径（如 `decisions/use-jwt`）。
//!
//! 目录前缀**由分类表权威决定**（[`build_concept_id`]）：模型返回的 `concept_id` 只取其
//! 末段作为 slug，目录一律按 `category.dir` 重新拼装 —— 消除「模型把 Decision 写进
//! concepts/」这一整类错误，并让 `type` 与目录始终自洽。
//!
//! frontmatter 切分沿用 `skill/index.rs::parse_skill_content` 的约定（`\n---` 定界），
//! 保持全仓一致。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::paths::is_reserved_file_name;
use crate::config::MemoryCategory;

/// frontmatter 来源条目（OKF v0.2 provenance：`sources`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    /// 来源资源（相对 bundle 根，如 `raw/sessions/2026-09-25-a1f9.md`）。
    pub resource: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_count: Option<u32>,
    /// 来源最后修改时间（RFC3339；合并去重时保留最新）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// 生成信息（OKF v0.2 trust：`generated`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedInfo {
    /// 生产者标识（如 `astorm-memory/0.1.0`）。
    pub by: String,
    /// 生成时间（RFC3339）。
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// OKF 概念 frontmatter。
///
/// 字段顺序即 YAML 落盘顺序（可读性）：必填 `type` → 推荐字段 → v0.2 信任字段 →
/// 生产者扩展（`observed_at` + [`Self::extra`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConceptFrontmatter {
    /// **OKF 唯一必填字段**（非空字符串；描述性、非枚举）。
    #[serde(rename = "type", default)]
    pub concept_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// v0.2 lifecycle。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// v0.2 freshness。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after: Option<String>,
    /// v0.2 provenance。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SourceEntry>>,
    /// v0.2 trust（由调用方填写，模型不产出）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<GeneratedInfo>,
    /// 生产者扩展：该记忆最早被观察到的日期（ISO `YYYY-MM-DD`，由 `frag_ids` 推导）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// 生产者扩展：支撑该记忆的**消息锚点**（指向 `raw/sessions/*.md` 里的 `### [mNN]`）。
    ///
    /// 这是**权威**依据集（`observed_at` 即由它推导），也是「跳回原文」的唯一可靠依据：
    /// 正文里模型自述的「依据：mNN」是自由文本，不能作为可点锚点的来源。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frag_ids: Vec<String>,
    /// 其他生产者扩展字段（原样保留，避免读写往返丢字段）。
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl Default for ConceptFrontmatter {
    fn default() -> Self {
        Self {
            concept_type: String::new(),
            title: None,
            description: None,
            tags: Vec::new(),
            status: None,
            stale_after: None,
            sources: None,
            generated: None,
            observed_at: None,
            frag_ids: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// 一份 OKF 概念文档（frontmatter + 正文）。
#[derive(Debug, Clone, PartialEq)]
pub struct ConceptDocument {
    /// Concept ID（相对 `wiki/` 去 `.md`）；[`Self::from_markdown`] 由调用方给定。
    pub concept_id: String,
    pub frontmatter: ConceptFrontmatter,
    pub body: String,
}

impl ConceptDocument {
    /// 新建（正文去首尾空白）。
    pub fn new(concept_id: impl Into<String>, frontmatter: ConceptFrontmatter, body: impl Into<String>) -> Self {
        Self {
            concept_id: concept_id.into(),
            frontmatter,
            body: body.into().trim().to_string(),
        }
    }

    /// 分类目录名（Concept ID 的首段；无 `/` 表示概念直接位于 `wiki/` 根）。
    pub fn dir(&self) -> String {
        self.concept_id
            .split_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_default()
    }

    /// slug（Concept ID 末段）。
    pub fn slug(&self) -> &str {
        self.concept_id.rsplit('/').next().unwrap_or(&self.concept_id)
    }

    /// 序列化为 Markdown 文本（`---` + YAML + `---` + 空行 + 正文）。
    pub fn to_markdown(&self) -> Result<String, String> {
        let yaml = serde_yaml::to_string(&self.frontmatter)
            .map_err(|e| format!("序列化 frontmatter 失败({}): {e}", self.concept_id))?;
        let body = self.body.trim_end();
        Ok(format!("---\n{yaml}---\n\n{body}\n"))
    }

    /// 解析 Markdown 文本（`concept_id` 由调用方按文件路径给定）。
    pub fn from_markdown(concept_id: &str, content: &str) -> Result<Self, String> {
        let (yaml, body) = split_frontmatter(content)
            .ok_or_else(|| format!("概念 {concept_id} 缺少合法的 frontmatter（须以 --- 开头且有闭合 ---）"))?;
        let frontmatter: ConceptFrontmatter = serde_yaml::from_str(&yaml)
            .map_err(|e| format!("概念 {concept_id} 的 frontmatter 解析失败: {e}"))?;
        Ok(Self {
            concept_id: concept_id.to_string(),
            frontmatter,
            body: body.trim().to_string(),
        })
    }

    /// 校验 frontmatter 是否满足 OKF 最低要求（`type` 非空、ID 非保留名）。
    ///
    /// 返回 `Err` 表示**不得写入 Bundle**（方案文档 §5.2：缺 `type` 的产出标记为无效）。
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.concept_id.trim().is_empty() {
            return Err("Concept ID 为空".into());
        }
        if self.frontmatter.concept_type.trim().is_empty() {
            return Err(format!("概念 {} 缺少必填字段 type（非空字符串）", self.concept_id));
        }
        let file_name = format!("{}.md", self.slug());
        if is_reserved_file_name(&file_name) {
            return Err(format!("概念 {} 的 slug 命中了 OKF 保留文件名", self.concept_id));
        }
        Ok(())
    }
}

/// 切分 frontmatter：返回 `(yaml 原文, 正文)`；不合法（无 `---` 开头或无闭合）返回 None。
///
/// 与 `skill/index.rs` 同约定（去 BOM、`\n---` 定界）；额外容忍 CRLF。
pub fn split_frontmatter(content: &str) -> Option<(String, String)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    // 首个独占一行的 `---` 作为闭合（跳过紧邻的换行）
    let mut sep: Option<(usize, usize)> = None;
    for (idx, _) in rest.match_indices("---") {
        let before_ok = idx > 0 && rest.as_bytes()[idx - 1] == b'\n';
        let after_ok = rest[idx + 3..]
            .chars()
            .next()
            .map(|c| c == '\n' || c == '\r')
            .unwrap_or(true);
        if before_ok && after_ok {
            // 闭合标记后若有 \r\n 需一并吃掉，故 +3 之后交给 trim 处理
            sep = Some((idx, 3));
            break;
        }
    }
    let (sep_idx, _) = sep?;
    // CRLF 归一为 LF：serde_yaml 对块首换行 + CRLF 的容忍度不一致，先统一更稳
    let yaml = rest[..sep_idx].replace("\r\n", "\n");
    let body = rest[sep_idx + 3..].to_string();
    Some((yaml, body))
}

/// slug 最大字符数（留足文件系统字节余量：60 个 4 字节字符 + 目录 + `.md` 仍远低于 255 字节）。
const MAX_SLUG_CHARS: usize = 60;
/// slug 为空时的确定性回退长度（sha256 十六进制前 N 位）。
const SLUG_HASH_LEN: usize = 12;

/// 抹平正文里的**相对 `.md` 链接语法**（只留链接文字），保留其它链接不变。
///
/// 用途：**比较用**。悬空链接修复后处理会改动正文，于是「磁盘内容 ≠ LLM 原始输出」成为常态；
/// 若直接逐字比较，`--force` 重跑会被判为「有变化」而重复追加，wiki 会无限膨胀。
/// 链接的有无不构成「新信息」，故比较时两侧都抹平。
pub fn strip_relative_md_links(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    while let Some(rel) = body[cursor..].find("](") {
        let close_bracket = cursor + rel;
        let target_start = close_bracket + 2;
        let Some(paren_rel) = body[target_start..].find(')') else {
            break;
        };
        let target_end = target_start + paren_rel;
        let target = body[target_start..target_end].trim();
        let is_rel_md = {
            let path = target.split(['#', '?']).next().unwrap_or("");
            !path.is_empty()
                && !path.starts_with('/')
                && !path.starts_with('#')
                && path.to_ascii_lowercase().ends_with(".md")
                && !matches!(path.find(':'), Some(i) if i > 0 && path[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
        };
        match (is_rel_md, body[..close_bracket].rfind('[')) {
            (true, Some(ts)) => {
                out.push_str(&body[cursor..ts]);
                out.push_str(&body[ts + 1..close_bracket]); // 只留文字
            }
            _ => out.push_str(&body[cursor..target_end + 1]),
        }
        cursor = target_end + 1;
    }
    out.push_str(&body[cursor..]);
    out
}

/// 由分类 + 模型给出的原始 `concept_id` 构建权威 Concept ID。
///
/// 规则：
/// - 取原始值**末段**作为 slug（丢弃模型多给的目录前缀）
/// - slug 清洗：去 `.md`、非法文件名字符 → `-`、空白 → `-`、ASCII 小写、折叠/去首尾 `-`
/// - slug 为空 → 以 `fallback_source`（一般传 `event_text`）的 sha256 前 12 位兜底
/// - slug 命中 OKF 保留名（`index` / `log`）→ 追加 `-concept`
/// - 目录前缀 = `category.dir`（空则概念直接位于 `wiki/` 根）
pub fn build_concept_id(category: &MemoryCategory, raw_concept_id: &str, fallback_source: &str) -> String {
    let slug = sanitize_slug(raw_concept_id, fallback_source);
    let dir = category.dir.trim().trim_matches('/');
    if dir.is_empty() {
        slug
    } else {
        format!("{dir}/{slug}")
    }
}

/// 原始路径 → 清洗后的 slug（见 [`build_concept_id`] 规则）。
pub fn sanitize_slug(raw: &str, fallback_source: &str) -> String {
    // ① 取末段：丢弃模型多给的目录（目录由分类权威决定）
    let last_seg = raw
        .replace('\\', "/")
        .split('/')
        .rev()
        .map(str::trim)
        .find(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .unwrap_or("")
        .to_string();

    // ② 去 .md 后缀
    let stem = strip_md_suffix(&last_seg);

    // ③ 字符清洗
    let mut out = String::with_capacity(stem.len());
    let mut last_was_dash = false;
    for ch in stem.chars() {
        let mapped = if ch.is_whitespace() || is_slug_separator(ch) || ch.is_control() {
            '-'
        } else {
            // ASCII 统一小写（CJK 原样保留）
            ch.to_ascii_lowercase()
        };
        if mapped == '-' {
            if last_was_dash || out.is_empty() {
                continue; // 折叠连续 `-`，且不留首部 `-`
            }
            last_was_dash = true;
            out.push('-');
        } else {
            last_was_dash = false;
            out.push(mapped);
        }
    }
    // 首尾的 `-` / `.` 都清掉：首部 `.` 会让文件变成隐藏文件（`paths.rs` 扫描时跳过）
    let mut slug = out.trim_matches(|c| c == '-' || c == '.').to_string();

    // ④ 空 slug → 确定性哈希兜底（保证同一声明每次得到同一 ID）
    if slug.is_empty() {
        slug = format!("mem-{}", short_hash(fallback_source));
    }

    // ⑤ 长度上限（按字符截断，保证 UTF-8 边界安全）
    if slug.chars().count() > MAX_SLUG_CHARS {
        slug = slug.chars().take(MAX_SLUG_CHARS).collect::<String>();
        slug = slug.trim_end_matches(['-', '.']).to_string();
        if slug.is_empty() {
            slug = format!("mem-{}", short_hash(fallback_source));
        }
    }

    // ⑥ OKF 保留文件名避让
    if is_reserved_file_name(&format!("{slug}.md")) {
        slug = format!("{slug}-concept");
    }
    slug
}

/// 概念 ID 冲突时追加 `-2` / `-3` …（`exists` 判定是否已占用）。
pub fn dedupe_concept_id(base: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(base) {
        return base.to_string();
    }
    for n in 2..10_000u32 {
        let candidate = format!("{base}-{n}");
        if !exists(&candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", short_hash(base))
}

/// slug 分隔符：文件名非法字符（Windows 保留集 + 路径分隔符）**以及**中日韩全角标点与
/// 全角空格。
///
/// 中文 LLM 产出几乎必然带全角标点（如 `决策：使用 JWT`），若不归一，slug 会变成
/// `决策：使用-jwt` 这种既难看又在 Markdown 链接里易出问题的形态。
fn is_slug_separator(ch: char) -> bool {
    matches!(
        ch,
        '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\'
            | '：' | '。' | '，' | '、' | '；' | '！' | '？'
            | '（' | '）' | '【' | '】' | '「' | '」' | '『' | '』'
            | '《' | '》' | '〈' | '〉' | '“' | '”' | '‘' | '’'
            | '…' | '—' | '～' | '·' | '　'
    )
}

/// 去 `.md` 后缀（大小写不敏感）。
///
/// **必须按字符回退、不能按字节切片**：`s[s.len()-3..]` 在全中文 slug（如
/// `拦截目标路径-a`）上会切到 UTF-8 字符中间而 **panic**（真实 LLM 产出立刻触发过）。
fn strip_md_suffix(s: &str) -> &str {
    let mut tail = s.char_indices().rev();
    let (_, last) = match tail.next() {
        Some(v) => v,
        None => return s,
    };
    let (_, mid) = match tail.next() {
        Some(v) => v,
        None => return s,
    };
    let (dot_idx, dot) = match tail.next() {
        Some(v) => v,
        None => return s,
    };
    if dot == '.' && mid.eq_ignore_ascii_case(&'m') && last.eq_ignore_ascii_case(&'d') {
        &s[..dot_idx]
    } else {
        s
    }
}

/// 字符串的 sha256 十六进制前缀（确定性短 ID）。
pub fn short_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let hex = format!("{:x}", hasher.finalize());
    hex.chars().take(SLUG_HASH_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryConfig;

    fn cat(id: &str) -> MemoryCategory {
        MemoryConfig::default_categories()
            .into_iter()
            .find(|c| c.id == id)
            .expect("默认分类存在")
    }

    fn doc(id: &str) -> ConceptDocument {
        ConceptDocument::new(
            id,
            ConceptFrontmatter {
                concept_type: "Decision".into(),
                title: Some("用 JWT 而不是 session".into()),
                description: Some("认证方案选型".into()),
                tags: vec!["auth".into(), "backend".into()],
                observed_at: Some("2026-09-25".into()),
                sources: Some(vec![SourceEntry {
                    resource: "raw/sessions/2026-09-25-a1f9.md".into(),
                    title: Some("认证方案讨论".into()),
                    author: None,
                    usage_count: None,
                    last_modified: Some("2026-09-25T10:00:00Z".into()),
                }]),
                generated: Some(GeneratedInfo {
                    by: "astorm-memory/0.1.0".into(),
                    at: "2026-09-25T10:05:00Z".into(),
                    model: Some("deepseek-chat".into()),
                }),
                ..Default::default()
            },
            "选 JWT，因为要支持多端无状态鉴权。\n\n见 [相关决策](../decisions/use-jwt.md)。",
        )
    }

    /// 往返：序列化 → 解析 → 全字段一致（含 unknown 字段保留）。
    #[test]
    fn markdown_round_trip_preserves_everything() {
        let original = doc("decisions/use-jwt");
        let text = original.to_markdown().unwrap();
        assert!(text.starts_with("---\n"), "必须以 --- 开头: {text}");
        assert!(text.contains("\ntype: Decision\n"), "type 必填且在前: {text}");
        assert!(text.contains("\n---\n\n选 JWT"), "frontmatter 与正文之间应有空行: {text}");

        let parsed = ConceptDocument::from_markdown("decisions/use-jwt", &text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn round_trip_keeps_unknown_frontmatter_fields() {
        let text = "\
---
type: Fact
title: 后端是 napi dll
custom_field: 保留我
nested:
  a: 1
---

正文
";
        let parsed = ConceptDocument::from_markdown("concepts/napi-dll", text).unwrap();
        assert_eq!(parsed.frontmatter.concept_type, "Fact");
        assert_eq!(
            parsed.frontmatter.extra.get("custom_field"),
            Some(&serde_yaml::Value::String("保留我".into()))
        );
        assert!(parsed.frontmatter.extra.contains_key("nested"));

        // 再序列化后未知字段仍在
        let again = parsed.to_markdown().unwrap();
        assert!(again.contains("custom_field: 保留我"), "{again}");
    }

    #[test]
    fn from_markdown_rejects_broken_input() {
        // 无 frontmatter
        assert!(ConceptDocument::from_markdown("a/b", "# 只有正文").is_err());
        // 未闭合
        assert!(ConceptDocument::from_markdown("a/b", "---\ntype: Fact\n正文").is_err());
        // YAML 非法
        assert!(ConceptDocument::from_markdown("a/b", "---\ntype: [broken\n---\nbody").is_err());
    }

    #[test]
    fn empty_body_is_allowed() {
        let d = ConceptDocument::new(
            "concepts/x",
            ConceptFrontmatter {
                concept_type: "Fact".into(),
                ..Default::default()
            },
            "   ",
        );
        let text = d.to_markdown().unwrap();
        let back = ConceptDocument::from_markdown("concepts/x", &text).unwrap();
        assert_eq!(back.body, "");
    }

    #[test]
    fn validate_shape_requires_non_empty_type() {
        let ok = doc("decisions/use-jwt");
        assert!(ok.validate_shape().is_ok());

        let mut missing = doc("decisions/use-jwt");
        missing.frontmatter.concept_type = "   ".into();
        assert!(missing.validate_shape().unwrap_err().contains("type"));

        // slug 命中保留名 → 拒绝写入（index.md / log.md 不得作为 Concept）
        let reserved = doc("concepts/index");
        assert!(reserved.validate_shape().unwrap_err().contains("保留文件名"));
        let empty_id = doc("");
        assert!(empty_id.validate_shape().is_err());
    }

    #[test]
    fn strip_relative_md_links_keeps_text_and_other_links() {
        let body = "见 [乙](./b.md) 与 [没有](./ghost.md#x)、[外链](https://a/b.md)、[锚点](#s)、![图](./i.png)。";
        let stripped = strip_relative_md_links(body);
        assert_eq!(stripped, "见 乙 与 没有、[外链](https://a/b.md)、[锚点](#s)、![图](./i.png)。");
        // 幂等
        assert_eq!(strip_relative_md_links(&stripped), stripped);
    }

    #[test]
    fn dir_and_slug_derive_from_concept_id() {
        let d = doc("decisions/use-jwt");
        assert_eq!(d.dir(), "decisions");
        assert_eq!(d.slug(), "use-jwt");
        // wiki 根下的概念：无目录
        let flat = doc("loose-note");
        assert_eq!(flat.dir(), "");
        assert_eq!(flat.slug(), "loose-note");
    }

    /// 目录前缀由分类权威决定，模型返回的目录被丢弃。
    #[test]
    fn build_concept_id_discards_model_supplied_dir() {
        assert_eq!(
            build_concept_id(&cat("decision"), "decisions/use-jwt", "ignored"),
            "decisions/use-jwt"
        );
        // 模型把 Decision 写进 concepts/ → 仍落到 decisions/
        assert_eq!(
            build_concept_id(&cat("decision"), "concepts/use-jwt", "ignored"),
            "decisions/use-jwt"
        );
        // 无目录前缀同样可接受
        assert_eq!(build_concept_id(&cat("decision"), "use-jwt", "ignored"), "decisions/use-jwt");
        // 多级目录只取末段
        assert_eq!(
            build_concept_id(&cat("fact"), "a/b/c/some-fact", "ignored"),
            "concepts/some-fact"
        );
    }

    #[test]
    fn build_concept_id_puts_root_dir_concepts_at_wiki_root() {
        let root_cat = MemoryCategory {
            id: "misc".into(),
            dir: String::new(),
            type_name: "Misc".into(),
            hint: String::new(),
        };
        assert_eq!(build_concept_id(&root_cat, "random/thing", "x"), "thing");
    }

    #[test]
    fn sanitize_slug_normalizes_chars() {
        // 空白 → `-`，折叠连续，去首尾
        assert_eq!(sanitize_slug("  Use  JWT  ", "x"), "use-jwt");
        assert_eq!(sanitize_slug("a---b", "x"), "a-b");
        // ASCII 大写 → 小写
        assert_eq!(sanitize_slug("USe-JWT", "x"), "use-jwt");
        // 非法文件名字符 → `-`
        assert_eq!(sanitize_slug("a:b*c?d", "x"), "a-b-c-d");
        // 全角/CJK 标点同样归一为分隔符
        assert_eq!(sanitize_slug("决策：使用 JWT", "x"), "决策-使用-jwt");
        assert_eq!(sanitize_slug("为什么不选 session？", "x"), "为什么不选-session");
        assert_eq!(sanitize_slug("《规范》摘要", "x"), "规范-摘要");
        // 去 .md 后缀（大小写不敏感）
        assert_eq!(sanitize_slug("use-jwt.MD", "x"), "use-jwt");
        // 首尾 `.` 必须清掉（否则会变成隐藏文件）
        assert_eq!(sanitize_slug(".hidden.", "x"), "hidden");
        // `..` / `.` 段被跳过，取真末段
        assert_eq!(sanitize_slug("a/../b", "x"), "b");
    }

    /// 中文原字保留（OKF 未限制 ID 字符集，macOS/Windows 文件名均支持）。
    /// 回归：中文结尾的 slug 不得 panic（曾因 `s[s.len()-3..]` 按字节切片而在真实
    /// LLM 产出 `拦截目标路径-a` 上崩溃）。
    #[test]
    fn strip_md_suffix_is_utf8_safe() {
        assert_eq!(strip_md_suffix("拦截目标路径-a"), "拦截目标路径-a");
        assert_eq!(strip_md_suffix("中文名.md"), "中文名");
        assert_eq!(strip_md_suffix("中文名.MD"), "中文名");
        assert_eq!(strip_md_suffix("a.md"), "a");
        assert_eq!(strip_md_suffix("名"), "名");
        assert_eq!(strip_md_suffix("ab"), "ab");
        assert_eq!(strip_md_suffix(""), "");
        assert_eq!(strip_md_suffix(".md"), "");
        // 真后缀在中文之后仍要正确剥离
        assert_eq!(strip_md_suffix("认证方案.md"), "认证方案");
    }

    /// 全中文输入走完整 slug 流程（真实 LLM 常见产出）。
    #[test]
    fn sanitize_slug_handles_pure_cjk_and_mixed() {
        assert_eq!(sanitize_slug("拦截目标路径-a", "x"), "拦截目标路径-a");
        assert_eq!(sanitize_slug("拦截目标路径.md", "x"), "拦截目标路径");
        assert_eq!(sanitize_slug("决策：使用 JWT 而非 session", "x"), "决策-使用-jwt-而非-session");
        assert_eq!(sanitize_slug("认证方案（最终版）", "x"), "认证方案-最终版");
        assert_eq!(build_concept_id(&cat("decision"), "拦截目标路径-a", "x"), "decisions/拦截目标路径-a");
    }

    #[test]
    fn sanitize_slug_keeps_cjk() {
        assert_eq!(sanitize_slug("认证方案选型", "x"), "认证方案选型");
        assert_eq!(sanitize_slug("认证 方案 use JWT", "x"), "认证-方案-use-jwt");
    }

    #[test]
    fn sanitize_slug_falls_back_to_deterministic_hash() {
        let a = sanitize_slug("///", "选了 JWT 而不是 session");
        let b = sanitize_slug("", "选了 JWT 而不是 session");
        assert!(a.starts_with("mem-"), "{a}");
        assert_eq!(a, b, "同一来源必须得到同一 ID（幂等）");
        // 不同来源 → 不同 ID
        assert_ne!(a, sanitize_slug("", "另一条声明"));
        assert_eq!(a.len(), 4 + SLUG_HASH_LEN);
    }

    #[test]
    fn sanitize_slug_avoids_reserved_file_names() {
        assert_eq!(sanitize_slug("index", "x"), "index-concept");
        assert_eq!(sanitize_slug("LOG", "x"), "log-concept");
        // 近名不误伤
        assert_eq!(sanitize_slug("indexes", "x"), "indexes");
        assert_eq!(sanitize_slug("my-log", "x"), "my-log");
    }

    #[test]
    fn sanitize_slug_caps_length_on_char_boundary() {
        let long = "很".repeat(200);
        let slug = sanitize_slug(&long, "x");
        assert_eq!(slug.chars().count(), MAX_SLUG_CHARS);
        // 必须是合法 UTF-8（能被正常处理）
        assert!(slug.chars().all(|c| c == '很'));
    }

    #[test]
    fn dedupe_concept_id_appends_suffix() {
        let taken = ["decisions/use-jwt", "decisions/use-jwt-2"];
        let id = dedupe_concept_id("decisions/use-jwt", |c| taken.contains(&c));
        assert_eq!(id, "decisions/use-jwt-3");
        // 未占用则原样返回
        assert_eq!(dedupe_concept_id("decisions/fresh", |_| false), "decisions/fresh");
    }

    #[test]
    fn split_frontmatter_tolerates_crlf_and_bom() {
        let crlf = "\u{feff}---\r\ntype: Fact\r\n---\r\nbody\r\n";
        let (yaml, body) = split_frontmatter(crlf).unwrap();
        assert!(yaml.contains("type: Fact"), "{yaml:?}");
        assert!(body.trim() == "body", "{body:?}");
    }

    #[test]
    fn split_frontmatter_ignores_dashes_inside_yaml() {
        // YAML 值与正文里的 `---` 不应被误当闭合标记
        let text = "---\ntype: Fact\ndescription: a---b\n---\n正文\n\n---\n\n分隔线\n";
        let (yaml, body) = split_frontmatter(text).unwrap();
        assert!(yaml.contains("a---b"), "{yaml:?}");
        assert!(body.contains("分隔线"), "{body:?}");
    }
}

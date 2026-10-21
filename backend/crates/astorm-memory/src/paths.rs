//! 记忆 Bundle 路径与保留文件名。
//!
//! 目录布局（`<user_dir>/memory/`；用户目录 = `AppConfig.workspace`，默认 `~/astorm`）：
//! ```text
//! memory/
//!   raw/sessions/2026-09-25-<sid8>.md   # 归一化来源（不可变层：写后不再修改）
//!   wiki/                               # 编译产物（OKF v0.2 Bundle）
//!     index.md                          # 保留文件：根导航（frontmatter 声明 okf_version）
//!     log.md                            # 保留文件：变更历史（倒序）
//!     <category.dir>/                   # 概念分类目录（由 MemoryConfig.categories 决定）
//!       index.md                        # 保留文件：目录索引（由 writer 重建）
//!       <slug>.md                       # Concept（所在目录由分类决定）
//!   .okfstate.json                      # 增量状态（已处理会话的哈希记录）
//! ```
//!
//! **Concept ID** = 相对 `wiki/` 去掉 `.md` 的路径（如 `decisions/use-jwt`），即 OKF 中概念的身份。
//! `index.md` / `log.md` 是 OKF **保留文件名**，任何层级都不得作为 Concept 文档。
//!
//! 分类目录**不做白名单**：OKF 要求消费者容忍未知 type，因此读侧（列表 / 索引重建 / 校验 / 检索）
//! 一律扫描 `wiki/` 下实际存在的目录（见 [`BundlePaths::concept_dirs`] / [`BundlePaths::concept_files`]），
//! 手工放入的自定义分类目录同样能被列出、校验与检索。

use std::path::{Path, PathBuf};

/// OKF 规范版本（写入根 `index.md` frontmatter 的 `okf_version`）。
pub const OKF_VERSION: &str = "0.2";

/// raw 层目录名（相对 bundle 根）。
pub const RAW_DIR: &str = "raw";
/// 归一化会话目录名（相对 `raw/`）。
pub const RAW_SESSIONS_DIR: &str = "sessions";
/// wiki 层目录名（相对 bundle 根）。
pub const WIKI_DIR: &str = "wiki";
/// 增量状态文件名（相对 bundle 根）。
pub const STATE_FILE: &str = ".okfstate.json";
/// Bundle 归属标记文件（本管道创建时写入；用于识别「这是自己的 bundle」）。
pub const MARKER_FILE: &str = ".okf-bundle.json";
/// 全文索引目录名（默认位于 Bundle 内部；归属判定时忽略——它是我们自己的产物）。
pub const INDEX_DIR_NAME: &str = ".okf-index";
/// 保留文件名：目录导航（各层级自动重建）。
pub const RESERVED_INDEX: &str = "index.md";
/// 保留文件名：变更历史。
pub const RESERVED_LOG: &str = "log.md";

/// Bundle 归属判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleOwnership {
    /// 目录不存在或为空 → 可安全初始化。
    Absent,
    /// 含本管道的标记（`.okf-bundle.json` / `.okfstate.json`）→ 自己的 bundle。
    Owned,
    /// 有内容但不含本管道标记 → **他人的 bundle，拒绝写入**。
    Foreign {
        /// 现有内容（用于错误提示，最多 8 项）。
        evidence: Vec<String>,
    },
}

/// 记忆 Bundle 路径集（根为 `<用户目录>/memory`，每次操作现算，跟随系统设置）。
#[derive(Debug, Clone)]
pub struct BundlePaths {
    root: PathBuf,
}

impl BundlePaths {
    /// 由 bundle 根目录构建（不创建目录，写操作前调 [`Self::ensure_dirs`]）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// bundle 根目录（`<用户目录>/memory`）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 归一化来源目录（`raw/sessions/`）。
    pub fn raw_sessions_dir(&self) -> PathBuf {
        self.root.join(RAW_DIR).join(RAW_SESSIONS_DIR)
    }

    /// 编译产物根（`wiki/`）。
    pub fn wiki_dir(&self) -> PathBuf {
        self.root.join(WIKI_DIR)
    }

    /// 增量状态文件（`.okfstate.json`）。
    pub fn state_file(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

/// Concept ID → 落盘文件路径（`wiki/{concept_id}.md`；不校验存在性）。
///
/// **不做穿越校验**：写入侧的 id 由 [`crate::concept::build_concept_id`] 自造，天然安全：
/// 读路径（id 可能来自用户或**模型**）请改用 [`Self::concept_path_checked`]。
pub fn concept_path(&self, concept_id: &str) -> PathBuf {
    let mut path = self.wiki_dir();
    for seg in concept_id.split('/') {
        if !seg.is_empty() {
            path.push(seg);
        }
    }
    path.set_extension("md");
    path
}

/// 读路径准入版：先校验 id，再确认落点仍在 `wiki/` 内（双重保证）。
///
/// 为什么必须有：`concept_path` 是按段 `push` 的，`..` 段会真的一层层跳出去 —— 一旦 id 来自
/// 模型（`memory` 工具 / 未来任何 agent 通路），`read ../../../../某文件.md` 就是**越权读取**，
/// 而且工具结果还会回喂给模型。
pub fn concept_path_checked(&self, concept_id: &str) -> Result<PathBuf, String> {
    validate_concept_id(concept_id)?;
    let path = self.concept_path(concept_id);
    if !lexical_inside(&self.wiki_dir(), &path) {
        return Err(format!("Concept ID 落点越出 wiki/：{concept_id}"));
    }
    Ok(path)
}

/// 归一化来源文件名主干 → 路径（`raw/sessions/{stem}.md`；不校验存在性）。
///
/// 同 [`Self::concept_path`]：读路径请用 [`Self::raw_file_checked`]。
pub fn raw_file(&self, stem: &str) -> PathBuf {
    self.raw_sessions_dir().join(format!("{stem}.md"))
}

/// 读路径准入版：`stem` 必须是单一文件名（不含任何分隔符 / `..`）。
pub fn raw_file_checked(&self, stem: &str) -> Result<PathBuf, String> {
    let stem = stem.trim();
    if stem.is_empty() {
        return Err("来源标识不能为空".into());
    }
    if stem.contains(['/', '\\']) {
        return Err(format!("来源标识只能是文件名（不含路径分隔符）: {stem}"));
    }
    if stem == "." || stem == ".." || stem.starts_with('.') {
        return Err(format!("来源标识不合法: {stem}"));
    }
    if stem.chars().any(char::is_control) {
        return Err(format!("来源标识不能包含控制字符: {stem:?}"));
    }
    let path = self.raw_file(stem);
    if !lexical_inside(&self.raw_sessions_dir(), &path) {
        return Err(format!("来源标识落点越出 raw/sessions/：{stem}"));
    }
    Ok(path)
}


    /// 目录索引文件路径（`wiki/{dir}/index.md`；`dir` 为空 → 根索引）。
    pub fn index_path(&self, dir: &str) -> PathBuf {
        if dir.is_empty() {
            self.root_index_path()
        } else {
            self.wiki_dir().join(dir).join(RESERVED_INDEX)
        }
    }

    /// 根索引路径（`wiki/index.md`）。
    pub fn root_index_path(&self) -> PathBuf {
        self.wiki_dir().join(RESERVED_INDEX)
    }

    /// 变更历史路径（`wiki/log.md`）。
    pub fn log_path(&self) -> PathBuf {
        self.wiki_dir().join(RESERVED_LOG)
    }

    /// bundle 是否已存在（根目录存在即视为已初始化）。
    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    /// Bundle 归属标记文件路径（`.okf-bundle.json`）。
    pub fn marker_file(&self) -> PathBuf {
        self.root.join(MARKER_FILE)
    }

    /// 判定 Bundle 归属：防止把**他人已有内容**当作自己的 bundle 破坏。
    pub fn ownership(&self) -> BundleOwnership {
        if !self.root.is_dir() {
            return BundleOwnership::Absent;
        }
        // 忽略本管道自己的产物与系统垃圾文件：只剩这些时说明「尚未初始化」，而不是「他人的目录」
        let entries: Vec<String> = std::fs::read_dir(&self.root)
            .map(|it| {
                it.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| {
                        n != ".DS_Store" && n != MARKER_FILE && n != STATE_FILE && n != INDEX_DIR_NAME
                    })
                    .collect()
            })
            .unwrap_or_default();
        if entries.is_empty() {
            return BundleOwnership::Absent;
        }
        // 本管道的标记：归属标记文件 或 增量状态文件（兼容早期只写状态文件的版本）
        if self.marker_file().is_file() || self.state_file().is_file() {
            return BundleOwnership::Owned;
        }
        let mut evidence = entries;
        evidence.sort();
        evidence.truncate(8);
        BundleOwnership::Foreign { evidence }
    }

    /// 写入前守卫：目标目录有内容但不属于本管道 → 报错（**绝不覆盖**）。
    pub fn ensure_writable(&self) -> Result<(), String> {
        match self.ownership() {
            BundleOwnership::Foreign { evidence } => Err(format!(
                "目标目录已有内容，但不属于本记忆管道，拒绝写入以免破坏原有数据：\n  {}\n  现有内容: {}\n\
                 如确认要在此建立记忆 Bundle，请改指向一个空目录（CLI 用 --bundle 指定）；\n\
                 若那是你已有的其他系统（例如外部 memory 工具），请换路径，勿在此写入。",
                self.root.display(),
                evidence.join(", ")
            )),
            _ => Ok(()),
        }
    }

    /// 创建 bundle 骨架目录：`raw/sessions/` + `wiki/` + 各分类目录（`dirs` 为分类目录名，
    /// 空串表示 wiki 根，跳过）。
    ///
    /// 先做归属守卫：目标目录有**他人内容**时直接报错（见 [`Self::ensure_writable`]）。
    pub fn ensure_dirs(&self, dirs: &[String]) -> Result<(), String> {
        self.ensure_writable()?;
        let mut all = vec![self.raw_sessions_dir(), self.wiki_dir()];
        for dir in dirs {
            let dir = dir.trim_matches('/');
            if !dir.is_empty() {
                all.push(self.wiki_dir().join(dir));
            }
        }
        for dir in all {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("创建记忆目录失败({}): {e}", dir.display()))?;
        }
        // 首次创建时写入归属标记（之后 ownership() 即判为 Owned）
        if !self.marker_file().is_file() {
            let marker = format!(
                "{{\n  \"producer\": \"astorm-memory\",\n  \"okfVersion\": \"{OKF_VERSION}\"\n}}\n"
            );
            std::fs::write(self.marker_file(), marker)
                .map_err(|e| format!("写入 Bundle 标记失败({}): {e}", self.marker_file().display()))?;
        }
        Ok(())
    }

    /// 扫描 `wiki/` 下**实际存在且含至少一个 Concept** 的分类目录名（升序）。
    ///
    /// 不做白名单：手工放入的自定义分类目录同样会被列出。
    pub fn concept_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .concept_files()
            .into_iter()
            .filter_map(|(id, _)| id.split_once('/').map(|(dir, _)| dir.to_string()))
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// 扫描 `wiki/` 下全部 Concept（含 wiki 根下的散落文件）：`(concept_id, 文件路径)`，按 ID 升序。
    pub fn concept_files(&self) -> Vec<(String, PathBuf)> {
        let wiki = self.wiki_dir();
        let mut out: Vec<(String, PathBuf)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&wiki) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // 分类目录：收集其下 .md（跳过保留文件与更深层目录——OKF 概念为单层 slug）
                let Ok(inner) = std::fs::read_dir(&path) else {
                    continue;
                };
                for item in inner.flatten() {
                    let item_path = item.path();
                    let item_name = item.file_name().to_string_lossy().into_owned();
                    if let Some(slug) = concept_slug(&item_name, item_path.is_file()) {
                        out.push((format!("{name}/{slug}"), item_path));
                    }
                }
            } else if let Some(slug) = concept_slug(&name, true) {
                // wiki 根下的散落 Concept（无分类目录）
                out.push((slug, path));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// 文件名 → Concept slug（`.md`、非保留文件、非隐藏文件；否则 None）。
fn concept_slug(name: &str, is_file: bool) -> Option<String> {
    if !is_file || name.starts_with('.') || is_reserved_file_name(name) {
        return None;
    }
    let stem = name.strip_suffix(".md").or_else(|| name.strip_suffix(".MD"))?;
    (!stem.is_empty()).then(|| stem.to_string())
}

/// 文件名是否为 OKF 保留文件（`index.md` / `log.md`，任何层级都不得作为 Concept）。
pub fn is_reserved_file_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(RESERVED_INDEX) || name.eq_ignore_ascii_case(RESERVED_LOG)
}

/// Concept ID 准入校验（**读路径**；写路径的 id 由 `build_concept_id` 自造，无需校验）。
///
/// 规则：非空、非绝对路径、无盘符、无反斜杠、无控制字符、无空段 / `.` / `..` / 以点开头的段、
/// 末段不命中 OKF 保留名（`index` / `log`）。
/// **层级数不设上限**：层级本身不是安全边界，真正的保证是 [`lexical_inside`] 的蕴含检查。
pub fn validate_concept_id(concept_id: &str) -> Result<(), String> {
    let id = concept_id.trim();
    if id.is_empty() {
        return Err("Concept ID 不能为空".into());
    }
    if id.starts_with('/') || id.starts_with('\\') {
        return Err(format!("Concept ID 不能是绝对路径: {id}"));
    }
    if id.contains('\\') {
        return Err(format!("Concept ID 不能包含反斜杠（层级用 / 分隔）: {id}"));
    }
    if id.len() >= 2 && id.as_bytes()[1] == b':' {
        return Err(format!("Concept ID 不能是盘符路径: {id}"));
    }
    if id.chars().any(char::is_control) {
        return Err(format!("Concept ID 不能包含控制字符: {id:?}"));
    }
    for seg in id.split('/') {
        if seg.is_empty() {
            return Err(format!("Concept ID 含空路径段: {id}"));
        }
        if seg == "." || seg == ".." {
            return Err(format!("Concept ID 不允许相对路径段（`.` / `..`）: {id}"));
        }
        if seg.starts_with('.') {
            return Err(format!("Concept ID 的路径段不能以点开头（会被目录扫描跳过）: {id}"));
        }
    }
    let last = id.rsplit('/').next().unwrap_or(id);
    if is_reserved_file_name(&format!("{last}.md")) {
        return Err(format!("Concept ID 命中 OKF 保留文件名（index / log）: {id}"));
    }
    Ok(())
}

/// `child` 是否位于 `base` 之内（**纯词法**判断，不访问文件系统、不做符号链接解析）。
///
/// 用作 [`validate_concept_id`] 之外的蕴含检查：段校验万一将来被放松，
/// 这一层仍会挡住任何向上逃逸的落点。
fn lexical_inside(base: &Path, child: &Path) -> bool {
    let Ok(rel) = child.strip_prefix(base) else {
        return false;
    };
    let mut depth: i64 = 0;
    for seg in rel.components() {
        match seg {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> BundlePaths {
        BundlePaths::new("/tmp/memory-bundle")
    }

    fn dirs() -> Vec<String> {
        ["decisions", "preferences", "concepts", "instructions", "entities"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn layout_matches_documentation() {
        let p = paths();
        assert_eq!(p.root(), Path::new("/tmp/memory-bundle"));
        assert_eq!(p.raw_sessions_dir(), Path::new("/tmp/memory-bundle/raw/sessions"));
        assert_eq!(p.wiki_dir(), Path::new("/tmp/memory-bundle/wiki"));
        assert_eq!(p.state_file(), Path::new("/tmp/memory-bundle/.okfstate.json"));
        assert_eq!(p.log_path(), Path::new("/tmp/memory-bundle/wiki/log.md"));
        assert_eq!(p.root_index_path(), Path::new("/tmp/memory-bundle/wiki/index.md"));
        assert_eq!(
            p.index_path("decisions"),
            Path::new("/tmp/memory-bundle/wiki/decisions/index.md")
        );
        // 空 dir → 根索引
        assert_eq!(p.index_path(""), Path::new("/tmp/memory-bundle/wiki/index.md"));
    }

    #[test]
    fn concept_path_nests_and_sets_md_extension() {
        let p = paths();
        assert_eq!(
            p.concept_path("decisions/use-jwt"),
            Path::new("/tmp/memory-bundle/wiki/decisions/use-jwt.md")
        );
        // 中文 slug 原样保留
        assert_eq!(
            p.concept_path("concepts/认证方案"),
            Path::new("/tmp/memory-bundle/wiki/concepts/认证方案.md")
        );
    }

    #[test]
    fn concept_path_ignores_empty_segments() {
        let p = paths();
        assert_eq!(
            p.concept_path("/decisions//use-jwt/"),
            Path::new("/tmp/memory-bundle/wiki/decisions/use-jwt.md")
        );
    }

    #[test]
    fn reserved_names_are_recognized_case_insensitively() {
        assert!(is_reserved_file_name("index.md"));
        assert!(is_reserved_file_name("INDEX.md"));
        assert!(is_reserved_file_name("log.md"));
        assert!(!is_reserved_file_name("indexes.md"));
        assert!(!is_reserved_file_name("use-jwt.md"));
    }

    #[test]
    fn ensure_dirs_creates_skeleton_from_config() {
        let root = std::env::temp_dir().join(format!("astorm_mem_paths_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = BundlePaths::new(&root);
        assert!(!p.exists());
        p.ensure_dirs(&dirs()).unwrap();
        assert!(p.exists());
        assert!(p.raw_sessions_dir().is_dir());
        assert!(p.wiki_dir().is_dir());
        for dir in dirs() {
            assert!(p.wiki_dir().join(&dir).is_dir(), "缺少目录 {dir}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 归属判定：空目录可初始化；写入后变为 Owned；他人的 bundle 拒绝写入。
    #[test]
    fn ownership_guards_against_foreign_bundles() {
        let root = std::env::temp_dir().join(format!("astorm_mem_own_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = BundlePaths::new(&root);

        // 不存在 → Absent，可写
        assert_eq!(p.ownership(), BundleOwnership::Absent);
        assert!(p.ensure_writable().is_ok());

        // 空目录 → Absent
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(p.ownership(), BundleOwnership::Absent);

        // 初始化后 → Owned 且写了标记
        p.ensure_dirs(&["decisions".into()]).unwrap();
        assert_eq!(p.ownership(), BundleOwnership::Owned);
        assert!(p.marker_file().is_file());
        assert!(p.ensure_writable().is_ok());

        // 模拟「他人的 bundle」：有内容但无本管道标记
        let foreign = std::env::temp_dir().join(format!("astorm_mem_foreign_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&foreign);
        std::fs::create_dir_all(foreign.join("wiki")).unwrap();
        std::fs::write(foreign.join("wiki/index.md"), "---\nokf_version: \"0.2\"\n---\n").unwrap();
        std::fs::write(foreign.join("memory.json"), "{}").unwrap();
        let fp = BundlePaths::new(&foreign);
        match fp.ownership() {
            BundleOwnership::Foreign { evidence } => {
                assert!(evidence.iter().any(|e| e == "wiki"), "{evidence:?}");
                assert!(evidence.iter().any(|e| e == "memory.json"), "{evidence:?}");
            }
            other => panic!("应判为 Foreign，实际 {other:?}"),
        }
        let err = fp.ensure_dirs(&[]).unwrap_err();
        assert!(err.contains("不属于本记忆管道"), "{err}");
        assert!(
            foreign.join("wiki/index.md").is_file(),
            "守卫触发后不得改动他人文件"
        );
        assert!(!fp.marker_file().is_file(), "不得在他人目录写标记");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&foreign);
    }

    /// 仅含本管道自己的索引目录（还没有标记文件）→ 不算他人目录，仍可初始化。
    #[test]
    fn ownership_ignores_own_index_dir() {
        let root = std::env::temp_dir().join(format!("astorm_mem_ownidx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(INDEX_DIR_NAME)).unwrap();
        std::fs::write(root.join(INDEX_DIR_NAME).join("schema_version"), "1").unwrap();
        let p = BundlePaths::new(&root);
        assert_eq!(
            p.ownership(),
            BundleOwnership::Absent,
            "自己的索引目录不应被判为他人内容"
        );
        assert!(p.ensure_dirs(&[]).is_ok());
        assert_eq!(p.ownership(), BundleOwnership::Owned);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 动态目录发现：手工放入的自定义分类目录也能被列出（OKF 容忍未知 type）。
    #[test]
    fn concept_discovery_is_dynamic_not_whitelisted() {
        let root = std::env::temp_dir().join(format!("astorm_mem_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = BundlePaths::new(&root);
        p.ensure_dirs(&dirs()).unwrap();

        // 配置里没有 runbooks/，手工放入
        let custom = p.wiki_dir().join("runbooks");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("rollback.md"), "---\ntype: Runbook\n---\n回滚步骤\n").unwrap();
        // 保留文件与隐藏文件必须被排除
        std::fs::write(p.wiki_dir().join("decisions/index.md"), "x").unwrap();
        std::fs::write(p.wiki_dir().join("decisions/log.md"), "x").unwrap();
        std::fs::write(p.wiki_dir().join("decisions/.hidden.md"), "x").unwrap();
        // 空目录不应出现在分类列表
        std::fs::create_dir_all(p.wiki_dir().join("empty-dir")).unwrap();
        // 真正的概念
        std::fs::write(p.wiki_dir().join("decisions/use-jwt.md"), "---\ntype: Decision\n---\n").unwrap();
        // wiki 根下的散落概念
        std::fs::write(p.wiki_dir().join("loose.md"), "---\ntype: Note\n---\n").unwrap();

        let files = p.concept_files();
        let ids: Vec<&str> = files.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["decisions/use-jwt", "loose", "runbooks/rollback"], "{ids:?}");

        let dirs_found = p.concept_dirs();
        assert_eq!(dirs_found, vec!["decisions", "runbooks"], "{dirs_found:?}");
        assert!(!dirs_found.contains(&"empty-dir".to_string()), "空目录不应被列出");
        assert!(!dirs_found.contains(&"concepts".to_string()), "无概念目录不应被列出");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 读路径准入：`..` 这类穿越必须被挡住（否则模型填参就成了越权读文件）。
    #[test]
    fn concept_id_validation_rejects_traversal() {
        for bad in [
            "",
            "   ",
            "/etc/passwd",
            "\\windows\\system32",
            "C:/Users/x/secret",
            "../../../etc/passwd",
            "decisions/../../secret",
            "decisions/..",
            "./decisions/x",
            "decisions//x",
            "decisions/./x",
            "decisions\\x",
            ".hidden/x",
            "decisions/index",
            "log",
            "decisions/LOG",
            "decisions/x\u{0}",
            "decisions/x\ny",
        ] {
            assert!(
                validate_concept_id(bad).is_err(),
                "应被拒绝（读路径准入）: {bad:?}"
            );
        }
        // 合法：中文、连字符、一到多层级、根下散落概念
        for ok in ["loose", "decisions/use-jwt", "concepts/dll-重命名为-node-才能被加载", "a/b/c"] {
            assert!(validate_concept_id(ok).is_ok(), "应被接受: {ok:?}");
        }
    }

    /// checked 版本：穿越 id 一律 Err，合法 id 与未校验版本落点一致。
    #[test]
    fn checked_paths_refuse_escapes() {
        let p = paths();
        let err = p.concept_path_checked("../../../../etc/passwd").unwrap_err();
        assert!(err.contains("不允许"), "{err}");
        assert!(p.concept_path_checked("decisions/index").is_err(), "保留名应被拒");
        assert_eq!(
            p.concept_path_checked("decisions/use-jwt").unwrap(),
            p.concept_path("decisions/use-jwt"),
            "合法 id 落点应与未校验版本一致"
        );

        // raw stem：只允许单一文件名
        for bad in ["", "  ", "../secret", "sub/dir", "a\\b", ".hidden", ".."] {
            assert!(p.raw_file_checked(bad).is_err(), "应被拒绝: {bad:?}");
        }
        assert_eq!(
            p.raw_file_checked("2026-09-25-99a57122").unwrap(),
            p.raw_file("2026-09-25-99a57122")
        );
    }

    /// 蕴含检查本身：向上逃逸的落点必须为 false（即便段校验被放松）。
    #[test]
    fn lexical_containment_detects_escapes() {
        let base = Path::new("/bundle/wiki");
        assert!(lexical_inside(base, Path::new("/bundle/wiki/decisions/a.md")));
        assert!(lexical_inside(base, Path::new("/bundle/wiki/a.md")));
        assert!(!lexical_inside(base, Path::new("/bundle/wiki/../secret.md")));
        assert!(!lexical_inside(base, Path::new("/bundle/wiki/a/../../../etc/x.md")));
        assert!(!lexical_inside(base, Path::new("/bundle/other/a.md")));
        assert!(!lexical_inside(base, Path::new("/etc/passwd")));
    }
}

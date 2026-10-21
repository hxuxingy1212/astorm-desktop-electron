//! SkillManager：全局技能管理器（仿 ToolManager 模式）。
//!
//! 由 `App` 持有；agent 提示词注入、`skills` 工具与技能门面（settings 页 CRUD）
//! 都经它访问 [`SkillIndex`]，业务层不直接触碰索引互斥锁。
//! 创建 / 更新技能时在本层写 SKILL.md 文件（`user_dir/<name>/SKILL.md`），
//! 与 astorm-desktop 的存储布局保持一致。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use astorm_common::skill::{CreateSkillRequest, SkillUpdate};
use chrono::Utc;
use serde::Serialize;

use super::{DiscoveredSkill, SkillDoc, SkillIndex, SkillSummary};

/// SKILL.md frontmatter 序列化结构（与 [`SkillIndex::parse_skill_content`] 互逆）。
#[derive(Serialize)]
struct SkillFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<&'a str>,
    metadata: FrontmatterMetadata<'a>,
}

/// frontmatter 元数据（category / tags 均为逗号分隔字符串，与解析侧一致）。
#[derive(Serialize)]
struct FrontmatterMetadata<'a> {
    #[serde(skip_serializing_if = "str::is_empty")]
    category: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    tags: &'a str,
}

/// 技能管理器（全局单例；`AppState.skill`）。
pub struct SkillManager {
    index: Arc<Mutex<SkillIndex>>,
}

impl SkillManager {
    /// 包装共享索引（初始化时构建，进程内不变）。
    pub fn new(index: Arc<Mutex<SkillIndex>>) -> Self {
        Self { index }
    }

    /// 按名称列表取技能摘要（提示词注入：`- name: desc (source_path)`）。
    pub fn list_by_names(&self, names: &[String]) -> Result<Vec<SkillSummary>, String> {
        self.index.lock().map_err(|e| e.to_string())?.list_by_names(names)
    }

    /// 按名称读取完整技能（`skills` 工具 load）。
    pub fn get_by_name(&self, name: &str) -> Result<Option<SkillDoc>, String> {
        self.index.lock().map_err(|e| e.to_string())?.get_by_name(name)
    }

    /// 全文检索：按名称 / 描述命中。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSummary>, String> {
        self.index.lock().map_err(|e| e.to_string())?.search(query, limit)
    }

    /// 全文检索：仅按正文命中。
    pub fn search_content(&self, query: &str, limit: usize) -> Result<Vec<SkillSummary>, String> {
        self.index
            .lock()
            .map_err(|e| e.to_string())?
            .search_content(query, limit)
    }

    /// 用户技能目录（供前端定位 / 展示技能包位置）。
    pub fn user_skills_dir(&self) -> PathBuf {
        self.index
            .lock()
            .map(|g| g.user_skills_dir())
            .unwrap_or_else(|e| e.into_inner().user_skills_dir())
    }

    /// 列出全部技能（可按启停 / 分类过滤；`list_skills`）。
    pub fn list(&self, enabled_only: bool, category: Option<&str>) -> Result<Vec<SkillSummary>, String> {
        self.index
            .lock()
            .map_err(|e| e.to_string())?
            .list(enabled_only, category)
    }

    /// 扫描技能目录发现新技能（`discover_skills`）。
    pub fn discover(&self) -> Result<Vec<DiscoveredSkill>, String> {
        self.index.lock().map_err(|e| e.to_string())?.discover()
    }

    /// 按一批路径导入技能（SKILL.md 文件或目录），逐条复制 + 索引，最后统一 commit。
    /// 单个失败仅跳过（返回成功导入的摘要列表）。
    pub fn import_paths(&self, paths: &[String]) -> Result<Vec<SkillSummary>, String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let mut imported = Vec::new();
        for p in paths {
            let path = Path::new(p);
            let candidates = collect_skill_candidates(path);
            for f in candidates {
                if let Ok(doc) = index.import_from_path(&f) {
                    imported.push(to_summary(doc));
                }
            }
        }
        index.commit()?;
        Ok(imported)
    }

    /// 注册单个路径的技能（`skills` 工具 register）：候选收集与 [`Self::import_paths`]
    /// 相同，但逐条收集失败原因（不静默跳过），供工具层向模型反馈校验错误。
    /// 返回 (成功导入的摘要列表, 失败原因列表)；路径下无 SKILL.md 候选视为结构错误。
    pub fn register_path(&self, path: &str) -> Result<(Vec<SkillSummary>, Vec<String>), String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let candidates = collect_skill_candidates(Path::new(path));
        if candidates.is_empty() {
            return Err(format!(
                "路径下未发现技能：{path}。期望的结构为 <目录>/SKILL.md（单个技能）或 <目录>/<技能名>/SKILL.md（技能集合）"
            ));
        }
        let mut imported = Vec::new();
        let mut errors = Vec::new();
        for f in candidates {
            match index.import_from_path(&f) {
                Ok(doc) => imported.push(to_summary(doc)),
                Err(e) => errors.push(e),
            }
        }
        index.commit()?;
        Ok((imported, errors))
    }

    /// 导入技能包（zip / tar.gz），解压后逐个导入并统一 commit（`import_skill_package`）。
    pub fn import_archive(&self, archive: &str) -> Result<Vec<SkillSummary>, String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let docs = index.import_from_archive(Path::new(archive))?;
        index.commit()?;
        Ok(docs.into_iter().map(to_summary).collect())
    }

    /// 创建技能：写 `user_dir/<name>/SKILL.md` + 索引（重名报错；`create_skill`）。
    pub fn create(&self, req: &CreateSkillRequest) -> Result<SkillDoc, String> {
        SkillIndex::validate_skill_name(&req.name)?;
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        if index.get_by_name(&req.name)?.is_some() {
            return Err(format!("技能 '{}' 已存在", req.name));
        }
        let dir = index.user_skills_dir().join(&req.name);
        let dest = dir.join("SKILL.md");
        if dest.exists() {
            return Err(format!("技能 '{}' 已存在", req.name));
        }
        let doc = SkillDoc {
            name: req.name.clone(),
            description: req.description.clone(),
            prompt: req.prompt.clone(),
            category: req.category.join(","),
            tags: req.tags.clone(),
            version: if req.version.is_empty() {
                "1.0.0".into()
            } else {
                req.version.clone()
            },
            author: req.author.clone(),
            source: "user".into(),
            source_path: dest.to_string_lossy().into_owned(),
            enabled: true,
            updated_at: Utc::now().timestamp(),
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
        write_skill_file(&doc)?;
        index.index_skill(&doc)?;
        index.commit()?;
        Ok(doc)
    }

    /// 更新技能字段（仅 user 来源；文件留在原路径，改名会清理旧索引项；`update_skill`）。
    pub fn update(&self, name: &str, updates: &SkillUpdate) -> Result<SkillDoc, String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let mut doc = index
            .get_by_name(name)?
            .ok_or_else(|| format!("技能不存在: {name}"))?;
        if doc.source != "user" {
            return Err("只能修改用户导入的 skill".into());
        }
        let old_name = doc.name.clone();
        if let Some(v) = &updates.name {
            if !v.trim().is_empty() {
                doc.name = v.clone();
            }
        }
        if let Some(v) = &updates.description {
            doc.description = v.clone();
        }
        if let Some(v) = &updates.prompt {
            doc.prompt = v.clone();
        }
        if let Some(v) = &updates.category {
            doc.category = v.clone();
        }
        if let Some(v) = &updates.tags {
            doc.tags = v.clone();
        }
        if let Some(v) = &updates.version {
            if !v.is_empty() {
                doc.version = v.clone();
            }
        }
        if let Some(v) = &updates.author {
            doc.author = v.clone();
        }
        doc.updated_at = Utc::now().timestamp();
        write_skill_file(&doc)?;
        if old_name != doc.name {
            index.remove_skill(&old_name)?;
        }
        index.index_skill(&doc)?;
        index.commit()?;
        Ok(doc)
    }

    /// 启停技能（仅更新索引，不改文件；`toggle_skill`）。
    pub fn toggle(&self, name: &str, enabled: bool) -> Result<SkillDoc, String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let mut doc = index
            .get_by_name(name)?
            .ok_or_else(|| format!("技能不存在: {name}"))?;
        doc.enabled = enabled;
        doc.updated_at = Utc::now().timestamp();
        index.index_skill(&doc)?;
        index.commit()?;
        Ok(doc)
    }

    /// 删除技能（仅 user 来源；删除整目录 + 索引项；`remove_skill`）。
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut index = self.index.lock().map_err(|e| e.to_string())?;
        let doc = index
            .get_by_name(name)?
            .ok_or_else(|| format!("技能不存在: {name}"))?;
        if doc.source != "user" {
            return Err("只能删除用户导入的 skill".into());
        }
        // 删除源文件所在目录（user_dir/<name>/ 整个目录）
        if let Some(parent) = Path::new(&doc.source_path).parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
        index.remove_skill(name)?;
        index.commit()?;
        Ok(())
    }
}

/// 把路径解析为待导入的 SKILL.md 候选：
/// - 目录 → 每个直接子目录下的 SKILL.md
/// - 文件 → 本身（须是 SKILL.md）
fn collect_skill_candidates(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let f = p.join("SKILL.md");
                    if f.is_file() {
                        out.push(f);
                    }
                } else if p.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                    out.push(p);
                }
            }
        }
        out
    } else if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// 技能文档 → 摘要。
fn to_summary(doc: SkillDoc) -> SkillSummary {
    SkillSummary {
        name: doc.name,
        description: doc.description,
        category: doc.category,
        version: doc.version,
        source: doc.source,
        source_path: doc.source_path,
        enabled: doc.enabled,
        score: None,
    }
}

/// 以与解析侧互逆的 frontmatter 格式写 SKILL.md 文件。
fn write_skill_file(doc: &SkillDoc) -> Result<(), String> {
    let fm = SkillFrontmatter {
        name: &doc.name,
        description: &doc.description,
        version: Some(doc.version.as_str()).filter(|v| !v.is_empty()),
        author: Some(doc.author.as_str()).filter(|a| !a.is_empty()),
        metadata: FrontmatterMetadata {
            category: &doc.category,
            tags: &doc.tags.join(","),
        },
    };
    let yaml = serde_yaml::to_string(&fm).map_err(|e| format!("frontmatter 序列化失败: {e}"))?;
    let content = format!("---\n{yaml}---\n\n{}", doc.prompt.trim());
    std::fs::write(&doc.source_path, content).map_err(|e| format!("写技能文件失败: {e}"))
}
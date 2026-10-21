//! 技能系统（独立目录）：SKILL.md 全文索引（[`index::SkillIndex`]）+ 全局管理器（[`manager::SkillManager`]）。
//!
//! - `index`：Tantivy + jieba 全文索引 SKILL.md（`---` frontmatter + 正文），支持发现 / 导入 / 检索
//! - `manager`：全局技能管理器（仿 ToolManager 模式，由 `App` 持有；agent 与工具经它访问索引）
//!
//! 技能来源三处：项目 `.astorm/skills`（cwd 向上逐层）、用户全局 `~/.config/astorm/skills`、
//! 用户导入目录（`AppDir.skills_dir`）。

pub mod index;
pub mod manager;

pub use index::SkillIndex;
pub use manager::SkillManager;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

/// 技能摘要（列表 / 搜索 / 提示词注入用，不含正文指令）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// 技能名称。
    pub name: String,
    /// 技能描述。
    pub description: String,
    /// 分类（frontmatter `metadata.category`）。
    pub category: String,
    /// 版本。
    pub version: String,
    /// 来源：file / user / builtin。
    pub source: String,
    /// 源文件路径。
    pub source_path: String,
    /// 是否启用。
    pub enabled: bool,
    /// 相关度得分（搜索时有值，其余为 None）。
    pub score: Option<f32>,
}

/// 技能全文（含 `prompt` 指令；`skills` 工具 load 时返回）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDoc {
    /// 技能名称。
    pub name: String,
    /// 技能描述。
    pub description: String,
    /// 技能指令（SKILL.md frontmatter 后的正文）。
    pub prompt: String,
    /// 分类。
    pub category: String,
    /// 标签。
    pub tags: Vec<String>,
    /// 版本。
    pub version: String,
    /// 作者。
    pub author: String,
    /// 来源（file / user / builtin）。
    pub source: String,
    /// 源文件路径。
    pub source_path: String,
    /// 是否启用。
    pub enabled: bool,
    /// 更新时间（unix 秒）。
    pub updated_at: i64,
}

/// 技能目录 = `SKILL.md` 源路径的父目录。
///
/// 技能正文里的资产引用（`scripts/`、`references/`、`assets/`）都**相对技能目录**，
/// 所以只要给模型看技能，就必须同时给出这个目录，否则它只能按技能名去猜路径
/// （曾编造出 `…/astorm-ppt-generator/astorm-ppt-generator` 这种重复末段的路径）。
/// 源路径为空 / 无父目录时返回 `None`——调用方应省略该行，**不要伪造路径**。
pub fn skill_dir_of(source_path: &str) -> Option<String> {
    let parent = std::path::Path::new(source_path).parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.to_string_lossy().into_owned())
}

/// 目录发现结果（frontmatter 损坏的技能标记 `error`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkill {
    /// 技能名称（解析失败时为目录名）。
    pub name: String,
    /// 技能描述。
    pub description: String,
    /// SKILL.md 文件路径。
    pub path: String,
    /// 解析错误（None = 合法技能）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------- 契约转换：core 技能类型 → astorm-common 门面类型（字段一一对应） ----------

impl From<SkillSummary> for astorm_common::skill::SkillSummary {
    fn from(v: SkillSummary) -> Self {
        astorm_common::skill::SkillSummary {
            name: v.name,
            description: v.description,
            category: v.category,
            version: v.version,
            source: v.source,
            source_path: v.source_path,
            enabled: v.enabled,
            score: v.score,
        }
    }
}

impl From<SkillDoc> for astorm_common::skill::SkillDoc {
    fn from(v: SkillDoc) -> Self {
        astorm_common::skill::SkillDoc {
            name: v.name,
            description: v.description,
            prompt: v.prompt,
            category: v.category,
            tags: v.tags,
            version: v.version,
            author: v.author,
            source: v.source,
            source_path: v.source_path,
            enabled: v.enabled,
            updated_at: v.updated_at,
        }
    }
}

impl From<DiscoveredSkill> for astorm_common::skill::DiscoveredSkill {
    fn from(v: DiscoveredSkill) -> Self {
        astorm_common::skill::DiscoveredSkill {
            name: v.name,
            description: v.description,
            path: v.path,
            error: v.error,
        }
    }
}
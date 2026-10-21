//! 技能类型（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// 技能摘要（列表 / 搜索 / 导入结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// 分类
    pub category: String,
    /// 版本
    pub version: String,
    /// 来源（user / builtin / ...）
    pub source: String,
    /// 源文件路径
    pub source_path: String,
    /// 是否启用
    pub enabled: bool,
    /// 搜索相关度（搜索场景下才有值）
    pub score: Option<f32>,
}

/// 技能完整文档。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDoc {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// SKILL.md 正文（作为 prompt）
    pub prompt: String,
    /// 分类
    pub category: String,
    /// 标签
    pub tags: Vec<String>,
    /// 版本
    pub version: String,
    /// 作者
    pub author: String,
    /// 来源
    pub source: String,
    /// 源文件路径
    pub source_path: String,
    /// 是否启用
    pub enabled: bool,
    /// 更新时间（unix 秒）
    pub updated_at: i64,
}

/// 扫描发现的技能（含读取失败信息）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkill {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// 路径
    pub path: String,
    /// 解析错误（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
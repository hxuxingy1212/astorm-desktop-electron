//! 技能 DTO 镜像。

use astorm_common::skill::{
    CreateSkillRequest as CoreCreateSkillRequest, DiscoveredSkill as CoreDiscoveredSkill,
    SkillDoc as CoreSkillDoc, SkillSummary as CoreSkillSummary, SkillUpdate as CoreSkillUpdate,
};
use napi_derive::napi;
use serde::Serialize;

/// 技能摘要（列表 / 搜索 / 导入结果）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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
    pub score: Option<f64>,
}

impl From<CoreSkillSummary> for SkillSummary {
    fn from(v: CoreSkillSummary) -> Self {
        SkillSummary {
            name: v.name,
            description: v.description,
            category: v.category,
            version: v.version,
            source: v.source,
            source_path: v.source_path,
            enabled: v.enabled,
            score: v.score.map(|s| s as f64),
        }
    }
}

/// 技能完整文档。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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

impl From<CoreSkillDoc> for SkillDoc {
    fn from(v: CoreSkillDoc) -> Self {
        SkillDoc {
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

/// 扫描发现的技能（含读取失败信息）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSkill {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// 路径
    pub path: String,
    /// 解析错误（可选）
    pub error: Option<String>,
}

impl From<CoreDiscoveredSkill> for DiscoveredSkill {
    fn from(v: CoreDiscoveredSkill) -> Self {
        DiscoveredSkill {
            name: v.name,
            description: v.description,
            path: v.path,
            error: v.error,
        }
    }
}

/// 创建技能请求（name / description / prompt 必填，其余可选）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSkillRequest {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// SKILL.md 正文（prompt）
    pub prompt: String,
    /// 分类（可多个）
    pub category: Vec<String>,
    /// 标签
    pub tags: Vec<String>,
    /// 版本
    pub version: String,
    /// 作者
    pub author: String,
}

impl From<CreateSkillRequest> for CoreCreateSkillRequest {
    fn from(r: CreateSkillRequest) -> Self {
        CoreCreateSkillRequest {
            name: r.name,
            description: r.description,
            prompt: r.prompt,
            category: r.category,
            tags: r.tags,
            version: r.version,
            author: r.author,
        }
    }
}

/// 技能字段更新（全部可选，仅更新传入项）。
#[napi(object)]
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdate {
    /// 新名称
    pub name: Option<String>,
    /// 新描述
    pub description: Option<String>,
    /// 新 prompt
    pub prompt: Option<String>,
    /// 新分类
    pub category: Option<String>,
    /// 新标签
    pub tags: Option<Vec<String>>,
    /// 新版本
    pub version: Option<String>,
    /// 新作者
    pub author: Option<String>,
}

impl From<SkillUpdate> for CoreSkillUpdate {
    fn from(u: SkillUpdate) -> Self {
        CoreSkillUpdate {
            name: u.name,
            description: u.description,
            prompt: u.prompt,
            category: u.category,
            tags: u.tags,
            version: u.version,
            author: u.author,
        }
    }
}
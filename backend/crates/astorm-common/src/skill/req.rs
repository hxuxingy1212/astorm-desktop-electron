//! 技能请求体（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// 创建技能请求（name / description / prompt 必填，其余回退默认）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSkillRequest {
    /// 技能名
    pub name: String,
    /// 描述
    pub description: String,
    /// SKILL.md 正文（prompt）
    pub prompt: String,
    /// 分类（可多个）
    #[serde(default)]
    pub category: Vec<String>,
    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 版本
    #[serde(default)]
    pub version: String,
    /// 作者
    #[serde(default)]
    pub author: String,
}

/// 技能字段更新（全部可选，仅更新传入项）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
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
//! list_skills：列出技能（可按启停 / 分类过滤）。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillSummary;

/// 列出技能（可按启停 / 分类过滤）。
#[napi]
pub fn list_skills(
    enabled_only: Option<bool>,
    category: Option<String>,
) -> napi::Result<Vec<SkillSummary>> {
    crate::app()?
        .skills()
        .list_skills(enabled_only, category)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
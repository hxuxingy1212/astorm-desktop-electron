//! import_skills：按路径导入一批技能。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillSummary;

/// 按路径导入一批技能。
#[napi]
pub fn import_skills(paths: Vec<String>) -> napi::Result<Vec<SkillSummary>> {
    crate::app()?
        .skills()
        .import_skills(paths)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
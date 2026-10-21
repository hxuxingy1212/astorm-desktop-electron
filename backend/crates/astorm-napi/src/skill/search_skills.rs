//! search_skills：全文检索技能。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillSummary;

/// 全文检索技能。
#[napi]
pub fn search_skills(query: String, limit: Option<i32>) -> napi::Result<Vec<SkillSummary>> {
    crate::app()?
        .skills()
        .search_skills(query, limit.map(|n| n as usize))
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
//! toggle_skill：启停技能。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillSummary;

/// 启停技能。
#[napi]
pub fn toggle_skill(name: String, enabled: bool) -> napi::Result<SkillSummary> {
    crate::app()?
        .skills()
        .toggle_skill(name, enabled)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
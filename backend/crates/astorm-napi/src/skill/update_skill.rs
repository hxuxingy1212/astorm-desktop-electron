//! update_skill：更新技能字段。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::{SkillSummary, SkillUpdate};

/// 更新技能字段（仅更新传入项）。
#[napi]
pub fn update_skill(name: String, updates: SkillUpdate) -> napi::Result<SkillSummary> {
    crate::app()?
        .skills()
        .update_skill(name, updates.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
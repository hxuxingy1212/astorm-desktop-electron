//! get_skill：按名取技能完整文档。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillDoc;

/// 按名取技能完整文档。
#[napi]
pub fn get_skill(name: String) -> napi::Result<SkillDoc> {
    crate::app()?
        .skills()
        .get_skill(name)
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
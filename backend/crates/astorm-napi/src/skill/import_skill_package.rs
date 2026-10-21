//! import_skill_package：导入技能包（.zip / .tar.gz）。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::SkillSummary;

/// 导入技能包（.zip / .tar.gz）。
#[napi]
pub fn import_skill_package(path: String) -> napi::Result<Vec<SkillSummary>> {
    crate::app()?
        .skills()
        .import_skill_package(path)
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
//! create_skill：创建技能（写 SKILL.md + YAML frontmatter）。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::{CreateSkillRequest, SkillSummary};

/// 创建技能（写 SKILL.md + YAML frontmatter）。
#[napi]
pub fn create_skill(request: CreateSkillRequest) -> napi::Result<SkillSummary> {
    crate::app()?
        .skills()
        .create_skill(request.into())
        .map(Into::into)
        .map_err(|e| napi::Error::from_reason(e))
}
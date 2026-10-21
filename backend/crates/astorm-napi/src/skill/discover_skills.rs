//! discover_skills：扫描技能目录发现新技能。

use astorm_common::skill::Skill;
use napi_derive::napi;

use crate::skill::dto::DiscoveredSkill;

/// 扫描技能目录发现新技能。
#[napi]
pub fn discover_skills() -> napi::Result<Vec<DiscoveredSkill>> {
    crate::app()?
        .skills()
        .discover_skills()
        .map(|list| list.into_iter().map(Into::into).collect())
        .map_err(|e| napi::Error::from_reason(e))
}
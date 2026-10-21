//! remove_skill：删除技能（仅 user 来源）。

use astorm_common::skill::Skill;
use napi_derive::napi;

/// 删除技能（仅 user 来源）。
#[napi]
pub fn remove_skill(name: String) -> napi::Result<()> {
    crate::app()?
        .skills()
        .remove_skill(name)
        .map_err(|e| napi::Error::from_reason(e))
}
//! set_session_skills：设置会话启用技能。

use astorm_common::skill::Skill;
use napi_derive::napi;

/// 设置会话启用技能。
#[napi]
pub fn set_session_skills(session_id: String, skill_names: Vec<String>) -> napi::Result<()> {
    crate::app()?
        .skills()
        .set_session_skills(session_id, skill_names)
        .map_err(|e| napi::Error::from_reason(e))
}
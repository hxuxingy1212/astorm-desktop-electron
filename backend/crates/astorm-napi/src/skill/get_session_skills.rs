//! get_session_skills：会话已启用的技能名列表。

use astorm_common::skill::Skill;
use napi_derive::napi;

/// 会话已启用的技能名列表。
#[napi]
pub fn get_session_skills(session_id: String) -> napi::Result<Vec<String>> {
    crate::app()?
        .skills()
        .get_session_skills(session_id)
        .map_err(|e| napi::Error::from_reason(e))
}
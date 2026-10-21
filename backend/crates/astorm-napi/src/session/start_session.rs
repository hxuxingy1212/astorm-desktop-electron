//! start_session：把「仅创建」落下、尚未执行的指令跑起来。

use astorm_common::session::Session;
use napi_derive::napi;
use uuid::Uuid;

/// 启动会话：以**已有历史**（末尾那条 user 指令）驱动一轮 ReAct，**不追加新消息**。
///
/// 供看板「待办」卡片的「启动」使用（那些会话是「仅创建」建出来的，指令在历史里没跑过）。
/// 正在运行 / 历史为空 / 历史末尾不是用户消息 → 报错。
#[napi]
pub fn start_session(session_id: String) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    crate::app()?
        .session()
        .start_session(&session_id)
        .map_err(|e| napi::Error::from_reason(e))
}

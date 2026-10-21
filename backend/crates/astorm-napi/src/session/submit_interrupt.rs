//! submit_interrupt：提交中断决议（审批同意/拒绝、表单回答/忽略）。
//!
//! 与 `send_message` 并列的**第二条会话输入**：决议不追加用户消息，而是就地把用户决定
//! 写回被中断的那条 tool 消息（决议即 tool 结果），随后循环续跑。

use astorm_common::session::interrupt::{
    InterruptResolution as CoreInterruptResolution, InterruptType as CoreInterruptType,
};
use napi_derive::napi;
use serde::Serialize;
use uuid::Uuid;

/// 中断类型：审批 / 提问 / 写邮件确认。
///
/// 用 `snake_case`（而非 `lowercase`）：`lowercase` 会把 `MailCompose` 拍成 `mailcompose`，
/// 与信封里的 `mail_compose` 不一致 → napi 反序列化报 “does not match any variant”。
#[napi(string_enum = "snake_case")]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptType {
    Approval,
    Form,
    MailCompose,
}

impl From<InterruptType> for CoreInterruptType {
    fn from(v: InterruptType) -> Self {
        match v {
            InterruptType::Approval => CoreInterruptType::Approval,
            InterruptType::Form => CoreInterruptType::Form,
            InterruptType::MailCompose => CoreInterruptType::MailCompose,
        }
    }
}

/// 提交中断决议的请求体（字段按 `interrupt_type` 取用）：
/// - `approval`：`decision` = `allow` / `deny`（`allow_scope = session` 表示本会话内同目标不再弹卡）
/// - `form`：`answer`（表单值对象 JSON 文本）或 `ignored = true`
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInterruptRequest {
    /// 中断请求标识（来自 interrupt 事件 `payload.request_id`）。
    pub request_id: String,
    /// 中断类型（审批 / 提问；与台账登记一致否则报错）。
    pub interrupt_type: InterruptType,
    /// 审批决议：`allow` / `deny`（仅 approval）。
    pub decision: Option<String>,
    /// 表单回答：表单值对象的 JSON 文本（仅 form）。
    pub answer: Option<String>,
    /// 表单忽略：放弃回答（仅 form；不续跑）。
    pub ignored: Option<bool>,
    /// 同意范围（仅 approval + decision=allow）：`once`（缺省）/ `session`
    /// （记入会话内存白名单，本会话内同目标不再弹卡）。
    pub allow_scope: Option<String>,
}

impl From<SubmitInterruptRequest> for CoreInterruptResolution {
    fn from(r: SubmitInterruptRequest) -> Self {
        let allow = match r.decision.as_deref() {
            Some("allow") => Some(true),
            Some("deny") => Some(false),
            _ => None,
        };
        CoreInterruptResolution {
            request_id: r.request_id,
            interrupt_type: r.interrupt_type.into(),
            allow,
            answer: r.answer,
            ignored: r.ignored.unwrap_or(false),
            allow_scope: r.allow_scope,
        }
    }
}

/// 提交中断决议：审批同意（自动重放被拦工具）/ 拒绝，或提交表单回答 / 忽略。
///
/// 决议入队时即校验并取走台账待决项：不存在或已处理 → 报错（前端提示失效）。
#[napi]
pub fn submit_interrupt(
    session_id: String,
    request: SubmitInterruptRequest,
) -> napi::Result<()> {
    let session_id = Uuid::parse_str(&session_id)
        .map_err(|e| napi::Error::from_reason(format!("session_id 非法: {e}")))?;
    let resolution: CoreInterruptResolution = request.into();
    crate::app()?
        .session()
        .resolve_interrupt(&session_id, resolution)
        .map_err(|e| napi::Error::from_reason(e))
}

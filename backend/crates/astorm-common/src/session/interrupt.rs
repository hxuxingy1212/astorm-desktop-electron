//! 中断与决议契约：工具中断（安全拦截待审批 / 向用户提问）的统一请求与决议类型。
//!
//! 中断请求由工具以**纯 JSON 信封**形式随 tool 结果返回（见 `agent::interrupt`，
//! 字段约定：`interrupt` / `interrupt_type` / `request_id` / `tool_call_id` +
//! 类型专属 `approval` 或 `form`）。前端据此弹审批卡或提问表单，用户决定后经
//! `submitInterrupt` 把 [`InterruptResolution`] 交回后端 —— 后端就地把决议写回
//! 那条 tool 消息并继续循环（决议即 tool 结果，不产生额外用户消息）。

use serde::{Deserialize, Serialize};

/// 中断类型（决定前端 UI 与后端决议方式）。
///
/// serde 用 `snake_case` 以与 `as_str()` / 信封里的 `mail_compose` 保持一致
/// （`lowercase` 会把 `MailCompose` 变成 `mailcompose`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptType {
    /// 安全拦截待审批：同意后重放被拦工具 / 拒绝则写拒绝结果
    Approval,
    /// 向用户提问：用户回答写入 tool 结果（可忽略）
    Form,
    /// 写邮件确认：前端就地展示写邮件 UI（只读预览草稿），用户点发送/取消后
    /// 由后端就地执行 `mail.send`（answer = 最终邮件 JSON）或写取消结果（ignored）。
    MailCompose,
}

impl InterruptType {
    /// 小写标识（写入中断信封 `interrupt_type`；与 serde 序列化一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            InterruptType::Approval => "approval",
            InterruptType::Form => "form",
            InterruptType::MailCompose => "mail_compose",
        }
    }
}

impl std::fmt::Display for InterruptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 用户对一次中断的决议（napi `submitInterrupt` 的请求体）。
///
/// 字段按类型使用：
/// - `interrupt_type = approval`：`allow` 必填（true 同意 / false 拒绝）
/// - `interrupt_type = form`：`answer`（表单值 JSON 文本）或 `ignored = true`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptResolution {
    /// 中断请求标识（来自中断信封 `request_id`）。
    pub request_id: String,
    /// 中断类型；与台账中登记的待决项不符时拒绝（防串用）。
    pub interrupt_type: InterruptType,
    /// 审批决议：true = 同意执行（重放）/ false = 拒绝。
    #[serde(default)]
    pub allow: Option<bool>,
    /// 表单回答：表单值对象的 JSON 文本（`{"name":"张三",...}`）。
    #[serde(default)]
    pub answer: Option<String>,
    /// 表单忽略：用户放弃回答（仅 form；落 `.ignored` 终态且不续跑）。
    #[serde(default)]
    pub ignored: bool,
    /// 同意范围（仅 approval + allow=true）：`once`（缺省，一次性）/ `session`
    /// （记入会话内存白名单，本会话内同目标不再弹卡）。拒绝时必须缺省。
    #[serde(default)]
    pub allow_scope: Option<String>,
}

impl InterruptResolution {
    /// 校验字段与类型匹配（napi 入参防呆，避免「审批却没有 allow / 表单却什么都没给」）。
    pub fn validate(&self) -> Result<(), String> {
        if self.request_id.trim().is_empty() {
            return Err("缺少 request_id".into());
        }
        match self.interrupt_type {
            InterruptType::Approval => {
                if self.allow.is_none() {
                    return Err("审批决议缺少 allow".into());
                }
                if let Some(scope) = self.allow_scope.as_deref() {
                    if self.allow != Some(true) {
                        return Err("allow_scope 只能与同意（allow=true）同时出现".into());
                    }
                    if !matches!(scope, "once" | "session") {
                        return Err(format!("未知 allow_scope：{scope}（仅 once / session）"));
                    }
                }
            }
            InterruptType::Form => {
                if !self.ignored && self.answer.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("表单决议缺少 answer（或置 ignored）".into());
                }
            }
            InterruptType::MailCompose => {
                // 发送：answer 为最终邮件 JSON；取消：ignored=true。
                if !self.ignored && self.answer.as_deref().unwrap_or("").trim().is_empty() {
                    return Err("写邮件决议缺少 answer（或置 ignored）".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_mismatched_payloads() {
        let no_id = InterruptResolution {
            request_id: "  ".into(),
            interrupt_type: InterruptType::Approval,
            allow: Some(true),
            answer: None,
            ignored: false,
            allow_scope: None,
        };
        assert!(no_id.validate().is_err());

        let approval_without_allow = InterruptResolution {
            request_id: "r1".into(),
            interrupt_type: InterruptType::Approval,
            allow: None,
            answer: None,
            ignored: false,
            allow_scope: None,
        };
        assert!(approval_without_allow.validate().is_err());

        let form_without_answer = InterruptResolution {
            request_id: "r1".into(),
            interrupt_type: InterruptType::Form,
            allow: None,
            answer: Some("  ".into()),
            ignored: false,
            allow_scope: None,
        };
        assert!(form_without_answer.validate().is_err());

        let ok_form_ignored = InterruptResolution {
            request_id: "r1".into(),
            interrupt_type: InterruptType::Form,
            allow: None,
            answer: None,
            ignored: true,
            allow_scope: None,
        };
        assert!(ok_form_ignored.validate().is_ok());
    }

    /// allow_scope 只能与同意同现，且只接受 once / session。
    #[test]
    fn validate_restricts_allow_scope() {
        let mk = |allow: Option<bool>, scope: Option<&str>| InterruptResolution {
            request_id: "r1".into(),
            interrupt_type: InterruptType::Approval,
            allow,
            answer: None,
            ignored: false,
            allow_scope: scope.map(str::to_string),
        };
        assert!(mk(Some(true), Some("session")).validate().is_ok());
        assert!(mk(Some(true), Some("once")).validate().is_ok());
        assert!(mk(Some(false), Some("session")).validate().is_err(), "拒绝不得带范围");
        assert!(mk(Some(true), Some("forever")).validate().is_err(), "未知范围");
    }

    #[test]
    fn interrupt_type_str_matches_serde() {
        assert_eq!(InterruptType::Approval.as_str(), "approval");
        assert_eq!(serde_json::to_value(InterruptType::Form).unwrap(), serde_json::json!("form"));
    }
}

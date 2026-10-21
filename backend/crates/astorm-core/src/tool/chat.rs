//! `chat_with_user` 工具：生成分布表单向用户收集信息，也可仅用 `description` 文本做简单提问。
//!
//! 与安全审批**同构**：执行后返回「中断信封」（`interrupt_type = "form"` + `form` 载荷），
//! 引擎据此中断循环并向前端发 `interrupt` 事件；前端弹提问表单，用户提交后经
//! `submitInterrupt` 交回 —— **用户回答就写回这条 tool 消息**（决议即 tool 结果，不产生
//! 额外用户消息），随后循环续跑。
//!
//! Auto 模式禁止提问（参考桌面端：自动模式自行决策，不打断用户）——返回说明文本给模型，
//! 不带中断标记，故不会中断。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use astorm_common::session::types::SessionMode;
use uuid::Uuid;

use crate::tool::{session_mode, ToolContext};

/// `chat_with_user` 工具。
pub struct UserChatTool;

/// 表单字段（text / number / boolean / single_choice / multiple_choice）。
#[derive(Debug, Serialize, Deserialize)]
struct FormField {
    name: String,
    /// 字段类型（text / number / boolean / single_choice / multiple_choice）。
    #[serde(rename = "field_type")]
    field_type: String,
    /// 向用户提出的问题原文（前端作为卡片标题展示）。
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    options: Option<Vec<String>>,
    /// 布尔字段「是」选项文案（如「需要」）；其它类型忽略。
    #[serde(default)]
    true_label: Option<String>,
    /// 布尔字段「否」选项文案（如「不需要」）；其它类型忽略。
    #[serde(default)]
    false_label: Option<String>,
}

/// 工具参数。
#[derive(Debug, Deserialize)]
struct UserChatArgs {
    /// 向用户提出的问题文本（无表单字段时作为纯文本提问发送；即本次调用的说明）。
    #[serde(default)]
    description: Option<String>,
    /// 表单字段列表（可空；为空时退化为纯文本提问）。
    #[serde(default)]
    fields: Vec<FormField>,
}

#[async_trait]
impl crate::tool::Tool for UserChatTool {
    fn name(&self) -> &str {
        "chat_with_user"
    }

    fn description(&self) -> &str {
        "向用户提问：description 为向用户提出的问题文本（同时作为卡片标题/本次调用说明，无表单字段时即纯文本提问）；可选提供表单字段收集结构化信息。执行后会中断循环、等待用户提交回答（回答将作为本工具的结果返回）。每个字段必须给出 description（即向用户提出的问题，会作为卡片标题展示）；boolean 字段须给出正反文案 true_label / false_label。自动模式（auto）下不可用——此时自行决策，不打断用户。"
    }

    /// 提问信封（`interrupt_type = "form"`）→ 中断载荷：引擎据此打断并让前端弹提问表单。
    /// Auto 模式返回的是纯文本说明（不带中断标记），自然不命中。
    fn interrupt_payload(&self, result: &str) -> Option<serde_json::Value> {
        crate::tool::policy::interrupt_request_from_result(result)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "向用户提出的问题文本（作为卡片标题展示；即本次提问的说明）" },
                "fields": {
                    "type": "array",
                    "description": "表单字段列表（可空，为空时退化为纯文本提问）",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "字段名：仅作回答回传的键，不展示给用户" },
                            "field_type": { "type": "string", "enum": ["text", "number", "boolean", "single_choice", "multiple_choice"], "description": "字段类型：text/number 文本输入；boolean 是/否二选一（选填字段用户可不选，此时回答里不含该字段）；single_choice 单选；multiple_choice 多选（选择类字段前端会附一个自由输入框，用户可自行补充选项外的答案）" },
                            "description": { "type": "string", "description": "向用户提出的问题原文（必填）：会作为卡片标题展示，一句话说清要问什么" },
                            "required": { "type": "boolean", "description": "是否必填：标题后展示「必填/选填」，必填项未答无法提交" },
                            "options": { "type": "array", "items": { "type": "string" }, "description": "single_choice / multiple_choice 的候选项（选择类字段必给，2-6 个为宜）" },
                            "true_label": { "type": "string", "description": "boolean 字段「是」选项文案，如「需要」；其它类型忽略" },
                            "false_label": { "type": "string", "description": "boolean 字段「否」选项文案，如「不需要」；其它类型忽略" }
                        },
                        "required": ["name", "field_type", "description"]
                    }
                }
            },
            "required": ["description"]
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String> {
        // Auto 模式禁止向用户提问：返回说明文本给模型自行决策（不中断）
        if session_mode(&ctx.session_id) == SessionMode::Auto {
            return Ok("当前为自动模式；如缺少必要条件则中止对话，否则自行决策直到任务完成或中止。".into());
        }

        let parsed: UserChatArgs = serde_json::from_value(params).map_err(|e| format!("参数解析失败: {e}"))?;
        // fields 为空时退化为纯文本提问（仅提示文本），保证无参数调用也能正常提问
        let message = parsed
            .description
            .clone()
            .unwrap_or_else(|| "已发送给用户，等待用户响应".into());
        let request_id = Uuid::new_v4().to_string();
        let form = json!({
            "message": message,
            "tip": parsed.description,
            "fields": parsed.fields,
        });
        let request = json!({
            "interrupt": true,
            "interrupt_type": "form",
            "request_id": request_id,
            "tool_call_id": ctx.tool_call_id,
            "message": "已向用户提问，等待用户提交回答（回答将作为本工具结果返回）。",
            "form": form,
        });
        register_pending_form(ctx, &request);
        serde_json::to_string(&request).map_err(|e| e.to_string())
    }
}

/// 提问待决登记（用户提交后经 `submitInterrupt` 交回；冷启动由 metadata.interrupt 恢复）。
fn register_pending_form(ctx: &ToolContext, request: &serde_json::Value) {
    let Some(request_id) = request.get("request_id").and_then(|v| v.as_str()) else {
        return;
    };
    crate::tool::policy::register_pending_interrupt(
        &ctx.session_id,
        crate::tool::interrupt::PendingInterrupt {
            request_id: request_id.to_string(),
            interrupt_type: astorm_common::session::InterruptType::Form,
            tool_call_id: ctx.tool_call_id.clone(),
            tool_name: "chat_with_user".into(),
            request: request.clone(),
            command: None,
            requested_at: crate::agent::session::unix_now(),
        },
    );
}
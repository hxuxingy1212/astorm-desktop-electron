//! `tool_load` 工具：按需加载延迟工具（定时任务 / 邮件 / 内置浏览器等）。
//!
//! 重承载 / 少用的工具初始**不下发** LLM（见 `agent::dynamic_tools::DYNAMIC_TOOLS`），
//! 模型根据上下文判断需要时调用本工具显式加载：写进会话记录的 `loaded_tool_list`
//! （持久化，重启不丢），**下一次回复**起携带其 schema（本轮不重建请求）。
//! `unload` 可移除加载态；`status` 自省当前加载情况。
//!
//! 可加载清单是**清单驱动**的通用机制：新增延迟加载工具时在 `DYNAMIC_TOOLS` 登记 +
//! 在 [`DYNAMIC_TOOL_HELP`] 补一行用途（单测保证两处同步）。

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::agent::dynamic_tools::DYNAMIC_TOOL_HELP;
use crate::tool::{Tool, ToolContext};

/// 渲染可加载清单（含用途），供 description 与错误文案共用。
fn dynamic_tool_help() -> String {
    DYNAMIC_TOOL_HELP
        .iter()
        .map(|(name, help)| format!("{name}（{help}）"))
        .collect::<Vec<_>>()
        .join("；")
}

/// `tool_load` 工具。
pub struct ToolLoadTool;

/// 工具参数。
#[derive(Debug, Deserialize)]
struct ToolLoadArgs {
    /// 操作：load 加载（默认）/ unload 移除 / status 查询。
    #[serde(default)]
    action: Option<String>,
    /// 要加载 / 移除的工具名（status 时可省）。
    #[serde(default)]
    tools: Option<Vec<String>>,
}

#[async_trait]
impl Tool for ToolLoadTool {
    fn name(&self) -> &str {
        "tool_load"
    }

    fn description(&self) -> &str {
        "按需加载延迟工具：定时任务、邮件、内置浏览器等工具初始不在可用工具列表里，需要时调用本工具加载（如用户提到定时 / 提醒 / 邮件 / 网页操作）。加载后下一次回复起该工具即可调用，无需重复加载；用 unload 可移除，status 可查看当前加载情况。"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["load", "unload", "status"],
                    "description": "操作类型：load 加载（默认）/ unload 移除加载态 / status 查询已加载与可加载清单"
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "要加载 / 移除的工具名（load / unload 时必填；status 时省略）"
                }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<String, String> {
        let args: ToolLoadArgs =
            serde_json::from_value(params).map_err(|e| format!("参数错误: {e}"))?;
        let action = args.action.as_deref().unwrap_or("load");
        let session_id = ctx.session_id.clone();

        let app = crate::app::App::try_global().ok_or("应用未初始化，无法变更工具加载态")?;

        match action {
            "status" => {
                let loaded = app.session().loaded_tools_of(&session_id);
                let available: Vec<&str> = crate::agent::dynamic_tools::DYNAMIC_TOOLS
                    .iter()
                    .copied()
                    .filter(|n| !loaded.iter().any(|l| l == n))
                    .collect();
                return Ok(json!({
                    "loaded": loaded,
                    "available": available,
                    "help": dynamic_tool_help(),
                })
                .to_string());
            }
            "load" | "unload" => {
                let names = args.tools.clone().unwrap_or_default();
                if names.is_empty() {
                    return Err(format!(
                        "tools 必填（可加载: {}）",
                        crate::agent::dynamic_tools::DYNAMIC_TOOLS.join("、")
                    ));
                }
                let names: Vec<String> = names.into_iter().map(|s| s.trim().to_string()).collect();
                let (loaded, newly) =
                    app.session()
                        .mutate_loaded_tools(&session_id, &names, action == "load")?;
                let available: Vec<&str> = crate::agent::dynamic_tools::DYNAMIC_TOOLS
                    .iter()
                    .copied()
                    .filter(|n| !loaded.iter().any(|l| l == n))
                    .collect();
                let mut receipt = json!({
                    "ok": true,
                    "action": action,
                    "loaded": loaded,
                    "available": available,
                });
                if action == "load" {
                    receipt["newly_loaded"] = json!(newly);
                    receipt["effective"] = json!("下一轮回复起可调用（本轮尚未携带其 schema）");
                }
                Ok(receipt.to_string())
            }
            other => Err(format!(
                "不支持的操作: {other}（可用: load / unload / status）"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用途表与可加载清单必须一一对应（清单驱动机制的两处登记不许漂移）。
    #[test]
    fn help_table_matches_dynamic_tools() {
        let help_names: Vec<&str> = DYNAMIC_TOOL_HELP.iter().map(|(n, _)| *n).collect();
        let dynamic: Vec<&str> = crate::agent::dynamic_tools::DYNAMIC_TOOLS.to_vec();
        assert_eq!(help_names, dynamic);
        // 渲染非空且每项带用途说明
        let rendered = dynamic_tool_help();
        for (name, _) in DYNAMIC_TOOL_HELP {
            assert!(rendered.contains(name), "清单应渲染 {name}");
        }
    }
}

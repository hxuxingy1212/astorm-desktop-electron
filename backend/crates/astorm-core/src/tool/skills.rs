//! `skills` 工具：技能管理（load 加载指令 / search 全文检索 / register 注册导入）。
//!
//! 经 `App` 持有的 [`SkillManager`] 访问技能索引（不直接触碰索引锁）。

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

use astorm_common::session::types::SessionMode;

use crate::app::App;
use crate::skill::{skill_dir_of, SkillDoc, SkillManager, SkillSummary};
use crate::tool::{expand_session_vars, resolve_workspace, session_mode, ToolContext};

/// `skills` 工具。
pub struct SkillsTool;

#[async_trait]
impl crate::tool::Tool for SkillsTool {
    fn name(&self) -> &str {
        "skills"
    }

    fn description(&self) -> &str {
        "技能管理工具。支持三种操作：\n 1. load — 加载技能的完整指令，参数 skills 为技能名称列表\n 2. search — 全文搜索已注册的技能，按相关性返回技能名称和描述\n 3. register — 从路径导入技能并注册：校验目录结构（<目录>/SKILL.md 或 <目录>/<技能名>/SKILL.md）后复制到应用技能目录并索引元数据，参数 path 支持目录或 SKILL.md 文件"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["load", "search", "register"],
                    "description": "操作类型：load=加载技能指令，search=搜索技能，register=导入并注册技能"
                },
                "skills": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "要加载的技能名称列表（load 模式使用）"
                },
                "query": { "type": "string", "description": "搜索关键词（search 模式使用）" },
                "size": { "type": "integer", "description": "最大返回数，默认 5（search 模式使用）" },
                "threshold": { "type": "number", "description": "相关度阈值，默认 0.5，低于此值不返回（search 模式使用）" },
                "path": {
                    "type": "string",
                    "description": "技能路径（register 模式使用）：单个技能目录（含 SKILL.md）、SKILL.md 文件本身，或包含多个 <技能名>/SKILL.md 子目录的集合目录；支持 $SESSION_WORKSPACE 变量"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<String, String> {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("load");
        let manager = App::try_global()
            .map(|app| Arc::clone(&app.skill))
            .ok_or_else(|| "App 未初始化，无法访问技能索引".to_string())?;
        match action {
            "load" => execute_load(&params, &manager),
            "search" => execute_search(&params, &manager),
            "register" => execute_register(&params, ctx, &manager),
            _ => Err(format!("不支持的技能操作: {action}（可用: load / search / register）")),
        }
    }
}

/// load：按名称列表加载技能完整指令。
fn execute_load(params: &Value, manager: &SkillManager) -> Result<String, String> {
    let skills = params
        .get("skills")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "skills 参数必须是一个数组".to_string())?;
    if skills.is_empty() {
        return Err("skills 参数不能为空".into());
    }

    let mut parts = Vec::new();
    for name in skills {
        let name = name.as_str().ok_or_else(|| "skills 数组元素必须是字符串".to_string())?;
        if let Some(doc) = manager.get_by_name(name)? {
            parts.push(render_loaded_skill(&doc));
        }
    }
    if parts.is_empty() {
        return Ok("未找到指定的技能文件".into());
    }
    Ok(parts.join("\n\n---\n\n"))
}

/// 渲染一个已加载的技能：**技能目录必须给出**。
///
/// 技能正文里的资产引用（`scripts/`、`references/`、`assets/`）都是相对技能目录的，
/// 只给正文会让模型按技能名去猜路径（曾编造出 `…/astorm-ppt-generator/astorm-ppt-generator`）。
/// 源路径异常（取不到父目录）时省略该行，不伪造。
fn render_loaded_skill(doc: &SkillDoc) -> String {
    let mut head = format!("## {}", doc.name);
    if let Some(dir) = skill_dir_of(&doc.source_path) {
        head.push_str(&format!(
            "\n技能目录：{dir}\n（技能引用的 scripts/、references/、assets/ 等资产都在该目录下，直接用该目录拼绝对路径；不要凭技能名猜测路径）"
        ));
    }
    format!("{head}\n\n{}", doc.prompt)
}

/// search：按名称 / 描述与正文双路全文检索，过滤低相关度结果。
fn execute_search(params: &Value, manager: &SkillManager) -> Result<String, String> {
    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
    if query.is_empty() {
        return Err("query 不能为空".into());
    }
    let size = params.get("size").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    let threshold = params.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.5);

    let by_name = render_hits(manager.search(query, size)?, threshold);
    let by_content = render_hits(manager.search_content(query, size)?, threshold);

    if by_name.is_empty() && by_content.is_empty() {
        return Ok(format!("未找到与「{query}」相关的技能"));
    }
    let mut out = Vec::new();
    if !by_name.is_empty() {
        out.push(format!("【名称/描述匹配】\n{}", by_name.join("\n")));
    }
    if !by_content.is_empty() {
        out.push(format!("【内容匹配】\n{}", by_content.join("\n")));
    }
    Ok(out.join("\n\n"))
}

/// register：从路径导入技能并注册元数据（复制到应用技能目录 + 建索引，幂等覆盖）。
///
/// path 支持：单个技能目录（其下有 SKILL.md）、SKILL.md 文件本身、
/// 或包含多个 `<技能名>/SKILL.md` 子目录的集合目录；支持 `$SESSION_WORKSPACE` 变量。
fn execute_register(
    params: &Value,
    ctx: &ToolContext,
    manager: &SkillManager,
) -> Result<String, String> {
    // Plan 模式只允许只读操作（load / search）
    if session_mode(&ctx.session_id) == SessionMode::Plan {
        return Err(
            "当前模式为 Plan，不允许注册技能。Plan 模式只允许查看技能（load / search），请切换到 Execute 或 Auto 模式后重试".into(),
        );
    }
    let raw = params
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if raw.is_empty() {
        return Err("path 参数不能为空（register 模式使用）".into());
    }
    let workspace = resolve_workspace(&ctx.session_id);
    let path = expand_session_vars(raw, &ctx.session_id, &workspace);
    if !Path::new(&path).exists() {
        return Err(format!(
            "路径不存在: {path}。期望的结构为 <目录>/SKILL.md（单个技能）或 <目录>/<技能名>/SKILL.md（技能集合）"
        ));
    }

    let (imported, errors) = manager.register_path(&path)?;
    let mut out = Vec::new();
    if !imported.is_empty() {
        let lines: Vec<String> = imported
            .iter()
            .map(|s| format!("- {} — {}", s.name, s.description))
            .collect();
        out.push(format!("已注册 {} 个技能：\n{}", imported.len(), lines.join("\n")));
        out.push(
            "技能已复制到应用技能目录并完成索引（默认启用），可在「设置 → 技能」管理，会话中经 skills 工具 load 使用".into(),
        );
    }
    if !errors.is_empty() {
        let lines: Vec<String> = errors.iter().map(|e| format!("- {e}")).collect();
        out.push(format!("以下 {} 项导入失败：\n{}", errors.len(), lines.join("\n")));
    }
    Ok(out.join("\n\n"))
}

fn render_hits(hits: Vec<SkillSummary>, threshold: f64) -> Vec<String> {
    hits.into_iter()
        .filter(|s| s.score.unwrap_or(0.0) as f64 >= threshold)
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}. {} — {} [相关性: {:.2}]",
                i + 1,
                s.name,
                s.description,
                s.score.unwrap_or(0.0)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source_path: &str) -> SkillDoc {
        SkillDoc {
            name: "astorm-ppt-generator".into(),
            description: "生成 PPT".into(),
            prompt: "读 references/schema.json".into(),
            category: "builtin".into(),
            tags: vec![],
            version: "0.1.0".into(),
            author: "x".into(),
            source: "user".into(),
            source_path: source_path.into(),
            enabled: true,
            updated_at: 0,
        }
    }

    #[test]
    fn load_输出必须带上技能目录() {
        // 只给正文会让模型按技能名猜路径（曾编造 …/X/X 这种重复末段路径）
        let out = render_loaded_skill(&doc("/Users/x/skills/astorm-ppt-generator/SKILL.md"));
        assert!(
            out.contains("技能目录：/Users/x/skills/astorm-ppt-generator"),
            "应给出 SKILL.md 所在目录: {out}"
        );
        assert!(out.contains("读 references/schema.json"), "正文应保留: {out}");
    }

    #[test]
    fn source_path_异常时不伪造技能目录() {
        let out = render_loaded_skill(&doc("SKILL.md"));
        assert!(!out.contains("技能目录"), "取不到父目录时不应出现该行: {out}");
        let out = render_loaded_skill(&doc(""));
        assert!(!out.contains("技能目录"), "源路径为空时不应出现该行: {out}");
    }
}

//! 动态工具加载（`tool_load`）：LLM 实际可用工具集的计算与加载态变更（纯逻辑）。
//!
//! 设计（见会话级「按需加载」约定）：
//! - **可加载集** [`DYNAMIC_TOOLS`]：重承载 / 少用工具（定时任务、邮件、内置浏览器），
//!   初始**不下发** LLM，模型需要时经 `tool_load` 工具显式加载；
//! - 会话记录持有两个清单：`tool_list` = 智能体卡片物化的**可见工具集**（语义不变，
//!   冷启动自愈覆写它）；`loaded_tool_list` = 经 `tool_load` 显式加载的工具；
//! - **LLM 实际可用集** = `tool_list ∪ loaded_tool_list`，与注册中心取交集（防幻影名）；
//!   加载态持久化在会话记录，进程重启不丢；
//! - 下一轮生效：`tool_load` 执行时只写记录，本轮不重建 `ctx.tools`（引擎每轮请求从
//!   `ctx.tools` 导出 schema）；会话星的 `loop_start` hook 每轮按记录重建 → 下一轮携带。
//!   加载后的首轮拿到新工具并回给最终答复。
//!
//! 纯函数集中于此，便于单测（`ToolMap` / 会话锁不参与）。

/// 需经 `tool_load` 显式加载的工具（首批：定时任务 / 邮件 / 内置浏览器；可持续追加）。
///
/// `tool_load` 自身**不**进清单（始终下发）。清单里的工具必须仍注入注册中心
/// （`ToolRegistry::builtin`），否则加载报「工具不存在」。
pub const DYNAMIC_TOOLS: &[&str] = &["schedule_task", "mail", "browser"];

/// 可加载工具的用途说明（name → 一句话用途；键必须与 [`DYNAMIC_TOOLS`] 一致，单测保证）。
/// 供 `tool_load` 的描述与系统提示词「可加载工具」段共用。
pub const DYNAMIC_TOOL_HELP: &[(&str, &str)] = &[
    ("schedule_task", "定时任务：创建 / 管理 Cron 定时任务（到点自动开会话执行）"),
    ("mail", "邮件：收取 / 阅读 / 发送邮件"),
    ("browser", "内置浏览器：AI 代理操控网页（打开 / 点击 / 填表 / 截图）"),
];

/// 判断某工具是否属「按需加载」集。
pub fn is_dynamic_tool(name: &str) -> bool {
    DYNAMIC_TOOLS.contains(&name)
}

/// **LLM 实际可用工具集**（构建循环 / 每轮重建 / 展示的统一口径）：
/// `可见集 − 可加载集 ∪ 已加载集`（去重保序；与注册中心的交集由调用方 `tool_map_for`
/// 内部完成——幻影名静默忽略）。
///
/// 未加载的可加载工具不在其中（初始不给 LLM）；`tool_load` 加载后进入 `loaded_tool_list`
/// → 下一轮起出现在这里。可见集语义不变（含可加载集，供卡片物化 / 自愈），加载态单独持久化。
pub fn effective_tool_names(tool_list: &[String], loaded: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tool_list.len() + loaded.len());
    for name in tool_list
        .iter()
        .filter(|n| !is_dynamic_tool(n))
        .chain(loaded.iter())
    {
        if out.iter().any(|o| o == name) {
            continue;
        }
        out.push(name.clone());
    }
    out
}

/// 与注册中心取交集后的展示口径（环境提示词「可用工具」行用）。
pub fn llm_tool_names(tool_list: &[String], loaded: &[String], known: &[String]) -> Vec<String> {
    effective_tool_names(tool_list, loaded)
        .into_iter()
        .filter(|n| known.iter().any(|k| k == n))
        .collect()
}

/// 已加载集清洗：剔除不在 `known`（注册中心现有名）里的幻影名，返回 `(清洗后, 是否变化)`。
///
/// 幻影来源：清单往后剔除某工具（如浏览器下线）后，老会话记录里还留着旧名——
/// 留着会让 `tool_map_for` 静默忽略（无害），但环境提示词的「可用工具」会显示模型
/// 实际没有的工具，故在自愈 / 展示口径统一清洗。
pub fn sanitize_loaded(loaded: &[String], known: &[String]) -> (Vec<String>, bool) {
    let cleaned: Vec<String> = loaded
        .iter()
        .filter(|n| known.iter().any(|k| k == *n))
        .cloned()
        .collect();
    let changed = cleaned.len() != loaded.len();
    (cleaned, changed)
}

/// 应用一次 `tool_load` 的 load 请求（纯逻辑）：校验 + 去重 + 合并，返回新已加载集。
///
/// 不是可加载集的名称报错（错误文案附**当前可加载清单**，模型据此纠正）；
/// 全部已在加载集中也返回成功状态（幂等，不重复追加）。
pub fn apply_load(
    requested: &[String],
    loaded: &[String],
    known: &[String],
) -> Result<(Vec<String>, Vec<String>, Vec<String>), String> {
    let mut newly: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut merged = loaded.to_vec();
    for name in requested {
        if !known.iter().any(|k| k == name) {
            unknown.push(name.clone());
            continue;
        }
        if !is_dynamic_tool(name) {
            unknown.push(name.clone());
            continue;
        }
        if merged.iter().any(|m| m == name) {
            continue;
        }
        merged.push(name.clone());
        newly.push(name.clone());
    }
    if !unknown.is_empty() {
        return Err(format!(
            "这些工具不在可加载清单（可加载: {}）：{}",
            DYNAMIC_TOOLS.join("、"),
            unknown.join("、")
        ));
    }
    Ok((merged, newly, loaded.to_vec()))
}

/// 应用一次 `tool_load` 的 unload 请求：从加载集移除（未加载的忽略），返回新加载集。
pub fn apply_unload(requested: &[String], loaded: &[String]) -> Vec<String> {
    loaded
        .iter()
        .filter(|n| !requested.iter().any(|r| r == *n))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn initial_excludes_dynamic_tools() {
        let tool_list = set(&["bash", "schedule_task", "mail", "browser", "memory", "tool_load"]);
        // 从零（无加载态）= 可见集剔除可加载集
        assert_eq!(
            effective_tool_names(&tool_list, &set(&[])),
            set(&["bash", "memory", "tool_load"])
        );
        // 重启后新循环：已加载态生效（可见集仍含三工具）
        assert_eq!(
            effective_tool_names(&tool_list, &set(&["schedule_task", "mail"])),
            set(&["bash", "memory", "tool_load", "schedule_task", "mail"])
        );
    }

    #[test]
    fn llm_names_union_dedupe_and_filter() {
        let tool_list = set(&["bash", "schedule_task", "ghost"]);
        let loaded = set(&["schedule_task", "mail"]);
        let known = set(&["bash", "schedule_task", "mail", "memory"]);
        // ghost 不在注册中心，清洗；schedule_task 去重
        assert_eq!(
            llm_tool_names(&tool_list, &loaded, &known),
            set(&["bash", "schedule_task", "mail"])
        );
    }

    #[test]
    fn sanitize_drops_unknown_names() {
        let (cleaned, changed) = sanitize_loaded(
            &set(&["schedule_task", "browser", "removed_tool"]),
            &set(&["bash", "schedule_task", "browser", "memory"]),
        );
        assert_eq!(cleaned, set(&["schedule_task", "browser"]));
        assert!(changed);
        let (same, unchanged) = sanitize_loaded(&set(&["mail"]), &set(&["mail", "bash"]));
        assert!(!unchanged && same == set(&["mail"]));
    }

    #[test]
    fn apply_load_merges_idempotent_and_rejects_unknown() {
        let known = set(&["schedule_task", "mail", "browser", "bash"]);
        // 新加载 + 幂等重载
        let (merged, newly, _) =
            apply_load(&set(&["schedule_task", "mail", "schedule_task"]), &set(&[]), &known).unwrap();
        assert_eq!(merged, set(&["schedule_task", "mail"]));
        assert_eq!(newly, set(&["schedule_task", "mail"]));
        let (merged2, newly2, _) =
            apply_load(&set(&["schedule_task"]), &set(&["schedule_task"]), &known).unwrap();
        assert!(newly2.is_empty() && merged2 == set(&["schedule_task"]));
        // 非清单工具与未知工具都拒绝：错误文案带可加载清单
        let err = apply_load(&set(&["bash", "nope"]), &set(&[]), &known).unwrap_err();
        assert!(err.contains("schedule_task") && err.contains("mail") && err.contains("browser"));
    }

    #[test]
    fn apply_unload_removes_and_ignores_missing() {
        let out = apply_unload(&set(&["mail", "ghost"]), &set(&["schedule_task", "mail"]));
        assert_eq!(out, set(&["schedule_task"]));
    }
}

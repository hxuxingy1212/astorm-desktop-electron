//! 智能体管理器：[`AgentRegistry`]——智能体卡片（工具集 / 技能集 / 额外系统指引）的
//! 持久化与解析。
//!
//! 与 `crate::agent`（ReAct 循环）无关：本模块只管「能力配置」这一层。
//!
//! - 落地：`Storage` 的 `agents` 表（sqlite `data` 列整条 JSON），进程不缓存，
//!   每次读取即最新（卡片编辑后新会话立即生效）。
//! - 内置卡片：启动时由 [`AgentRegistry::ensure_builtin`] 播种，**不可编辑 / 删除**
//!   （仿技能系统的「来源标记 + 管理器层拒绝写删」，见 `skill::manager`）。
//! - 解析：[`AgentRegistry::resolve`] 把会话请求里的 `agentType` 解析为卡片，
//!   缺省 = 内置默认主智能体（= 不做工具 / 技能限制，与引入智能体前的行为一致）。

use std::sync::Arc;

use astorm_common::agent::req::{AgentUpdate, CreateAgentRequest};
use astorm_common::types::agent::{
    AgentRecord, BUILTIN_MAIN_AGENT_ID, builtin_explore_agent, builtin_general_agent,
    builtin_main_agent,
};
use astorm_common::types::Storage;

use crate::agent::session::unix_now;

/// 智能体管理器（挂在 `AppState`，经 `Storage` 持久化）。
pub struct AgentRegistry {
    storage: Arc<dyn Storage>,
}

impl AgentRegistry {
    /// 由存储门面构建。
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    /// 播种内置智能体（启动时调用）：默认主智能体（不限制）+ Explore（只读探索）
    /// + General（通用执行，可写）。
    ///
    /// 已存在的内置卡片**内容随版本刷新**（仿内建技能 `skill-creator` 的做法：卡片内容由
    /// 代码内嵌，随版本分发；内置卡片本就不允许用户编辑，刷新不会覆盖用户数据），
    /// 只保留原 `created_at` 使列表顺序与展示稳定。
    ///
    /// 存储后端不支持 `agents` 表（如 JSONL）时返回 `Err`：调用方告警即可，
    /// [`AgentRegistry::resolve`] 对内置主智能体有内存回退，功能不受影响。
    pub fn ensure_builtin(&self) -> Result<(), String> {
        let now = unix_now();
        for seed in [
            builtin_main_agent(now),
            builtin_explore_agent(now),
            builtin_general_agent(now),
        ] {
            let existing = self.get(&seed.agent_id)?;
            let should_install = existing.as_ref().map(|c| c.builtin).unwrap_or(true);
            if !should_install {
                // 同 id 的用户卡片（极端情况：手工改库）不覆盖
                tracing::warn!("[agents] 内置 id 已被非内置卡片占用，跳过播种: {}", seed.agent_id);
                continue;
            }
            let mut card = seed;
            if let Some(old) = existing {
                card.created_at = old.created_at;
            }
            self.storage
                .upsert_agent(&card)
                .map_err(|e| format!("播种内置智能体失败: {e}"))?;
        }
        Ok(())
    }

    /// 全部智能体（内置置前，其余按创建时间）。
    pub fn list(&self) -> Result<Vec<AgentRecord>, String> {
        self.storage.list_agents().map_err(|e| format!("读取智能体列表失败: {e}"))
    }

    /// 单个智能体（不存在 → None）。
    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, String> {
        self.storage.get_agent(agent_id).map_err(|e| format!("读取智能体失败: {e}"))
    }

    /// 新建智能体（名称必填 / 唯一）。
    pub fn create(&self, request: CreateAgentRequest) -> Result<AgentRecord, String> {
        let name = request.name.trim().to_string();
        if name.is_empty() {
            return Err("智能体名称不能为空".into());
        }
        self.ensure_name_free(&name, None)?;
        let now = unix_now();
        let record = AgentRecord {
            agent_id: uuid::Uuid::new_v4().to_string(),
            name,
            description: request.description,
            guidance: request.guidance,
            tool_ids: dedup(request.tool_ids),
            skill_ids: dedup(request.skill_ids),
            mode: request.mode,
            permission: request.permission,
            builtin: false,
            created_at: now,
            updated_at: now,
        };
        self.storage
            .upsert_agent(&record)
            .map_err(|e| format!("智能体落盘失败: {e}"))?;
        Ok(record)
    }

    /// 更新智能体（缺省字段保持原值；内置卡片拒绝；空名字段视为未给出）。
    pub fn update(&self, agent_id: &str, updates: AgentUpdate) -> Result<AgentRecord, String> {
        let mut record = self
            .get(agent_id)?
            .ok_or_else(|| format!("智能体不存在: {agent_id}"))?;
        if record.builtin {
            return Err("内置智能体不可修改".into());
        }
        if let Some(name) = updates.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            self.ensure_name_free(name, Some(agent_id))?;
            record.name = name.to_string();
        }
        if let Some(v) = updates.description {
            record.description = v;
        }
        if let Some(v) = updates.guidance {
            record.guidance = v;
        }
        if let Some(v) = updates.tool_ids {
            record.tool_ids = dedup(v);
        }
        if let Some(v) = updates.skill_ids {
            record.skill_ids = dedup(v);
        }
        if let Some(v) = updates.mode {
            record.mode = v;
        }
        if let Some(v) = updates.permission {
            record.permission = v;
        }
        record.updated_at = unix_now();
        self.storage
            .upsert_agent(&record)
            .map_err(|e| format!("智能体落盘失败: {e}"))?;
        Ok(record)
    }

    /// 删除智能体（内置卡片拒绝）。
    ///
    /// 已用该智能体创建的会话不受影响：其 `tool_list` / `skill_list` 是创建时的快照
    /// （见 `agent::session::create_session`），卡片删除后仍按快照运行。
    pub fn remove(&self, agent_id: &str) -> Result<(), String> {
        let record = self
            .get(agent_id)?
            .ok_or_else(|| format!("智能体不存在: {agent_id}"))?;
        if record.builtin {
            return Err("内置智能体不可删除".into());
        }
        self.storage
            .delete_agent(agent_id)
            .map_err(|e| format!("智能体删除失败: {e}"))
    }

    /// 解析会话请求里的智能体类型为卡片。
    ///
    /// - `None`（会话未指定）→ 内置默认主智能体；若存储里没有该卡片（后端不支持 agents 表 /
    ///   未播种）则返回内存构造的同一卡片，保证「不指定 = 不限制」始终成立。
    /// - `Some(id)` → 必须是已存在卡片，否则报错（附现有智能体列表，便于前端 / 模型纠正）。
    pub fn resolve(&self, agent_type: Option<&str>) -> Result<AgentRecord, String> {
        let Some(agent_id) = agent_type.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(self
                .get(BUILTIN_MAIN_AGENT_ID)?
                .unwrap_or_else(|| builtin_main_agent(0)));
        };
        match self.get(agent_id)? {
            Some(record) => Ok(record),
            None => {
                let available: Vec<String> = self
                    .list()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| format!("{}（{}）", a.name, a.agent_id))
                    .collect();
                Err(format!(
                    "智能体不存在: {agent_id}（可用: {}）",
                    if available.is_empty() { "无".to_string() } else { available.join(", ") }
                ))
            }
        }
    }

    /// 名称唯一性校验（`exclude` = 更新时排除自身）。
    fn ensure_name_free(&self, name: &str, exclude: Option<&str>) -> Result<(), String> {
        let exists = self
            .list()?
            .into_iter()
            .any(|a| a.name == name && Some(a.agent_id.as_str()) != exclude);
        if exists {
            return Err(format!("智能体名称已存在: {name}"));
        }
        Ok(())
    }
}

/// 去重（保持顺序；卡片里的工具 / 技能名重复无意义）。
fn dedup(mut items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use astorm_common::types::agent::{
        AgentExecMode, AgentPermissionMode, BUILTIN_EXPLORE_AGENT_ID, BUILTIN_GENERAL_AGENT_ID,
    };

    /// 内存存储：只实现 agents 表（其余方法用 trait 默认实现）。
    #[derive(Default)]
    struct MemStorage {
        agents: std::sync::Mutex<Vec<AgentRecord>>,
    }

    impl Storage for MemStorage {
        fn upsert_session(&self, _: &astorm_common::types::SessionRecord) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn get_session(&self, _: &str) -> Result<Option<astorm_common::types::SessionRecord>, astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn list_sessions(&self) -> Result<Vec<astorm_common::types::SessionRecord>, astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn delete_session(&self, _: &str) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn append_messages(&self, _: &str, _: &[astorm_common::types::MessageRecord]) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn load_messages(&self, _: &str) -> Result<Vec<astorm_common::types::MessageRecord>, astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn rewrite_live_messages(&self, _: &str, _: u32, _: &[astorm_common::types::MessageRecord]) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn search(&self, _: &str, _: usize) -> Result<Vec<astorm_common::types::SearchHit>, astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn upsert_plan(&self, _: &str, _: &astorm_common::types::PlanRecord) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn get_plan(&self, _: &str, _: &str) -> Result<Option<astorm_common::types::PlanRecord>, astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn delete_plan(&self, _: &str, _: &str) -> Result<(), astorm_common::types::StorageError> {
            unimplemented!()
        }
        fn list_agents(&self) -> Result<Vec<AgentRecord>, astorm_common::types::StorageError> {
            let list = self.agents.lock().unwrap();
            let mut out = list.clone();
            out.sort_by_key(|a| !a.builtin);
            Ok(out)
        }
        fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, astorm_common::types::StorageError> {
            Ok(self.agents.lock().unwrap().iter().find(|a| a.agent_id == agent_id).cloned())
        }
        fn upsert_agent(&self, agent: &AgentRecord) -> Result<(), astorm_common::types::StorageError> {
            let mut list = self.agents.lock().unwrap();
            list.retain(|a| a.agent_id != agent.agent_id);
            list.push(agent.clone());
            Ok(())
        }
        fn delete_agent(&self, agent_id: &str) -> Result<(), astorm_common::types::StorageError> {
            self.agents.lock().unwrap().retain(|a| a.agent_id != agent_id);
            Ok(())
        }
    }

    fn registry() -> AgentRegistry {
        let reg = AgentRegistry::new(Arc::new(MemStorage::default()));
        reg.ensure_builtin().expect("播种内置智能体");
        reg
    }

    fn req(name: &str) -> CreateAgentRequest {
        CreateAgentRequest { name: name.into(), ..Default::default() }
    }

    #[test]
    fn 缺省解析为内置主智能体且不限制工具技能() {
        let reg = registry();
        let card = reg.resolve(None).expect("解析缺省智能体");
        assert_eq!(card.agent_id, BUILTIN_MAIN_AGENT_ID);
        assert!(card.is_builtin_main());
        assert!(card.tool_ids.is_empty() && card.skill_ids.is_empty());
        // 空串 / 纯空白同样视为未指定
        assert!(reg.resolve(Some("  ")).unwrap().is_builtin_main());
    }

    #[test]
    fn 内置卡片不可改不可删() {
        let reg = registry();
        let err = reg.update(BUILTIN_MAIN_AGENT_ID, AgentUpdate { name: Some("改名".into()), ..Default::default() });
        assert!(err.unwrap_err().contains("内置"));
        let err = reg.remove(BUILTIN_MAIN_AGENT_ID);
        assert!(err.unwrap_err().contains("内置"));
    }

    /// 内置 Explore：工具集 = 文件 / 搜索 / 图片 / 文档解析，且这些名字在当前注册中心里真实存在。
    ///
    /// 工具集按**名字**配置，改工具名会静默失效（物化时取交集）——本用例把卡片里的名字与
    /// 注册中心的名字钉在一起，改名时立刻红。
    #[test]
    fn 内置探索智能体的工具名都真实存在() {
        let reg = registry();
        let explore = reg.resolve(Some(BUILTIN_EXPLORE_AGENT_ID)).expect("解析 Explore");
        assert!(explore.builtin, "Explore 应为内置（不可编辑 / 删除）");
        assert_eq!(explore.name, "Explore");
        // 只读由卡片执行模式承载（会话创建时落 Plan，工具层硬拒写）；权限自动 = 探索期读不弹卡
        assert_eq!(explore.mode, AgentExecMode::ReadOnly);
        assert!(explore.mode.is_read_only());
        assert_eq!(explore.permission, AgentPermissionMode::Auto);
        // 内置主智能体不干预模式与权限（保持引入智能体前的行为）
        let main = reg.resolve(None).unwrap();
        assert_eq!(main.mode, AgentExecMode::Inherit);
        assert_eq!(main.permission, AgentPermissionMode::Inherit);

        let registered = crate::tool::ToolRegistry::builtin().names();
        for name in &explore.tool_ids {
            assert!(
                registered.iter().any(|r| r == name),
                "Explore 的工具 `{name}` 不在当前注册中心: {registered:?}"
            );
        }
        // 只读语义来自工具集：不含 shell（可任意写）；memory 也是只读检索工具
        assert!(!explore.tool_ids.iter().any(|t| t == "bash"), "Explore 不应有 bash");
        for expected in ["file_operation", "search", "read_images", "memory"] {
            assert!(
                explore.tool_ids.iter().any(|t| t == expected),
                "Explore 应含 {expected}: {:?}",
                explore.tool_ids
            );
        }
        // 物化取交集依赖 App 全局（本用例未装配 App），故此处只钉「名字存在 + 集合内容」；
        // 物化后的可见工具集由 binding 冒烟测试覆盖。
        assert_eq!(explore.tool_ids.len(), 4, "Explore 工具集应恰好是这 4 个");
    }

    /// 执行模式 / 权限模式 → 会话模式 / 权限模式的映射（`继承` = 不干预）。
    #[test]
    fn 卡片模式映射到会话模式() {
        use astorm_common::session::types::{PermissionMode, SessionMode};

        assert_eq!(AgentExecMode::ReadOnly.to_session(), Some(SessionMode::Plan), "只读 → plan（写硬拒）");
        assert_eq!(AgentExecMode::ReadWrite.to_session(), Some(SessionMode::Execute));
        assert_eq!(AgentExecMode::Auto.to_session(), Some(SessionMode::Auto));
        assert_eq!(AgentExecMode::Inherit.to_session(), None, "继承 = 不干预");
        assert!(AgentExecMode::ReadOnly.is_read_only());
        assert!(!AgentExecMode::Auto.is_read_only());

        assert_eq!(AgentPermissionMode::Manual.to_session(), Some(PermissionMode::Manual));
        assert_eq!(AgentPermissionMode::Auto.to_session(), Some(PermissionMode::Auto));
        assert_eq!(AgentPermissionMode::Inherit.to_session(), None, "继承 = 不干预");
    }

    /// 内置 General：完整工具访问（除任务规划），可写、并行工作单元用。
    #[test]
    fn 内置通用智能体的工具集与模式() {
        let reg = registry();
        let general = reg.resolve(Some(BUILTIN_GENERAL_AGENT_ID)).expect("解析 General");
        assert!(general.builtin, "General 应为内置");
        assert_eq!(general.name, "General");

        let registered = crate::tool::ToolRegistry::builtin().names();
        for name in &general.tool_ids {
            assert!(
                registered.iter().any(|r| r == name),
                "General 的工具 `{name}` 不在当前注册中心: {registered:?}"
            );
        }
        // 「完整访问（除任务规划）」：不含 task_list；也不含 sub_session（嵌套被禁止，带上只会报错）
        assert!(!general.tool_ids.iter().any(|t| t == "task_list"), "General 不应有 task_list");
        assert!(!general.tool_ids.iter().any(|t| t == "sub_session"), "General 不应有 sub_session");
        // 可写：有文件操作与 bash（与 Explore 的只读四件套相对）
        for expected in ["file_operation", "bash", "browser", "tabs"] {
            assert!(general.tool_ids.iter().any(|t| t == expected), "General 应含 {expected}");
        }
        // 执行模式与权限模式都继承主会话（写不写、弹不弹卡随会话，不由卡片另设）
        assert_eq!(general.mode, AgentExecMode::Inherit);
        assert_eq!(general.permission, AgentPermissionMode::Inherit);
    }

    /// 内置卡片内容随版本刷新（种子改工具集后重启即生效），且保留创建时间。
    #[test]
    fn 内置卡片内容随版本刷新() {
        let reg = registry();
        let first = reg.get(BUILTIN_EXPLORE_AGENT_ID).unwrap().unwrap();
        let mut stale = first.clone();
        stale.tool_ids = vec!["file_operation".into()];
        reg.storage.upsert_agent(&stale).unwrap();
        assert_eq!(reg.get(BUILTIN_EXPLORE_AGENT_ID).unwrap().unwrap().tool_ids.len(), 1);

        reg.ensure_builtin().expect("重新播种");
        let refreshed = reg.get(BUILTIN_EXPLORE_AGENT_ID).unwrap().unwrap();
        assert_eq!(refreshed.tool_ids.len(), 4, "内容应刷新为当前种子");
        assert_eq!(refreshed.created_at, first.created_at, "创建时间应保留");
    }

    #[test]
    fn 创建名称唯一且未知智能体报错() {
        let reg = registry();
        let card = reg.create(req("研究员")).expect("创建智能体");
        assert!(!card.builtin);
        assert!(reg.create(req("研究员")).is_err(), "同名应拒绝");
        // 名称两端空白归一后再比较
        assert!(reg.create(req("  研究员  ")).is_err());
        let err = reg.resolve(Some("不存在-id")).unwrap_err();
        assert!(err.contains("智能体不存在"), "错误信息应含提示：{err}");
        assert!(err.contains("研究员"), "错误信息应列出可用智能体：{err}");
    }

    #[test]
    fn 更新缺省字段保持原值() {
        let reg = registry();
        let card = reg
            .create(CreateAgentRequest {
                name: "研究员".into(),
                description: "调研".into(),
                guidance: "先查资料".into(),
                tool_ids: vec!["search".into()],
                skill_ids: vec!["writer".into(), "writer".into()],
                ..Default::default()
            })
            .expect("创建智能体");
        assert_eq!(card.skill_ids, vec!["writer".to_string()], "重复项应去重");

        let updated = reg
            .update(&card.agent_id, AgentUpdate { guidance: Some("".into()), ..Default::default() })
            .expect("更新智能体");
        assert_eq!(updated.guidance, "", "给出空串 = 清空指引");
        assert_eq!(updated.description, "调研", "未给出的字段保持原值");
        assert_eq!(updated.tool_ids, vec!["search".to_string()]);
    }
}

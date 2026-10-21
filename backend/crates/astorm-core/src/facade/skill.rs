//! 技能门面实现：委托 `AppState.skill`（[`SkillManager`]）完成搜索 / 发现 / 导入 / 增删改，
//! 会话技能绑定读写 `SessionRecord.skill_list`（经存储门面）。
//! 返回侧用 `Into` 把 core 技能类型转换为 common 门面类型（字段一一对应）。

use astorm_common::skill::{
    CreateSkillRequest, DiscoveredSkill, Skill, SkillDoc, SkillSummary, SkillUpdate,
};

use crate::app::App;

/// 技能门面（`AppState.skills`）。
pub struct SkillImpl;

impl Skill for SkillImpl {
    fn search_skills(&self, query: String, limit: Option<usize>) -> Result<Vec<SkillSummary>, String> {
        App::global()
            .skill
            .search(&query, limit.unwrap_or(20))
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    fn get_skill(&self, name: String) -> Result<SkillDoc, String> {
        App::global()
            .skill
            .get_by_name(&name)?
            .ok_or_else(|| format!("技能不存在: {name}"))
            .map(Into::into)
    }

    fn list_skills(&self, enabled_only: Option<bool>, category: Option<String>) -> Result<Vec<SkillSummary>, String> {
        App::global()
            .skill
            .list(enabled_only.unwrap_or(false), category.as_deref())
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    fn discover_skills(&self) -> Result<Vec<DiscoveredSkill>, String> {
        App::global()
            .skill
            .discover()
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    fn import_skills(&self, paths: Vec<String>) -> Result<Vec<SkillSummary>, String> {
        App::global()
            .skill
            .import_paths(&paths)
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    fn import_skill_package(&self, path: String) -> Result<Vec<SkillSummary>, String> {
        App::global()
            .skill
            .import_archive(&path)
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    fn create_skill(&self, request: CreateSkillRequest) -> Result<SkillSummary, String> {
        App::global().skill.create(&request).map(summarize_doc)
    }

    fn remove_skill(&self, name: String) -> Result<(), String> {
        App::global().skill.remove(&name)
    }

    fn toggle_skill(&self, name: String, enabled: bool) -> Result<SkillSummary, String> {
        App::global().skill.toggle(&name, enabled).map(summarize_doc)
    }

    fn update_skill(&self, name: String, updates: SkillUpdate) -> Result<SkillSummary, String> {
        App::global().skill.update(&name, &updates).map(summarize_doc)
    }

    fn get_session_skills(&self, session_id: String) -> Result<Vec<String>, String> {
        let app = App::try_global().ok_or("应用未初始化：请先调用 start")?;
        let rec = app
            .storage
            .get_session(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("会话不存在: {session_id}"))?;
        Ok(rec.skill_list)
    }

    fn set_session_skills(&self, session_id: String, skill_names: Vec<String>) -> Result<(), String> {
        let app = App::try_global().ok_or("应用未初始化：请先调用 start")?;
        let mut rec = app
            .storage
            .get_session(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("会话不存在: {session_id}"))?;
        rec.skill_list = skill_names;
        app.storage.upsert_session(&rec).map_err(|e| e.to_string())
    }

    fn select_directory(&self) -> Result<Vec<String>, String> {
        // 原生目录选择对话框由 Electron 主进程提供（window.api.dialog），后端不实现
        Err("目录选择请在桌面端对话框完成".into())
    }
}

/// 完整技能文档（core）→ 摘要（common）。
fn summarize_doc(doc: crate::skill::SkillDoc) -> SkillSummary {
    let d: astorm_common::skill::SkillDoc = doc.into();
    SkillSummary {
        name: d.name,
        description: d.description,
        category: d.category,
        version: d.version,
        source: d.source,
        source_path: d.source_path,
        enabled: d.enabled,
        score: None,
    }
}
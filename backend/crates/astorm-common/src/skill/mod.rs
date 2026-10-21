//! 技能接口契约：搜索 / 发现 / 导入 / 增删改 / 会话绑定（纯 serde，不依赖 napi）。

pub mod req;
pub mod types;

pub use req::{CreateSkillRequest, SkillUpdate};
pub use types::{DiscoveredSkill, SkillDoc, SkillSummary};

/// 技能门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait Skill {
    /// 全文检索技能。
    fn search_skills(&self, query: String, limit: Option<usize>) -> Result<Vec<SkillSummary>, String>;
    /// 按名取技能完整文档。
    fn get_skill(&self, name: String) -> Result<SkillDoc, String>;
    /// 列出技能（可按启停 / 分类过滤）。
    fn list_skills(&self, enabled_only: Option<bool>, category: Option<String>) -> Result<Vec<SkillSummary>, String>;
    /// 扫描技能目录发现新技能。
    fn discover_skills(&self) -> Result<Vec<DiscoveredSkill>, String>;
    /// 按路径导入一批技能。
    fn import_skills(&self, paths: Vec<String>) -> Result<Vec<SkillSummary>, String>;
    /// 导入技能包（.zip / .tar.gz）。
    fn import_skill_package(&self, path: String) -> Result<Vec<SkillSummary>, String>;
    /// 创建技能（写 SKILL.md + YAML frontmatter）。
    fn create_skill(&self, request: CreateSkillRequest) -> Result<SkillSummary, String>;
    /// 删除技能（仅 user 来源）。
    fn remove_skill(&self, name: String) -> Result<(), String>;
    /// 启停技能。
    fn toggle_skill(&self, name: String, enabled: bool) -> Result<SkillSummary, String>;
    /// 更新技能字段。
    fn update_skill(&self, name: String, updates: SkillUpdate) -> Result<SkillSummary, String>;
    /// 会话已启用的技能名列表。
    fn get_session_skills(&self, session_id: String) -> Result<Vec<String>, String>;
    /// 设置会话启用技能。
    fn set_session_skills(&self, session_id: String, skill_names: Vec<String>) -> Result<(), String>;
    /// 目录选择（原生对话框；接口保留，实现可能交由 Electron 端）。
    fn select_directory(&self) -> Result<Vec<String>, String>;
}
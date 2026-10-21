//! 技能（skill）接口：搜索 / 发现 / 导入 / 增删改 / 会话绑定。

mod create_skill;
mod discover_skills;
mod dto;
mod get_session_skills;
mod get_skill;
mod import_skill_package;
mod import_skills;
mod list_skills;
mod remove_skill;
mod search_skills;
mod select_directory;
mod set_session_skills;
mod toggle_skill;
mod update_skill;

pub use create_skill::create_skill;
pub use discover_skills::discover_skills;
pub use dto::{CreateSkillRequest, DiscoveredSkill, SkillDoc, SkillSummary, SkillUpdate};
pub use get_session_skills::get_session_skills;
pub use get_skill::get_skill;
pub use import_skill_package::import_skill_package;
pub use import_skills::import_skills;
pub use list_skills::list_skills;
pub use remove_skill::remove_skill;
pub use search_skills::search_skills;
pub use select_directory::select_directory;
pub use set_session_skills::set_session_skills;
pub use toggle_skill::toggle_skill;
pub use update_skill::update_skill;
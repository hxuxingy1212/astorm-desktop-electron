//! git 接口：环境 / 状态 / 暂存 / 提交 / 推送 / 差异。

mod check_git_env;
mod dto;
mod get_git_diff_summary;
mod get_git_file_diff;
mod git_commit;
mod git_ignored;
mod git_init;
mod git_push;
mod git_stage;
mod git_status;
mod git_unstage;

pub use check_git_env::check_git_env;
pub use dto::{DiffFileContent, DiffFileEntry, GitEnvInfo, GitFileChange, GitStatus};
pub use get_git_diff_summary::get_git_diff_summary;
pub use get_git_file_diff::get_git_file_diff;
pub use git_commit::git_commit;
pub use git_ignored::git_ignored;
pub use git_init::git_init;
pub use git_push::git_push;
pub use git_stage::git_stage;
pub use git_status::git_status;
pub use git_unstage::git_unstage;
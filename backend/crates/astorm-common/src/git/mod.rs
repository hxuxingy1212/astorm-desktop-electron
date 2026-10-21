//! git 接口契约：环境 / 状态 / 暂存 / 提交 / 推送 / 差异（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// git 环境信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEnvInfo {
    /// 系统是否可用 git
    pub git_available: bool,
    /// 目录是否在仓库内
    pub has_repo: bool,
}

/// 单个文件变更。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    /// 变更 id
    pub id: String,
    /// 文件路径
    pub path: String,
    /// 状态字（A/M/D/R/C）
    pub status: String,
    /// 变更差异文本
    pub diff: String,
}

/// 工作区状态（暂存 / 未暂存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// 已暂存变更
    pub staged: Vec<GitFileChange>,
    /// 未暂存变更
    pub unstaged: Vec<GitFileChange>,
}

/// 差异摘要条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileEntry {
    /// 文件路径
    pub path: String,
    /// 状态字（A/M/D/R/C；未跟踪为 `?`）
    pub status: String,
    /// 新增行数
    pub additions: u32,
    /// 删除行数
    pub deletions: u32,
}

/// 单个文件的差异全文（代码审查页签渲染用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    /// 统一 diff 文本（git diff 原样输出）；未跟踪文件合成为「全新增」
    pub diff: String,
    /// 新侧总行数：staged 取索引 blob、unstaged 取工作树文件的行数。
    /// 前端据此补齐**尾部**未修改区段的折叠条长度；二进制 / 不可读 = 0。
    pub new_lines: u32,
}

/// git 门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait Git {
    /// 初始化仓库。
    fn git_init(&self, dir: String) -> Result<String, String>;
    /// 工作区状态（staged / unstaged）。
    fn git_status(&self, dir: String) -> Result<GitStatus, String>;
    /// 暂存指定文件。
    fn git_stage(&self, dir: String, files: Vec<String>) -> Result<String, String>;
    /// 取消暂存指定文件。
    fn git_unstage(&self, dir: String, files: Vec<String>) -> Result<String, String>;
    /// 提交（先暂存 files 再 commit message）。
    fn git_commit(&self, dir: String, message: String, files: Vec<String>) -> Result<String, String>;
    /// 推送（SSH 密钥认证）。
    fn git_push(&self, dir: String) -> Result<String, String>;
    /// git 环境检查（是否可用 / 是否仓库内）。
    fn check_git_env(&self, dir: String) -> Result<GitEnvInfo, String>;
    /// 当前分支名（`rev-parse --abbrev-ref HEAD`）。
    ///
    /// 供系统提示词的「会话环境」块**每轮实时**取分支——不再读会话记录里的快照，因而
    /// agent 自己 checkout / 用户在外部终端切分支后，下一轮提示词拿到的就是新值。
    /// 非仓库 / git 不可用 → `Err`；分离 HEAD（git 返回字面量 `HEAD`，并非分支名）→ 空串。
    fn git_current_branch(&self, dir: String) -> Result<String, String>;
    /// 差异摘要（代码审查页签的文件列表）。
    ///
    /// `staged` = true 取索引与 HEAD 的差异（`git diff --cached`），false 取工作树与索引的差异。
    /// 未跟踪文件（`git ls-files --others`）只在 `staged = false` 时计入，状态字 `?`。
    fn get_git_diff_summary(&self, dir: String, staged: bool) -> Result<Vec<DiffFileEntry>, String>;
    /// 单个文件差异全文 + 新侧总行数。
    ///
    /// `staged` 语义同 [`Self::get_git_diff_summary`]。未跟踪文件（`git diff` 无输出）合成为
    /// 「每行前加 `+`」的全新增 diff，供未跟踪文件也能在审查页签里查看。
    fn get_git_file_diff(&self, dir: String, path: String, staged: bool)
        -> Result<GitFileDiff, String>;
    /// 被 git 忽略的路径列表（.gitignore 命中;目录经过滤保留其路径,如 node_modules）。
    fn git_ignored(&self, dir: String) -> Result<Vec<String>, String>;
}
//! git DTO 镜像。

use astorm_common::git::{
    DiffFileEntry as CoreDiffFileEntry, GitEnvInfo as CoreGitEnvInfo,
    GitFileChange as CoreGitFileChange, GitFileDiff as CoreGitFileDiff, GitStatus as CoreGitStatus,
};
use napi_derive::napi;
use serde::Serialize;

/// git 环境信息。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEnvInfo {
    /// 系统是否可用 git
    pub git_available: bool,
    /// 目录是否在仓库内
    pub has_repo: bool,
}

impl From<CoreGitEnvInfo> for GitEnvInfo {
    fn from(v: CoreGitEnvInfo) -> Self {
        GitEnvInfo {
            git_available: v.git_available,
            has_repo: v.has_repo,
        }
    }
}

/// 单个文件变更。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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

impl From<CoreGitFileChange> for GitFileChange {
    fn from(v: CoreGitFileChange) -> Self {
        GitFileChange {
            id: v.id,
            path: v.path,
            status: v.status,
            diff: v.diff,
        }
    }
}

/// 工作区状态（暂存 / 未暂存）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// 已暂存变更
    pub staged: Vec<GitFileChange>,
    /// 未暂存变更
    pub unstaged: Vec<GitFileChange>,
}

impl From<CoreGitStatus> for GitStatus {
    fn from(v: CoreGitStatus) -> Self {
        GitStatus {
            staged: v.staged.into_iter().map(Into::into).collect(),
            unstaged: v.unstaged.into_iter().map(Into::into).collect(),
        }
    }
}

/// 差异摘要条目。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileEntry {
    /// 文件路径
    pub path: String,
    /// 状态字（A/M/D/R/C）
    pub status: String,
    /// 新增行数
    pub additions: u32,
    /// 删除行数
    pub deletions: u32,
}

impl From<CoreDiffFileEntry> for DiffFileEntry {
    fn from(v: CoreDiffFileEntry) -> Self {
        DiffFileEntry {
            path: v.path,
            status: v.status,
            additions: v.additions,
            deletions: v.deletions,
        }
    }
}

/// 单个文件的差异全文（代码审查页签渲染用）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFileContent {
    /// 统一 diff 文本（git diff 原样输出）
    pub diff: String,
    /// 新侧总行数（前端据此补齐尾部未修改区段的折叠条长度；二进制 / 不可读 = 0）
    pub new_lines: u32,
}

impl From<CoreGitFileDiff> for DiffFileContent {
    fn from(v: CoreGitFileDiff) -> Self {
        DiffFileContent {
            diff: v.diff,
            new_lines: v.new_lines,
        }
    }
}
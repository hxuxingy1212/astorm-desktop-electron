//! 知识库 API 契约（门面 trait + 类型，纯 serde，不含 napi）。
//!
//! 数据 = 真实文件系统：文件夹 = 目录，文档 = 文件（默认 `.md`）。
//! 展示元数据（图标 / 颜色 / 标签）集中存于知识库根 `.manifest.json`（相对路径 → 元数据），
//! 文件系统操作与清单更新由门面实现负责保持一致。

use serde::{Deserialize, Serialize};

/// 知识库条目（文件系统真实条目 + 元数据合并视图）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbItem {
    /// id = 相对路径（`/` 分隔；根层条目即自身名称）
    pub id: String,
    /// 类型："folder" | "document"
    pub node_type: String,
    /// 展示名（文件系统名，文档含扩展名）
    pub name: String,
    /// 父条目 id（根层为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// 颜色 key（前端预设色板）
    pub color: String,
    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 文档大小（字节）；文件夹为 0
    pub size: i64,
    /// 创建时间（epoch ms）
    pub created_at: i64,
    /// 更新时间（epoch ms）
    pub updated_at: i64,
    /// 绝对路径（前端打开预览等场景使用）
    pub abs_path: String,
}

/// 新建条目请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbCreateRequest {
    /// 父目录相对路径（空串 = 知识库根层）
    pub parent: String,
    /// 名称（文档缺扩展名时自动补 `.md`）
    pub name: String,
    /// 类型："folder" | "document"
    pub kind: String,
    /// 颜色 key（可选；缺省 blue）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 标签（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// 文档初始内容（可选；文件夹忽略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// 元数据更新请求（不影响文件系统，仅写清单）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbMetaUpdate {
    /// 目标条目相对路径
    pub path: String,
    /// 颜色 key（None = 不变）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 标签（None = 不变；空数组 = 清空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// 知识库门面契约。
pub trait Knowledge {
    /// 列出整棵树（根目录缺失时自动创建；文件夹在前、名称排序）。
    fn list(&self) -> Result<Vec<KbItem>, String>;
    /// 新建文件夹 / 文档（真实落盘）。
    fn create(&self, request: KbCreateRequest) -> Result<KbItem, String>;
    /// 重命名（文件夹同步迁移后代元数据）。
    fn rename(&self, path: String, new_name: String) -> Result<KbItem, String>;
    /// 删除（文件夹递归删除；同步清理元数据）。
    fn delete(&self, path: String) -> Result<(), String>;
    /// 更新展示元数据（颜色 / 标签）。
    fn update_meta(&self, meta: KbMetaUpdate) -> Result<KbItem, String>;
    /// 读取文档内容（>1MB 拒绝）。
    fn read(&self, path: String) -> Result<String, String>;
    /// 读取文档字节（预览用；>50MB 拒绝）。
    fn read_bytes(&self, path: String) -> Result<Vec<u8>, String>;
    /// 写入文档内容。
    fn write(&self, path: String, content: String) -> Result<(), String>;
    /// 从本地磁盘导入文件 / 目录到指定父目录（目录递归拷贝;重名自动加序号后缀）。
    fn import(&self, parent: String, sources: Vec<String>) -> Result<Vec<KbItem>, String>;
}

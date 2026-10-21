//! 本地文件接口契约：目录列表 / 文件读取 / 应用路径（纯 serde，不依赖 napi）。

use serde::{Deserialize, Serialize};

/// 目录节点（递归树）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    /// 节点名
    pub name: String,
    /// 节点类型："folder" | "file"
    #[serde(rename = "type")]
    pub node_type: String,
    /// 子节点（文件夹时有值）
    pub children: Option<Vec<FileNode>>,
}

/// 本地文件门面：由业务层（`astorm-core`）实现，
/// napi 绑定层经 `astorm-core` 的 `App` 门面调用。
pub trait File {
    /// 列出目录树（排除 node_modules/target/.git 等噪音目录）。
    fn list_directory(&self, dir: String) -> Result<Vec<FileNode>, String>;
    /// 读取文件内容（1MB 上限；二进制扩展名按约定处理）。
    fn read_file_content(&self, path: String) -> Result<String, String>;
    /// 应用可执行文件所在目录。
    fn get_app_path(&self) -> Result<String, String>;
    /// 项目根目录（当前工作目录）。
    fn get_project_root(&self) -> Result<String, String>;
}
//! 本地文件 DTO 镜像。

use astorm_common::file::FileNode as CoreFileNode;
use napi_derive::napi;
use serde::Serialize;

/// 目录节点（递归树）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
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

impl From<CoreFileNode> for FileNode {
    fn from(v: CoreFileNode) -> Self {
        FileNode {
            name: v.name,
            node_type: v.node_type,
            children: v
                .children
                .map(|c| c.into_iter().map(Into::into).collect()),
        }
    }
}
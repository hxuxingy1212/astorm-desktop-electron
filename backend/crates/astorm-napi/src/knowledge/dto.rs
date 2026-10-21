//! 知识库 DTO 镜像。

use astorm_common::knowledge::{
    KbCreateRequest as CoreKbCreateRequest, KbItem as CoreKbItem, KbMetaUpdate as CoreKbMetaUpdate,
};
use napi_derive::napi;
use serde::Serialize;

/// 知识库条目（真实文件系统条目 + 元数据合并视图）。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbItem {
    /// id = 相对路径（`/` 分隔）
    pub id: String,
    /// 类型："folder" | "document"
    #[serde(rename = "type")]
    pub node_type: String,
    /// 展示名（文档含扩展名）
    pub name: String,
    /// 父条目 id（根层为 null）
    pub parent_id: Option<String>,
    /// 颜色 key
    pub color: String,
    /// 标签
    pub tags: Vec<String>,
    /// 文档大小（字节）；文件夹为 0
    pub size: i64,
    /// 创建时间（epoch ms）
    pub created_at: i64,
    /// 更新时间（epoch ms）
    pub updated_at: i64,
    /// 绝对路径
    pub abs_path: String,
}

impl From<CoreKbItem> for KbItem {
    fn from(v: CoreKbItem) -> Self {
        KbItem {
            id: v.id,
            node_type: v.node_type,
            name: v.name,
            parent_id: v.parent_id,
            color: v.color,
            tags: v.tags,
            size: v.size,
            created_at: v.created_at,
            updated_at: v.updated_at,
            abs_path: v.abs_path,
        }
    }
}

/// 新建条目请求。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbCreateRequest {
    /// 父目录相对路径（空串 = 根层）
    pub parent: String,
    /// 名称（文档缺扩展名自动补 `.md`）
    pub name: String,
    /// 类型："folder" | "document"
    pub kind: String,
    /// 颜色 key（可选）
    pub color: Option<String>,
    /// 标签（可选）
    pub tags: Option<Vec<String>>,
    /// 文档初始内容（可选）
    pub content: Option<String>,
}

impl From<KbCreateRequest> for CoreKbCreateRequest {
    fn from(v: KbCreateRequest) -> Self {
        CoreKbCreateRequest {
            parent: v.parent,
            name: v.name,
            kind: v.kind,
            color: v.color,
            tags: v.tags,
            content: v.content,
        }
    }
}

/// 元数据更新请求。
#[napi(object)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbMetaUpdate {
    /// 目标条目相对路径
    pub path: String,
    /// 颜色 key（null = 不变）
    pub color: Option<String>,
    /// 标签（null = 不变；空数组 = 清空）
    pub tags: Option<Vec<String>>,
}

impl From<KbMetaUpdate> for CoreKbMetaUpdate {
    fn from(v: KbMetaUpdate) -> Self {
        CoreKbMetaUpdate {
            path: v.path,
            color: v.color,
            tags: v.tags,
        }
    }
}

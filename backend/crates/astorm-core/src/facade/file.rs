//! 本地文件门面：目录树递归列出 + 文件内容读取。
//! 接口见 [`astorm_common::file`]。

use std::fs;
use std::path::Path;

use astorm_common::file::{File, FileNode};

/// 噪音目录（递归时整支跳过）。
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    ".git",
    ".vite",
    "__pycache__",
    ".idea",
    ".next",
    ".nuxt",
];
/// 忽略的文件。
const EXCLUDED_FILES: &[&str] = &[".DS_Store"];

/// 二进制扩展名黑名单（读取返回空串；≤50KB 时尝试兜底读文本，见 `read_file_content`）。
const BINARY_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg", ".mp3", ".wav", ".flac",
    ".ogg", ".mp4", ".mkv", ".avi", ".webm", ".mov", ".zip", ".rar", ".7z", ".tar", ".gz",
    ".exe", ".dll", ".bin", ".wasm", ".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx",
];

/// 文件内容读取上限（1MB）。
const MAX_CONTENT_BYTES: u64 = 1024 * 1024;

/// 本地文件门面。
pub struct FileImpl;

impl File for FileImpl {
    /// 递归列出目录树：文件夹优先、名称排序；子目录无权限时 children 置 None 不阻断。
    fn list_directory(&self, dir: String) -> Result<Vec<FileNode>, String> {
        list_dir_recursive(Path::new(&dir))
    }

    /// 读取文件内容：>1MB 或二进制扩展名 → 空串；二进制但 ≤50KB 尝试兜底读文本。
    fn read_file_content(&self, path: String) -> Result<String, String> {
        let p = Path::new(&path);
        let meta = fs::metadata(p).map_err(|e| format!("读取元数据失败: {e}"))?;
        if !meta.is_file() {
            return Err("不是文件".into());
        }
        if meta.len() > MAX_CONTENT_BYTES {
            return Ok(String::new()); // 超出上限,不读
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        let is_binary = BINARY_EXTS.contains(&ext.as_str());
        match fs::read(&p) {
            Ok(bytes) => {
                // 二进制但小文件：尝试兜底读 UTF-8（参考桌面版行为）
                if is_binary && bytes.len() > 50 * 1024 {
                    return Ok(String::new());
                }
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            Err(_e) if is_binary => Ok(String::new()),
            Err(e) => Err(format!("读取文件失败: {e}")),
        }
    }

    fn get_app_path(&self) -> Result<String, String> {
        // 可执行文件目录（扩展进程内不依赖 napi/electron,返回编译期环境不可行;先保持骨架）
        Err(super::not_impl("file", "get_app_path"))
    }

    fn get_project_root(&self) -> Result<String, String> {
        // 会话工作区由会话记录提供;项目根暂冲突,保持骨架
        Err(super::not_impl("file", "get_project_root"))
    }
}

/// 递归构建目录树（参考桌面版 `list_dir_recursive`）。
fn list_dir_recursive(dir: &Path) -> Result<Vec<FileNode>, String> {
    let mut nodes: Vec<FileNode> = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("读取目录失败 {dir:?}: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if EXCLUDED_FILES.contains(&name.as_str()) {
            continue;
        }
        let _ftype = entry
            .file_type()
            .map_err(|e| format!("读取类型失败 {name}: {e}"))?;
        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();
        if is_dir {
            if EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let children = match list_dir_recursive(&entry_path) {
                Ok(c) => Some(c),
                Err(_) => None, // 无权限等:保留节点但不展开
            };
            nodes.push(FileNode {
                name,
                node_type: "folder".into(),
                children,
            });
        } else {
            nodes.push(FileNode {
                name,
                node_type: "file".into(),
                children: None,
            });
        }
    }
    // 文件夹优先,再按名称排序(参考桌面版行为)
    nodes.sort_by(|a, b| match (a.node_type.as_str(), b.node_type.as_str()) {
        ("folder", "file") => std::cmp::Ordering::Less,
        ("file", "folder") => std::cmp::Ordering::Greater,
        _ => {
            let na = a.name.to_lowercase();
            let nb = b.name.to_lowercase();
            na.cmp(&nb)
        }
    });
    Ok(nodes)
}